//! `cargo xtask bench`: the kernel's boot-time benchmarks, compared against a committed
//! baseline (D27).
//!
//! The kernel (feature `qemu-bench`) prints one `EGGOS-BENCH:` line per benchmark. QEMU runs
//! with `-icount shift=0`, so the guest clock advances exactly 1 ns per instruction and every
//! result is an instruction count per iteration: deterministic, independent of how fast or
//! busy the host is, and therefore comparable with a committed number. A result above its
//! baseline by more than the tolerance is a regression and fails the run.

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

/// Header of the baseline file, rewritten by `--update-baseline`.
const BASELINE_HEADER: &str = "\
# EggOS boot-time benchmark baseline, written by `cargo xtask bench --arch all --update-baseline`.
# One line per result: <arch> <benchmark> <ns per iteration>. The kernel runs under QEMU
# `-icount shift=0` (1 ns per instruction), so each number is instructions per iteration.
# `cargo xtask bench` (and CI) fails when a result is slower than its line here by more than
# 1% + 0.5 ns. Update it in the PR that makes a change faster, or that the owner accepted as
# slower, and say why in the PR (CLAUDE.md, \"Never merge a performance regression\").
# `<arch> firmware <version>` names the firmware the results were recorded with: benchmarks
# that run inside it are gated only on the same firmware. Record the baseline with CI's QEMU
# (Ubuntu 24.04 packages).
";

/// Benchmarks that execute firmware code (the SBI implementation on RISC-V), so their counts
/// depend on the firmware build QEMU loads: (architecture, firmware banner prefix,
/// benchmarks). Those are compared only when the firmware matches the baseline's.
const FIRMWARE_DEPENDENT: &[(&str, &str, &[&str])] = &[(
    "riscv64",
    "OpenSBI v",
    &["timer_arm", "timer_interrupt_round_trip"],
)];

/// Results in nanoseconds (= instructions, under icount) per iteration, keyed by
/// (architecture, benchmark).
type Results = BTreeMap<(String, String), f64>;

/// A set of results and the firmware version each architecture ran with (when it matters).
#[derive(Debug, Default, PartialEq)]
struct Measurements {
    results: Results,
    firmware: BTreeMap<String, String>,
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
        let console = qemu::boot_test(arch, &kernel, qemu::Expect::Pass, BENCH_CPUS, ICOUNT_ARGS)?;
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
    let mut kept = Vec::new();
    let mut merged = Measurements::default();
    for ((arch, name), value) in &baseline.results {
        let keep = !ran.contains(&arch.as_str())
            || (is_firmware_dependent(arch, name) && other_firmware(arch));
        if keep {
            if ran.contains(&arch.as_str()) {
                kept.push(format!(
                    "kept {arch} {name} {value:.2}: measured on other firmware than the \
                     baseline's (record it with CI's QEMU)"
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
    for ((arch, name), value) in measured.results {
        if is_firmware_dependent(&arch, &name) && other_firmware(&arch) {
            if !merged.results.contains_key(&(arch.clone(), name.clone())) {
                kept.push(format!(
                    "not recorded: {arch} {name} {value:.2} was measured on other firmware \
                     than the baseline's (record it with CI's QEMU)"
                ));
            }
            continue;
        }
        merged.results.insert((arch, name), value);
    }
    for (arch, version) in measured.firmware {
        merged.firmware.entry(arch).or_insert(version);
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
        // Without a version, a changed banner would silently ungate the firmware's results.
        if now.is_none() && FIRMWARE_DEPENDENT.iter().any(|(a, _, _)| a == arch) {
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
    }
    for ((arch, name), &now) in &measured.results {
        let key = (arch.clone(), name.clone());
        let Some(&before) = baseline.results.get(&key) else {
            let _ = writeln!(report.text, "  {arch} {name}: {now:.2} ns (no baseline)");
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
        let limit = before * (1.0 + TOLERANCE_RELATIVE) + TOLERANCE_ABSOLUTE_NS;
        let other_firmware = is_firmware_dependent(arch, name)
            && measured.firmware.get(arch) != baseline.firmware.get(arch);
        let note = if other_firmware {
            "  not gated: firmware differs from the baseline's"
        } else if now > limit {
            report.failures.push(format!(
                "{arch} {name}: {now:.2} ns, baseline {before:.2} ns ({change:+.1}%)"
            ));
            "  REGRESSION"
        } else if now < before * (1.0 - TOLERANCE_RELATIVE) - TOLERANCE_ABSOLUTE_NS {
            "  faster: consider --update-baseline"
        } else {
            ""
        };
        let _ = writeln!(
            report.text,
            "  {arch} {name}: {now:.2} ns, baseline {before:.2} ns ({change:+.1}%){note}"
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
        // 62 500 ticks at 62.5 MHz = 1 ms, over 10 000 iterations = 100 ns each.
        assert!((parsed[0].1 - 100.0).abs() < 1e-9);
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
    fn within_tolerance_passes_and_beyond_fails() {
        let baseline = results(&[("a", "x", 100.0)]);
        // 1% + 0.5 ns of 100 ns: 101.5 passes, 101.6 fails.
        assert!(
            compare(&baseline, &results(&[("a", "x", 101.5)]), &["a"])
                .failures
                .is_empty()
        );
        let slower = compare(&baseline, &results(&[("a", "x", 101.6)]), &["a"]);
        assert_eq!(slower.failures.len(), 1);
        assert!(slower.text.contains("REGRESSION"));
        // Faster never fails, but is pointed out so the baseline can be tightened.
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

    #[test]
    fn firmware_dependent_results_are_gated_only_on_the_baselines_firmware() {
        let baseline = with_firmware(
            results(&[
                ("riscv64", "timer_arm", 100.0),
                ("riscv64", "clock_read", 4.0),
            ]),
            "riscv64",
            "OpenSBI-v1.3",
        );
        let slower = results(&[
            ("riscv64", "timer_arm", 200.0),
            ("riscv64", "clock_read", 4.0),
        ]);
        // Same firmware: a regression.
        let same = with_firmware(slower, "riscv64", "OpenSBI-v1.3");
        assert_eq!(compare(&baseline, &same, &["riscv64"]).failures.len(), 1);
        // Other firmware: reported, not gated; benchmarks that do not enter it still are.
        let other = with_firmware(
            results(&[
                ("riscv64", "timer_arm", 200.0),
                ("riscv64", "clock_read", 9.0),
            ]),
            "riscv64",
            "OpenSBI-v1.7",
        );
        let report = compare(&baseline, &other, &["riscv64"]);
        assert_eq!(report.failures.len(), 1, "{:?}", report.failures);
        assert!(report.failures[0].contains("clock_read"));
        assert!(
            report
                .text
                .contains("firmware OpenSBI-v1.7, baseline recorded with OpenSBI-v1.3")
        );
        assert!(report.text.contains("not gated"));
        // No firmware version at all (a changed banner) fails instead of ungating silently.
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
        // A firmware-dependent result with no baseline yet is not recorded from other firmware.
        assert_eq!(kept.len(), 2, "{kept:?}");
        assert!(kept[0].contains("kept riscv64 timer_arm"));
        assert!(kept[1].contains("not recorded: riscv64 timer_interrupt_round_trip"));

        // On the baseline's own firmware everything is replaced.
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
