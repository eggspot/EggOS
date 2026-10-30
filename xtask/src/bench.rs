//! `cargo xtask bench`: the kernel's boot-time benchmarks, compared against a committed
//! baseline (D27).
//!
//! Two modes (D122): the exact gate (the default, what `cargo xtask ci` and the weekly
//! performance pass run), where a result slower than its baseline by more than the tolerance
//! fails; and `--report-only` (pull requests and pushes to `main` in CI), which reports every
//! change against the baseline and fails only a result more than twice as slow.
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

const REPORT_FILES: [&str; 2] = ["report.md", "report.json"];

const BLOCK_FACTOR: f64 = 2.0;

const RELEASE_IMAGE_BYTES: &str = "release_image_bytes";
const TRANSLATE_INSTALL_BYTES: &str = "translate_install_bytes";

/// Results that are sizes, gated exactly: any growth fails.
const SIZES: &[&str] = &[RELEASE_IMAGE_BYTES, TRANSLATE_INSTALL_BYTES];

const TRANSLATE_IMAGE_BASE: u64 = 0x1_4000_0000;
const TRANSLATE_TEXT_RVA: u32 = 0x1000;
const TRANSLATE_FUNCTIONS: u32 = 256;
const TRANSLATE_FUNCTION_STRIDE: u32 = 16;
const TRANSLATE_FUNCTION: [u8; 12] = [
    0x48, 0x01, 0xc8, 0x48, 0x31, 0xd0, 0x48, 0xff, 0xc9, 0x75, 0xf5, 0xc3,
];
const NOP: u8 = 0x90;

/// Header of the baseline file, rewritten by `--update-baseline`.
const BASELINE_HEADER: &str = "\
# EggOS boot-time benchmark baseline, written by `cargo xtask bench --arch all --update-baseline`.
# One line per result: <arch> <benchmark> <ns per iteration>. The kernel runs under QEMU
# `-icount shift=0` (1 ns per instruction), so each number is instructions per iteration.
# `cargo xtask bench` fails when a result is slower than its line here by more than 1% + 0.5 ns;
# CI runs it with `--report-only`, which only reports changes and fails a result more than twice
# as slow. The weekly performance pass settles the differences and updates this file (D122).
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

fn report_directory() -> PathBuf {
    crate::target_directory().join("bench")
}

fn baseline_path() -> PathBuf {
    crate::workspace_root().join("benchmarks/baseline.txt")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Gate,
    ReportOnly,
    UpdateBaseline,
}

/// Builds and boots the benchmark kernel for each of `arches`, then compares the results with
/// the baseline, or writes them into it (keeping other architectures' lines) in
/// `UpdateBaseline` mode. A comparison also writes `target/bench/report.md` and `report.json`.
pub fn run(arches: &[&Arch], mode: Mode) -> Result<(), String> {
    for file in REPORT_FILES {
        let _ = std::fs::remove_file(report_directory().join(file));
    }
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
    if mode == Mode::UpdateBaseline {
        let (merged, kept) = updated_baseline(baseline, measured, &ran);
        for line in kept {
            eprintln!("xtask: {line}");
        }
        write_baseline(&path, &merged)?;
        eprintln!("xtask: wrote {}", path.display());
        return Ok(());
    }

    let report = compare(&baseline, &measured, &ran, mode);
    eprint!("{}", report.text);
    write_report(
        &crate::target_directory().join("bench"),
        &report,
        mode,
        &ran,
    )?;
    if report.failures.is_empty() {
        return Ok(());
    }
    let advice = match mode {
        Mode::ReportOnly => format!(
            "a result more than {BLOCK_FACTOR}x slower than its baseline blocks a pull request"
        ),
        _ => "if the change is intended, run `cargo xtask bench --arch all --update-baseline` \
              and justify it in the PR"
            .to_owned(),
    };
    Err(format!(
        "benchmarks failed against {}:\n  {}\n({advice})",
        path.display(),
        report.failures.join("\n  ")
    ))
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
    if arch.name == "aarch64" {
        results.push((
            TRANSLATE_INSTALL_BYTES.to_owned(),
            translate_install_bytes()?,
        ));
    }
    Ok(results)
}

fn translate_bench_image() -> Vec<u8> {
    let mut code = vec![NOP; (TRANSLATE_FUNCTIONS * TRANSLATE_FUNCTION_STRIDE) as usize];
    for function in code.chunks_mut(TRANSLATE_FUNCTION_STRIDE as usize) {
        function[..TRANSLATE_FUNCTION.len()].copy_from_slice(&TRANSLATE_FUNCTION);
    }
    code
}

fn translate_install_bytes() -> Result<f64, String> {
    use eggos_translate::cache::{self, CacheFile};
    use eggos_translate::codegen::CodegenMode;
    use eggos_translate::image::{ImageInput, Section, Seed, SeedOrigin, translate_image};
    let code = translate_bench_image();
    let sections = [Section {
        rva: TRANSLATE_TEXT_RVA,
        bytes: &code,
        executable: true,
    }];
    let seeds: Vec<Seed> = (0..TRANSLATE_FUNCTIONS)
        .map(|index| Seed {
            rva: TRANSLATE_TEXT_RVA + index * TRANSLATE_FUNCTION_STRIDE,
            origin: SeedOrigin::Export,
        })
        .collect();
    let input = ImageInput {
        image_base: TRANSLATE_IMAGE_BASE,
        sections: &sections,
        seeds: &seeds,
    };
    let mode = CodegenMode {
        lse: true,
        ..CodegenMode::default()
    };
    let image = translate_image(&input, mode)
        .map_err(|error| format!("could not translate the benchmark image: {error:?}"))?;
    let guest_bytes = image.stats.guest_bytes;
    if guest_bytes == 0 {
        return Err("the benchmark image translated no code".to_owned());
    }
    let file = CacheFile {
        input_digest: cache::input_digest(&code),
        mode,
        image_base: TRANSLATE_IMAGE_BASE,
        image,
    };
    Ok(cache::serialize(&file).len() as f64 / guest_bytes as f64)
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Status {
    Same,
    Faster,
    Slower,
    Regression,
    Blocked,
    New,
    NotGated,
}

impl Status {
    fn label(self) -> &'static str {
        match self {
            Status::Same => "same",
            Status::Faster => "faster",
            Status::Slower => "slower",
            Status::Regression => "regression",
            Status::Blocked => "blocked",
            Status::New => "new",
            Status::NotGated => "not gated",
        }
    }
}

#[derive(Debug)]
struct Row {
    arch: String,
    name: String,
    unit: &'static str,
    now: f64,
    before: Option<f64>,
    change_percent: Option<f64>,
    status: Status,
}

#[derive(Debug, Default)]
struct Comparison {
    /// A table of every result against its baseline.
    text: String,
    /// One line per regression, new benchmark without a baseline, or baseline without a result.
    failures: Vec<String>,
    rows: Vec<Row>,
    notes: Vec<String>,
}

/// Compares `measured` with `baseline` for the architectures in `ran`.
fn compare(
    baseline: &Measurements,
    measured: &Measurements,
    ran: &[&str],
    mode: Mode,
) -> Comparison {
    let report_only = mode == Mode::ReportOnly;
    let mut report = Comparison::default();
    for arch in ran {
        let (now, then) = (measured.firmware.get(*arch), baseline.firmware.get(*arch));
        let unreadable_firmware_would_ungate_its_results =
            now.is_none() && FIRMWARE_DEPENDENT.iter().any(|(a, _, _)| a == arch);
        if unreadable_firmware_would_ungate_its_results {
            let note = format!("{arch}: could not read the firmware version from the console");
            if report_only {
                report.notes.push(note);
            } else {
                report.failures.push(note);
            }
        }
        if now != then {
            let name = |v: Option<&String>| v.map_or("no firmware".to_owned(), String::clone);
            let note = format!(
                "{arch}: firmware {}, baseline recorded with {}",
                name(now),
                name(then)
            );
            let _ = writeln!(report.text, "  {note}");
            report.notes.push(note);
        }
        let (now, then) = (measured.qemu.get(*arch), baseline.qemu.get(*arch));
        if now != then {
            let name = |v: Option<&String>| v.map_or("unknown".to_owned(), String::clone);
            let note = format!(
                "{arch}: QEMU {}, baseline recorded with QEMU {}",
                name(now),
                name(then)
            );
            let _ = writeln!(report.text, "  {note}");
            report.notes.push(note);
        }
    }
    for ((arch, name), &now) in &measured.results {
        let key = (arch.clone(), name.clone());
        let exact = SIZES.contains(&name.as_str());
        let unit = if exact { "bytes" } else { "ns" };
        let mut row = Row {
            arch: arch.clone(),
            name: name.clone(),
            unit,
            now,
            before: None,
            change_percent: None,
            status: Status::New,
        };
        let Some(&before) = baseline.results.get(&key) else {
            let _ = writeln!(
                report.text,
                "  {arch} {name}: {now:.2} {unit} (no baseline)"
            );
            if !report_only {
                report
                    .failures
                    .push(format!("{arch} {name}: no baseline (new benchmark?)"));
            }
            report.rows.push(row);
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
        let block_limit = before * BLOCK_FACTOR + TOLERANCE_ABSOLUTE_NS;
        let other_firmware = is_firmware_dependent(arch, name)
            && measured.firmware.get(arch) != baseline.firmware.get(arch);
        let other_qemu = QEMU_DEPENDENT.contains(&name.as_str())
            && measured.qemu.get(arch) != baseline.qemu.get(arch);
        let (status, note) = if other_firmware {
            (
                Status::NotGated,
                "  not gated: firmware differs from the baseline's",
            )
        } else if other_qemu {
            (
                Status::NotGated,
                "  not gated: QEMU differs from the baseline's",
            )
        } else if report_only && now > block_limit {
            report.failures.push(format!(
                "{arch} {name}: {now:.2} {unit}, baseline {before:.2} {unit} ({change:+.1}%), \
                 more than {BLOCK_FACTOR}x slower"
            ));
            (Status::Blocked, "  BLOCKED: more than 2x slower")
        } else if now > limit && report_only {
            (Status::Slower, "  slower")
        } else if now > limit {
            report.failures.push(format!(
                "{arch} {name}: {now:.2} {unit}, baseline {before:.2} {unit} ({change:+.1}%)"
            ));
            (Status::Regression, "  REGRESSION")
        } else if now < faster_than {
            (Status::Faster, "  faster: consider --update-baseline")
        } else {
            (Status::Same, "")
        };
        row.before = Some(before);
        row.change_percent = Some(change);
        row.status = status;
        report.rows.push(row);
        let _ = writeln!(
            report.text,
            "  {arch} {name}: {now:.2} {unit}, baseline {before:.2} {unit} ({change:+.1}%){note}"
        );
    }
    for (arch, name) in baseline.results.keys() {
        if ran.contains(&arch.as_str())
            && !measured.results.contains_key(&(arch.clone(), name.clone()))
        {
            let note =
                format!("{arch} {name}: in the baseline but not measured (benchmark removed?)");
            if report_only {
                report.notes.push(note);
            } else {
                report.failures.push(note);
            }
        }
    }
    report
}

fn write_report(dir: &Path, report: &Comparison, mode: Mode, ran: &[&str]) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let [markdown, json] = REPORT_FILES;
    for (file, text) in [
        (markdown, report_markdown(report, mode, ran)),
        (json, report_json(report, mode, ran)),
    ] {
        let path = dir.join(file);
        std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(())
}

fn count(report: &Comparison, status: Status) -> usize {
    report
        .rows
        .iter()
        .filter(|row| row.status == status)
        .count()
}

fn describe_row(row: &Row) -> String {
    let before = row
        .before
        .map_or("-".to_owned(), |before| format!("{before:.2}"));
    let change = row
        .change_percent
        .map_or("-".to_owned(), |change| format!("{change:+.1}%"));
    format!(
        "| {} | {} | {:.2} {} | {} | {} | {} |\n",
        row.arch,
        row.name,
        row.now,
        row.unit,
        before,
        change,
        row.status.label()
    )
}

const TABLE_HEAD: &str = "| arch | benchmark | now | baseline | change | status |\n\
                          |---|---|---:|---:|---:|---|\n";

fn report_markdown(report: &Comparison, mode: Mode, ran: &[&str]) -> String {
    let mut text = String::new();
    let how = match mode {
        Mode::ReportOnly => format!(
            "report only: a result more than {BLOCK_FACTOR}x slower than its baseline fails the job"
        ),
        _ => "exact gate".to_owned(),
    };
    let _ = writeln!(
        text,
        "### Benchmarks against the baseline ({}): {how}\n",
        ran.join(", ")
    );
    let _ = writeln!(
        text,
        "{} slower, {} faster, {} unchanged, {} new, {} not gated, {} blocked.\n",
        count(report, Status::Slower) + count(report, Status::Regression),
        count(report, Status::Faster),
        count(report, Status::Same),
        count(report, Status::New),
        count(report, Status::NotGated),
        count(report, Status::Blocked),
    );
    let moved: Vec<&Row> = report
        .rows
        .iter()
        .filter(|row| row.status != Status::Same)
        .collect();
    if !moved.is_empty() {
        text.push_str(TABLE_HEAD);
        for row in moved {
            text.push_str(&describe_row(row));
        }
        text.push('\n');
    }
    for note in &report.notes {
        let _ = writeln!(text, "- {note}");
    }
    if !report.notes.is_empty() {
        text.push('\n');
    }
    let _ = writeln!(
        text,
        "<details><summary>All {} results</summary>\n",
        report.rows.len()
    );
    text.push_str(TABLE_HEAD);
    for row in &report.rows {
        text.push_str(&describe_row(row));
    }
    text.push_str("\n</details>\n");
    text
}

fn json_string(value: &str) -> String {
    let mut out = String::from("\"");
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn json_number(value: Option<f64>) -> String {
    value.map_or("null".to_owned(), |value| format!("{value:.2}"))
}

fn report_json(report: &Comparison, mode: Mode, ran: &[&str]) -> String {
    let strings = |items: &[String]| {
        items
            .iter()
            .map(|item| json_string(item))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut text = String::from("{\n");
    let mode_name = match mode {
        Mode::ReportOnly => "report-only",
        _ => "gate",
    };
    let _ = writeln!(text, "  \"mode\": {},", json_string(mode_name));
    let _ = writeln!(text, "  \"block_factor\": {BLOCK_FACTOR:.1},");
    let arches: Vec<String> = ran.iter().map(|arch| (*arch).to_owned()).collect();
    let _ = writeln!(text, "  \"architectures\": [{}],", strings(&arches));
    let _ = writeln!(text, "  \"failures\": [{}],", strings(&report.failures));
    let _ = writeln!(text, "  \"notes\": [{}],", strings(&report.notes));
    text.push_str("  \"results\": [\n");
    let last = report.rows.len().saturating_sub(1);
    for (index, row) in report.rows.iter().enumerate() {
        let _ = writeln!(
            text,
            "    {{\"arch\": {}, \"benchmark\": {}, \"unit\": {}, \"value\": {:.2}, \"baseline\": {}, \"change_percent\": {}, \"status\": {}}}{}",
            json_string(&row.arch),
            json_string(&row.name),
            json_string(row.unit),
            row.now,
            json_number(row.before),
            json_number(row.change_percent),
            json_string(row.status.label()),
            if index == last { "" } else { "," }
        );
    }
    text.push_str("  ]\n}\n");
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_translation_cache_per_code_byte_is_a_finite_ratio_above_one() {
        let ratio = translate_install_bytes().unwrap();
        assert!(ratio.is_finite() && ratio > 1.0, "{ratio}");
        assert_eq!(translate_bench_image().len(), 4096);
    }

    fn compare(baseline: &Measurements, measured: &Measurements, ran: &[&str]) -> Comparison {
        super::compare(baseline, measured, ran, Mode::Gate)
    }

    fn compare_report_only(
        baseline: &Measurements,
        measured: &Measurements,
        ran: &[&str],
    ) -> Comparison {
        super::compare(baseline, measured, ran, Mode::ReportOnly)
    }

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

    #[test]
    fn report_only_reports_a_slowdown_without_failing_it() {
        let baseline = results(&[
            ("a", "x", 100.0),
            ("a", "release_image_bytes", 1000.0),
            ("a", "y", 100.0),
        ]);
        let measured = results(&[
            ("a", "x", 199.0),
            ("a", "release_image_bytes", 1900.0),
            ("a", "y", 100.0),
        ]);
        let report = compare_report_only(&baseline, &measured, &["a"]);
        assert!(report.failures.is_empty(), "{:?}", report.failures);
        assert_eq!(count(&report, Status::Slower), 2);
        assert_eq!(count(&report, Status::Same), 1);
        assert_eq!(compare(&baseline, &measured, &["a"]).failures.len(), 2);
    }

    #[test]
    fn report_only_fails_a_result_more_than_twice_as_slow() {
        let baseline = results(&[("a", "x", 100.0), ("a", "release_image_bytes", 1000.0)]);
        let at_twice = results(&[("a", "x", 200.5), ("a", "release_image_bytes", 2000.0)]);
        assert!(
            compare_report_only(&baseline, &at_twice, &["a"])
                .failures
                .is_empty()
        );
        let beyond = results(&[("a", "x", 200.6), ("a", "release_image_bytes", 2000.0)]);
        let report = compare_report_only(&baseline, &beyond, &["a"]);
        assert_eq!(report.failures.len(), 1, "{:?}", report.failures);
        assert!(report.failures[0].contains("a x: 200.60 ns"));
        assert!(report.failures[0].contains("more than 2x slower"));
        assert_eq!(count(&report, Status::Blocked), 1);
        assert_eq!(count(&report, Status::Slower), 1);
        assert!(report.text.contains("BLOCKED"));
    }

    #[test]
    fn report_only_does_not_block_a_few_instructions_on_a_tiny_baseline() {
        let baseline = results(&[("a", "empty_loop", 4.0)]);
        let measured = results(&[("a", "empty_loop", 8.5)]);
        assert!(
            compare_report_only(&baseline, &measured, &["a"])
                .failures
                .is_empty()
        );
        let more = results(&[("a", "empty_loop", 8.6)]);
        assert_eq!(
            compare_report_only(&baseline, &more, &["a"]).failures.len(),
            1
        );
    }

    #[test]
    fn report_only_notes_new_and_vanished_benchmarks_without_failing() {
        let baseline = results(&[("a", "old", 1.0)]);
        let measured = results(&[("a", "new", 1.0)]);
        let report = compare_report_only(&baseline, &measured, &["a"]);
        assert!(report.failures.is_empty(), "{:?}", report.failures);
        assert_eq!(count(&report, Status::New), 1);
        assert_eq!(report.notes.len(), 1);
        assert!(report.notes[0].contains("a old: in the baseline but not measured"));
    }

    #[test]
    fn report_only_still_does_not_gate_results_that_depend_on_another_qemu() {
        let on = |version: &str, value: f64| {
            let mut m = results(&[("x86_64", "boot_to_prompt", value)]);
            m.qemu.insert("x86_64".into(), version.into());
            m
        };
        let report = compare_report_only(&on("8.2.2", 100.0), &on("10.2.2", 500.0), &["x86_64"]);
        assert!(report.failures.is_empty());
        assert_eq!(count(&report, Status::NotGated), 1);
    }

    #[test]
    fn report_only_notes_an_unreadable_firmware_version_instead_of_failing() {
        let baseline = timer_arm_and_clock_read_on("OpenSBI-v1.3", 100.0, 4.0);
        let unknown = results(&[
            ("riscv64", "timer_arm", 100.0),
            ("riscv64", "clock_read", 4.0),
        ]);
        let report = compare_report_only(&baseline, &unknown, &["riscv64"]);
        assert!(report.failures.is_empty(), "{:?}", report.failures);
        assert!(
            report
                .notes
                .iter()
                .any(|note| note.contains("could not read the firmware version"))
        );
    }

    #[test]
    fn the_markdown_report_lists_what_moved_and_every_result() {
        let baseline = results(&[("a", "x", 100.0), ("a", "same", 10.0), ("a", "fast", 100.0)]);
        let measured = results(&[("a", "x", 150.0), ("a", "same", 10.0), ("a", "fast", 50.0)]);
        let report = compare_report_only(&baseline, &measured, &["a"]);
        let text = report_markdown(&report, Mode::ReportOnly, &["a"]);
        assert!(text.contains("report only"));
        assert!(text.contains("1 slower, 1 faster, 1 unchanged, 0 new, 0 not gated, 0 blocked."));
        assert!(text.contains("| a | x | 150.00 ns | 100.00 | +50.0% | slower |"));
        assert!(text.contains("<summary>All 3 results</summary>"));
        let moved_table = text.split("<details>").next().unwrap();
        assert!(!moved_table.contains("| same |"));
    }

    #[test]
    fn the_json_report_is_machine_readable_and_escapes_strings() {
        let baseline = results(&[("a", "x", 100.0)]);
        let measured = results(&[("a", "x", 250.0), ("a", "new", 3.0)]);
        let report = compare_report_only(&baseline, &measured, &["a"]);
        let text = report_json(&report, Mode::ReportOnly, &["a"]);
        assert!(text.contains("\"mode\": \"report-only\""));
        assert!(text.contains("\"block_factor\": 2.0"));
        assert!(text.contains(
            "{\"arch\": \"a\", \"benchmark\": \"x\", \"unit\": \"ns\", \"value\": 250.00, \"baseline\": 100.00, \"change_percent\": 150.00, \"status\": \"blocked\"}"
        ));
        assert!(text.contains("\"baseline\": null, \"change_percent\": null, \"status\": \"new\""));
        assert_eq!(json_string("a\"b\\c\n"), "\"a\\\"b\\\\c\\n\"");
    }
}
