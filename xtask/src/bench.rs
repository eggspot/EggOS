//! `cargo xtask bench`: the kernel's boot-time benchmarks, compared against a committed
//! baseline (D27).
//!
//! The kernel (feature `qemu-bench`) prints one `EGGOS-BENCH:` line per benchmark. QEMU runs
//! with `-icount shift=0`, so the guest clock advances exactly 1 ns per instruction and every
//! result is an instruction count per iteration: deterministic, independent of how fast or
//! busy the host is, and therefore comparable with a committed number. A result above its
//! baseline by more than the tolerance is a regression and fails the run.
//!
//! Two more results per architecture come from the release kernel (D116): `boot_to_prompt`,
//! the guest time from the kernel's entry to the shell waiting at its first prompt, read by a
//! kernel built with `qemu-boot-clock` and booted serial-only under the same `-icount`; and
//! `release_image_bytes`, the size of the image `cargo xtask release` ships, which may not grow
//! at all without a baseline update (CLAUDE.md, note 8).

use crate::{Arch, qemu};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// Must match `eggos_kernel::bench::MARKER`.
const MARKER: &str = "EGGOS-BENCH:";

/// QEMU in deterministic instruction-counting mode: 2^0 ns of guest time per instruction, and
/// an idle CPU skips straight to its next timer deadline instead of waiting in real time.
/// `-seed` fixes what QEMU hands out as random (the device tree's `rng-seed`, `RDRAND` and
/// `RDSEED`), so a path that ever depends on random values runs the same way each time (D56).
pub const ICOUNT_ARGS: &[&str] = &["-icount", "shift=0,sleep=off", "-seed", "1"];

/// Benchmarks run on one CPU: with `-icount`, QEMU runs every CPU in one thread in turn, so
/// other CPUs' instructions would count against the measured ones (D40).
const BENCH_CPUS: u32 = 1;

/// How much slower than its baseline a result may be before it fails: relative, plus an
/// absolute allowance so results of a few instructions are not failed by one instruction.
const TOLERANCE_RELATIVE: f64 = 0.01;
const TOLERANCE_ABSOLUTE_NS: f64 = 0.5;

const RELEASE_IMAGE_BYTES: &str = "release_image_bytes";

/// Results that are sizes in bytes, gated exactly: any growth fails.
const SIZES: &[&str] = &[RELEASE_IMAGE_BYTES];

/// Header of the baseline file, rewritten by `--update-baseline`.
const BASELINE_HEADER: &str = "\
# EggOS boot-time benchmark baseline, written by `cargo xtask bench --arch all --update-baseline`.
# One line per result: <arch> <benchmark> <ns per iteration>. The kernel runs under QEMU
# `-icount shift=0` (1 ns per instruction), so each number is instructions per iteration.
# `cargo xtask bench` (and CI) fails when a result is slower than its line here by more than
# 1% + 0.5 ns. Update it in the PR that makes a change faster, or that the owner accepted as
# slower, and say why in the PR (CLAUDE.md, \"Never merge a performance regression\").
# `<arch> firmware <version>` names the firmware the results were recorded with: benchmarks
# that run inside it are gated only on the same firmware, and `<arch> qemu <version>` names
# the QEMU that QEMU-dependent results (boot_to_prompt) were recorded with, likewise. Record
# the baseline with CI's QEMU (Ubuntu 24.04 packages).
";

/// Benchmarks that execute firmware code (the SBI implementation on RISC-V), so their counts
/// depend on the firmware build QEMU loads: (architecture, firmware banner prefix,
/// benchmarks). Those are compared only when the firmware matches the baseline's.
const FIRMWARE_DEPENDENT: &[(&str, &str, &[&str])] = &[(
    "riscv64",
    "OpenSBI v",
    &["timer_arm", "timer_interrupt_round_trip"],
)];

/// Benchmarks whose counts depend on the QEMU version, which builds each machine's devices
/// and tables (Ubuntu 24.04's QEMU and QEMU 10.2 differ by up to 20% on x86_64's boot): those
/// are compared only when QEMU's version matches the baseline's `<arch> qemu <version>` line.
const QEMU_DEPENDENT: &[&str] = &["boot_to_prompt"];

/// Results in nanoseconds (= instructions, under icount) per iteration, keyed by
/// (architecture, benchmark).
type Results = BTreeMap<(String, String), f64>;

/// A set of results and the firmware and QEMU versions each architecture ran with (when they
/// matter).
#[derive(Debug, Default, PartialEq)]
struct Measurements {
    results: Results,
    firmware: BTreeMap<String, String>,
    qemu: BTreeMap<String, String>,
}

fn baseline_path() -> PathBuf {
    crate::workspace_root().join("benchmarks/baseline.txt")
}

/// Builds and boots the benchmark kernel for each of `arches`, then compares the results with
/// the baseline, or with `update` writes them into it (keeping other architectures' lines).
pub fn run(arches: &[&Arch], update: bool) -> Result<(), String> {
    let mut measured = Measurements::default();
    for arch in arches {
        let kernel = crate::build_kernel(arch, Some("qemu-bench"))?;
        let console = qemu::boot_test(
            arch,
            &kernel,
            qemu::Expect::Benchmarks,
            BENCH_CPUS,
            qemu::Display::Attached,
            ICOUNT_ARGS,
        )?;
        let results = parse_console(&console)?;
        if results.is_empty() {
            return Err(format!(
                "{}: the kernel printed no benchmark results",
                arch.name
            ));
        }
        for (name, ns_per_iteration) in results {
            measured
                .results
                .insert((arch.name.to_owned(), name), ns_per_iteration);
        }
        if let Some(version) = firmware_version(arch.name, &console) {
            measured.firmware.insert(arch.name.to_owned(), version);
        }
        for (name, value) in release_kernel_results(arch)? {
            measured.results.insert((arch.name.to_owned(), name), value);
        }
        measured
            .qemu
            .insert(arch.name.to_owned(), qemu_version(arch)?);
    }

    let path = baseline_path();
    let baseline = match std::fs::read_to_string(&path) {
        Ok(text) => parse_baseline(&text)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Measurements::default(),
        Err(e) => return Err(format!("could not read {}: {e}", path.display())),
    };

    let ran: Vec<&str> = arches.iter().map(|a| a.name).collect();
    if update {
        let (merged, kept) = updated_baseline(baseline, measured, &ran);
        for line in kept {
            eprintln!("xtask: {line}");
        }
        write_baseline(&path, &merged)?;
        eprintln!("xtask: wrote {}", path.display());
        return Ok(());
    }

    let report = compare(&baseline, &measured, &ran);
    eprint!("{}", report.text);
    if report.failures.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "benchmarks failed against {}:\n  {}\n(if the change is intended, run \
             `cargo xtask bench --arch all --update-baseline` and justify it in the PR)",
            path.display(),
            report.failures.join("\n  ")
        ))
    }
}

fn qemu_version(arch: &Arch) -> Result<String, String> {
    let output = std::process::Command::new(arch.qemu)
        .arg("--version")
        .output()
        .map_err(|e| format!("could not run {} --version: {e}", arch.qemu))?;
    parse_qemu_version(&String::from_utf8_lossy(&output.stdout))
        .ok_or_else(|| format!("{} --version printed no version", arch.qemu))
}

fn parse_qemu_version(text: &str) -> Option<String> {
    text.lines()
        .next()?
        .split_whitespace()
        .skip_while(|word| *word != "version")
        .nth(1)
        .map(str::to_owned)
}

fn release_kernel_results(arch: &Arch) -> Result<Vec<(String, f64)>, String> {
    let image = crate::build_kernel(arch, None)?;
    let bytes = std::fs::metadata(&image)
        .map_err(|e| format!("{}: {e}", image.display()))?
        .len();
    let kernel = crate::build_boot_clock_kernel(arch)?;
    let console = qemu::boot_test(
        arch,
        &kernel,
        qemu::Expect::BootClock,
        BENCH_CPUS,
        qemu::Display::Absent,
        ICOUNT_ARGS,
    )?;
    let mut results = parse_console(&console)?;
    results.push((RELEASE_IMAGE_BYTES.to_owned(), bytes as f64));
    Ok(results)
}

/// The baseline with the results of the architectures in `ran` replaced by `measured`'s,
/// except firmware-dependent results measured on other firmware than the baseline's: those
/// keep their baseline value (with its firmware line), so updating from a machine whose QEMU
/// ships another firmware build cannot move CI's numbers. Also returns a note per kept value.
fn updated_baseline(
    baseline: Measurements,
    measured: Measurements,
    ran: &[&str],
) -> (Measurements, Vec<String>) {
    let other_firmware = |arch: &str| {
        baseline
            .firmware
            .get(arch)
            .is_some_and(|then| measured.firmware.get(arch) != Some(then))
    };
    let other_qemu = |arch: &str| {
        baseline
            .qemu
            .get(arch)
            .is_some_and(|then| measured.qemu.get(arch) != Some(then))
    };
    let elsewhere = |arch: &str, name: &str| {
        (is_firmware_dependent(arch, name) && other_firmware(arch))
            || (QEMU_DEPENDENT.contains(&name) && other_qemu(arch))
    };
    let mut kept = Vec::new();
    let mut merged = Measurements::default();
    for ((arch, name), value) in &baseline.results {
        let keep = !ran.contains(&arch.as_str()) || elsewhere(arch, name);
        if keep {
            if ran.contains(&arch.as_str()) {
                kept.push(format!(
                    "kept {arch} {name} {value:.2}: measured on other firmware or QEMU than \
                     the baseline's (record it with CI's QEMU)"
                ));
            }
            merged.results.insert((arch.clone(), name.clone()), *value);
        }
    }
    for (arch, version) in &baseline.firmware {
        if !ran.contains(&arch.as_str()) || other_firmware(arch) {
            merged.firmware.insert(arch.clone(), version.clone());
        }
    }
    for (arch, version) in &baseline.qemu {
        if !ran.contains(&arch.as_str()) || other_qemu(arch) {
            merged.qemu.insert(arch.clone(), version.clone());
        }
    }
    for ((arch, name), value) in measured.results {
        if elsewhere(&arch, &name) {
            if !merged.results.contains_key(&(arch.clone(), name.clone())) {
                kept.push(format!(
                    "not recorded: {arch} {name} {value:.2} was measured on other firmware or \
                     QEMU than the baseline's (record it with CI's QEMU)"
                ));
            }
            continue;
        }
        merged.results.insert((arch, name), value);
    }
    for (arch, version) in measured.firmware {
        merged.firmware.entry(arch).or_insert(version);
    }
    for (arch, version) in measured.qemu {
        merged.qemu.entry(arch).or_insert(version);
    }
    (merged, kept)
}

/// Parses the kernel's `EGGOS-BENCH: <name> iterations=<n> ticks=<t> frequency=<hz>` lines.
fn parse_console(console: &str) -> Result<Vec<(String, f64)>, String> {
    let mut results = Vec::new();
    for line in console.lines().map(str::trim_end) {
        let Some(rest) = line.strip_prefix(MARKER) else {
            continue;
        };
        let bad = || format!("malformed benchmark line `{line}`");
        let mut words = rest.split_whitespace();
        let name = words.next().ok_or_else(bad)?;
        let mut field = |key: &str| -> Result<u64, String> {
            words
                .next()
                .and_then(|w| w.strip_prefix(key))
                .and_then(|w| w.strip_prefix('='))
                .and_then(|v| v.parse().ok())
                .ok_or_else(bad)
        };
        let iterations = field("iterations")?;
        let ticks = field("ticks")?;
        let frequency = field("frequency")?;
        if iterations == 0 || frequency == 0 {
            return Err(bad());
        }
        let ns_per_iteration = ticks as f64 * 1e9 / frequency as f64 / iterations as f64;
        results.push((name.to_owned(), ns_per_iteration));
    }
    Ok(results)
}

/// The firmware version `arch`'s console shows, if its benchmarks depend on firmware: the
/// rest of the firmware's banner line (e.g. `1.3` from `OpenSBI v1.3`).
fn firmware_version(arch: &str, console: &str) -> Option<String> {
    let (_, prefix, _) = FIRMWARE_DEPENDENT.iter().find(|(a, _, _)| *a == arch)?;
    console
        .lines()
        .find_map(|l| l.trim().strip_prefix(prefix))
        .map(|version| format!("{prefix}{version}").replace(' ', "-"))
}

fn is_firmware_dependent(arch: &str, benchmark: &str) -> bool {
    FIRMWARE_DEPENDENT
        .iter()
        .any(|(a, _, benchmarks)| *a == arch && benchmarks.contains(&benchmark))
}

fn parse_baseline(text: &str) -> Result<Measurements, String> {
    let mut baseline = Measurements::default();
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let words: Vec<&str> = line.split_whitespace().collect();
        if let [arch, "firmware", version] = words[..] {
            baseline
                .firmware
                .insert(arch.to_owned(), version.to_owned());
            continue;
        }
        if let [arch, "qemu", version] = words[..] {
            baseline.qemu.insert(arch.to_owned(), version.to_owned());
            continue;
        }
        let [arch, name, value] = words[..] else {
            return Err(format!("malformed baseline line `{line}`"));
        };
        let value: f64 = value
            .parse()
            .map_err(|_| format!("malformed baseline line `{line}`"))?;
        baseline
            .results
            .insert((arch.to_owned(), name.to_owned()), value);
    }
    Ok(baseline)
}

fn write_baseline(path: &Path, baseline: &Measurements) -> Result<(), String> {
    let mut text = BASELINE_HEADER.to_owned();
    for (arch, version) in &baseline.firmware {
        writeln!(text, "{arch} firmware {version}").expect("writing to a String");
    }
    for (arch, version) in &baseline.qemu {
        writeln!(text, "{arch} qemu {version}").expect("writing to a String");
    }
    for ((arch, name), value) in &baseline.results {
        writeln!(text, "{arch} {name} {value:.2}").expect("writing to a String");
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::write(path, text).map_err(|e| format!("could not write {}: {e}", path.display()))
}

#[derive(Debug, Default)]
struct Comparison {
    /// A table of every result against its baseline.
    text: String,
    /// One line per regression, new benchmark without a baseline, or baseline without a result.
    failures: Vec<String>,
}

/// Compares `measured` with `baseline` for the architectures in `ran`.
fn compare(baseline: &Measurements, measured: &Measurements, ran: &[&str]) -> Comparison {
    let mut report = Comparison::default();
    for arch in ran {
        let (now, then) = (measured.firmware.get(*arch), baseline.firmware.get(*arch));
        let unreadable_firmware_would_ungate_its_results =
            now.is_none() && FIRMWARE_DEPENDENT.iter().any(|(a, _, _)| a == arch);
        if unreadable_firmware_would_ungate_its_results {
            report.failures.push(format!(
                "{arch}: could not read the firmware version from the console"
            ));
        }
        if now != then {
            let name = |v: Option<&String>| v.map_or("no firmware".to_owned(), String::clone);
            let _ = writeln!(
                report.text,
                "  {arch}: firmware {}, baseline recorded with {}",
                name(now),
                name(then)
            );
        }
        let (now, then) = (measured.qemu.get(*arch), baseline.qemu.get(*arch));
        if now != then {
            let name = |v: Option<&String>| v.map_or("unknown".to_owned(), String::clone);
            let _ = writeln!(
                report.text,
                "  {arch}: QEMU {}, baseline recorded with QEMU {}",
                name(now),
                name(then)
            );
        }
    }
    for ((arch, name), &now) in &measured.results {
        let key = (arch.clone(), name.clone());
        let exact = SIZES.contains(&name.as_str());
        let unit = if exact { "bytes" } else { "ns" };
        let Some(&before) = baseline.results.get(&key) else {
            let _ = writeln!(
                report.text,
                "  {arch} {name}: {now:.2} {unit} (no baseline)"
            );
            report
                .failures
                .push(format!("{arch} {name}: no baseline (new benchmark?)"));
            continue;
        };
        let change = if before > 0.0 {
            (now - before) / before * 100.0
        } else {
            0.0
        };
        let limit = match exact {
            true => before,
            false => before * (1.0 + TOLERANCE_RELATIVE) + TOLERANCE_ABSOLUTE_NS,
        };
        let faster_than = match exact {
            true => before,
            false => before * (1.0 - TOLERANCE_RELATIVE) - TOLERANCE_ABSOLUTE_NS,
        };
        let other_firmware = is_firmware_dependent(arch, name)
            && measured.firmware.get(arch) != baseline.firmware.get(arch);
        let other_qemu = QEMU_DEPENDENT.contains(&name.as_str())
            && measured.qemu.get(arch) != baseline.qemu.get(arch);
        let note = if other_firmware {
            "  not gated: firmware differs from the baseline's"
        } else if other_qemu {
            "  not gated: QEMU differs from the baseline's"
        } else if now > limit {
            report.failures.push(format!(
                "{arch} {name}: {now:.2} {unit}, baseline {before:.2} {unit} ({change:+.1}%)"
            ));
            "  REGRESSION"
        } else if now < faster_than {
            "  faster: consider --update-baseline"
        } else {
            ""
        };
        let _ = writeln!(
            report.text,
            "  {arch} {name}: {now:.2} {unit}, baseline {before:.2} {unit} ({change:+.1}%){note}"
        );
    }
    for (arch, name) in baseline.results.keys() {
        if ran.contains(&arch.as_str())
            && !measured.results.contains_key(&(arch.clone(), name.clone()))
        {
            report.failures.push(format!(
                "{arch} {name}: in the baseline but not measured (benchmark removed?)"
            ));
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    fn results(entries: &[(&str, &str, f64)]) -> Measurements {
        Measurements {
            results: entries
                .iter()
                .map(|&(a, n, v)| ((a.to_owned(), n.to_owned()), v))
                .collect(),
            firmware: BTreeMap::new(),
            qemu: BTreeMap::new(),
        }
    }

    fn with_firmware(mut m: Measurements, arch: &str, version: &str) -> Measurements {
        m.firmware.insert(arch.to_owned(), version.to_owned());
        m
    }

    #[test]
    fn parses_kernel_lines_into_ns_per_iteration() {
        let console = "EggOS 0.1.0 (x)\r\n\
            EGGOS-BENCH: clock_read iterations=10000 ticks=62500 frequency=62500000\r\n\
            EGGOS-SELFTEST: PASS\n\
            EGGOS-BENCH: trap iterations=4 ticks=10 frequency=1000000000\n";
        let parsed = parse_console(console).unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].0, "clock_read");
        let one_ms_over_10_000_iterations_in_ns = 100.0;
        assert!((parsed[0].1 - one_ms_over_10_000_iterations_in_ns).abs() < 1e-9);
        assert!((parsed[1].1 - 2.5).abs() < 1e-9);
    }

    #[test]
    fn rejects_malformed_kernel_lines() {
        for line in [
            "EGGOS-BENCH:",
            "EGGOS-BENCH: x iterations=1 ticks=1",
            "EGGOS-BENCH: x iterations=0 ticks=1 frequency=1",
            "EGGOS-BENCH: x iterations=1 ticks=1 frequency=0",
            "EGGOS-BENCH: x ticks=1 iterations=1 frequency=1",
            "EGGOS-BENCH: x iterations=a ticks=1 frequency=1",
        ] {
            assert!(parse_console(line).is_err(), "{line}");
        }
    }

    #[test]
    fn baseline_round_trips_through_its_file_format() {
        let original = with_firmware(
            results(&[("aarch64", "a", 12.0), ("x86_64", "b", 3.25)]),
            "riscv64",
            "OpenSBI-v1.3",
        );
        let dir = std::env::temp_dir().join(format!("eggos-bench-test-{}", std::process::id()));
        let path = dir.join("baseline.txt");
        write_baseline(&path, &original).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_dir(&dir).unwrap();
        assert!(text.starts_with("# EggOS boot-time benchmark baseline"));
        assert!(text.ends_with("riscv64 firmware OpenSBI-v1.3\naarch64 a 12.00\nx86_64 b 3.25\n"));
        assert_eq!(parse_baseline(&text).unwrap(), original);
        assert!(parse_baseline("aarch64 a").is_err());
        assert!(parse_baseline("aarch64 a fast").is_err());
    }

    #[test]
    fn one_percent_plus_half_a_nanosecond_passes_and_beyond_fails() {
        let baseline = results(&[("a", "x", 100.0)]);
        let at_the_tolerance = 101.5;
        assert!(
            compare(&baseline, &results(&[("a", "x", at_the_tolerance)]), &["a"])
                .failures
                .is_empty()
        );
        let slower = compare(&baseline, &results(&[("a", "x", 101.6)]), &["a"]);
        assert_eq!(slower.failures.len(), 1);
        assert!(slower.text.contains("REGRESSION"));
    }

    #[test]
    fn boot_to_prompt_is_gated_only_on_the_baseline_s_qemu() {
        let on = |version: &str, value: f64| {
            let mut m = results(&[("x86_64", "boot_to_prompt", value), ("x86_64", "y", 4.0)]);
            m.qemu.insert("x86_64".into(), version.into());
            m
        };
        let baseline = on("8.2.2", 100.0);
        assert_eq!(
            compare(&baseline, &on("8.2.2", 120.0), &["x86_64"])
                .failures
                .len(),
            1
        );
        let other = compare(&baseline, &on("10.2.2", 120.0), &["x86_64"]);
        assert!(other.failures.is_empty(), "{:?}", other.failures);
        assert!(other.text.contains("not gated: QEMU differs"));
        let (merged, kept) = updated_baseline(on("8.2.2", 100.0), on("10.2.2", 120.0), &["x86_64"]);
        assert_eq!(merged, on("8.2.2", 100.0));
        assert_eq!(kept.len(), 1, "{kept:?}");
        let text = {
            let dir = std::env::temp_dir().join(format!("eggos-qemu-test-{}", std::process::id()));
            let path = dir.join("baseline.txt");
            write_baseline(&path, &baseline).unwrap();
            let text = std::fs::read_to_string(&path).unwrap();
            std::fs::remove_file(&path).unwrap();
            std::fs::remove_dir(&dir).unwrap();
            text
        };
        assert!(text.contains("\nx86_64 qemu 8.2.2\n"));
        assert_eq!(parse_baseline(&text).unwrap(), baseline);
    }

    #[test]
    fn reads_qemu_s_version_from_its_first_line() {
        assert_eq!(
            parse_qemu_version("QEMU emulator version 10.2.2\nCopyright (c) 2003-2025\n")
                .as_deref(),
            Some("10.2.2")
        );
        assert_eq!(
            parse_qemu_version("QEMU emulator version 8.2.2 (Debian 1:8.2.2+ds-0ubuntu1.4)\n")
                .as_deref(),
            Some("8.2.2")
        );
        assert_eq!(parse_qemu_version(""), None);
    }

    #[test]
    fn a_size_may_not_grow_by_one_byte() {
        let baseline = results(&[
            ("a", "release_image_bytes", 1000.0),
            ("a", "input_key_to_bytes", 300.0),
        ]);
        let same = compare(&baseline, &baseline, &["a"]);
        assert!(same.failures.is_empty());
        let within_the_time_gate = results(&[
            ("a", "release_image_bytes", 1000.0),
            ("a", "input_key_to_bytes", 301.0),
        ]);
        assert!(
            compare(&baseline, &within_the_time_gate, &["a"])
                .failures
                .is_empty()
        );
        let grown = compare(
            &baseline,
            &results(&[
                ("a", "release_image_bytes", 1001.0),
                ("a", "input_key_to_bytes", 300.0),
            ]),
            &["a"],
        );
        assert_eq!(grown.failures.len(), 1, "{:?}", grown.failures);
        assert!(grown.failures[0].contains("1001.00 bytes"));
        let shrunk = compare(
            &baseline,
            &results(&[
                ("a", "release_image_bytes", 999.0),
                ("a", "input_key_to_bytes", 300.0),
            ]),
            &["a"],
        );
        assert!(shrunk.failures.is_empty());
        assert!(shrunk.text.contains("faster"));
    }

    #[test]
    fn faster_never_fails_but_is_pointed_out_so_the_baseline_can_be_tightened() {
        let baseline = results(&[("a", "x", 100.0)]);
        let faster = compare(&baseline, &results(&[("a", "x", 50.0)]), &["a"]);
        assert!(faster.failures.is_empty());
        assert!(faster.text.contains("faster"));
    }

    #[test]
    fn new_and_vanished_benchmarks_fail_but_other_architectures_are_ignored() {
        let baseline = results(&[("a", "old", 1.0), ("b", "other", 1.0)]);
        let measured = results(&[("a", "new", 1.0)]);
        let report = compare(&baseline, &measured, &["a"]);
        assert_eq!(report.failures.len(), 2, "{:?}", report.failures);
        assert!(report.failures[0].contains("a new: no baseline"));
        assert!(report.failures[1].contains("a old: in the baseline but not measured"));
    }

    #[test]
    fn reads_the_firmware_version_only_where_benchmarks_depend_on_it() {
        let console = "\nOpenSBI v1.3\n   ____  \nEggOS 0.1.0 (riscv64)\n";
        assert_eq!(
            firmware_version("riscv64", console).as_deref(),
            Some("OpenSBI-v1.3")
        );
        assert_eq!(firmware_version("aarch64", console), None);
        assert_eq!(firmware_version("riscv64", "EggOS\n"), None);
    }

    fn timer_arm_and_clock_read_on(
        firmware: &str,
        timer_arm: f64,
        clock_read: f64,
    ) -> Measurements {
        with_firmware(
            results(&[
                ("riscv64", "timer_arm", timer_arm),
                ("riscv64", "clock_read", clock_read),
            ]),
            "riscv64",
            firmware,
        )
    }

    #[test]
    fn a_firmware_dependent_regression_on_the_baselines_firmware_fails() {
        let baseline = timer_arm_and_clock_read_on("OpenSBI-v1.3", 100.0, 4.0);
        let same = timer_arm_and_clock_read_on("OpenSBI-v1.3", 200.0, 4.0);
        assert_eq!(compare(&baseline, &same, &["riscv64"]).failures.len(), 1);
    }

    #[test]
    fn on_other_firmware_its_results_are_reported_not_gated_but_the_others_still_are() {
        let baseline = timer_arm_and_clock_read_on("OpenSBI-v1.3", 100.0, 4.0);
        let other = timer_arm_and_clock_read_on("OpenSBI-v1.7", 200.0, 9.0);
        let report = compare(&baseline, &other, &["riscv64"]);
        assert_eq!(report.failures.len(), 1, "{:?}", report.failures);
        assert!(report.failures[0].contains("clock_read"));
        assert!(
            report
                .text
                .contains("firmware OpenSBI-v1.7, baseline recorded with OpenSBI-v1.3")
        );
        assert!(report.text.contains("not gated"));
    }

    #[test]
    fn no_firmware_version_at_all_fails_instead_of_ungating_silently() {
        let baseline = timer_arm_and_clock_read_on("OpenSBI-v1.3", 100.0, 4.0);
        let unknown = results(&[
            ("riscv64", "timer_arm", 100.0),
            ("riscv64", "clock_read", 4.0),
        ]);
        let report = compare(&baseline, &unknown, &["riscv64"]);
        assert_eq!(report.failures.len(), 1, "{:?}", report.failures);
        assert!(report.failures[0].contains("could not read the firmware version"));
    }

    #[test]
    fn updating_replaces_the_architectures_run_but_keeps_other_firmwares_numbers() {
        let baseline = with_firmware(
            results(&[
                ("riscv64", "timer_arm", 100.0),
                ("riscv64", "clock_read", 4.0),
                ("riscv64", "gone", 1.0),
                ("x86_64", "clock_read", 8.0),
            ]),
            "riscv64",
            "OpenSBI-v1.3",
        );
        let measured = with_firmware(
            results(&[
                ("riscv64", "timer_arm", 200.0),
                ("riscv64", "clock_read", 5.0),
            ]),
            "riscv64",
            "OpenSBI-v1.7",
        );
        let mut measured = measured;
        measured
            .results
            .insert(("riscv64".into(), "timer_interrupt_round_trip".into()), 7.0);
        let (merged, kept) = updated_baseline(baseline, measured, &["riscv64"]);
        let expected = with_firmware(
            results(&[
                ("riscv64", "timer_arm", 100.0),
                ("riscv64", "clock_read", 5.0),
                ("x86_64", "clock_read", 8.0),
            ]),
            "riscv64",
            "OpenSBI-v1.3",
        );
        assert_eq!(merged, expected);
        assert_eq!(kept.len(), 2, "{kept:?}");
        assert!(kept[0].contains("kept riscv64 timer_arm"));
        assert!(
            kept[1].contains("not recorded: riscv64 timer_interrupt_round_trip"),
            "a firmware-dependent result with no baseline is not recorded from other firmware"
        );
    }

    #[test]
    fn updating_on_the_baselines_own_firmware_replaces_everything() {
        let baseline = with_firmware(
            results(&[("riscv64", "timer_arm", 100.0)]),
            "riscv64",
            "OpenSBI-v1.3",
        );
        let same = with_firmware(
            results(&[("riscv64", "timer_arm", 90.0)]),
            "riscv64",
            "OpenSBI-v1.3",
        );
        let (merged, kept) = updated_baseline(baseline, same, &["riscv64"]);
        assert_eq!(merged.results.values().copied().collect::<Vec<_>>(), [90.0]);
        assert!(kept.is_empty());
    }
}
