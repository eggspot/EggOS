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
pub const ICOUNT_ARGS: &[&str] = &["-icount", "shift=0,sleep=off"];

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
";

/// Results in nanoseconds (= instructions, under icount) per iteration, keyed by
/// (architecture, benchmark).
type Results = BTreeMap<(String, String), f64>;

fn baseline_path() -> PathBuf {
    crate::workspace_root().join("benchmarks/baseline.txt")
}

/// Builds and boots the benchmark kernel for each of `arches`, then compares the results with
/// the baseline, or with `update` writes them into it (keeping other architectures' lines).
pub fn run(arches: &[&Arch], update: bool) -> Result<(), String> {
    let mut measured = Results::new();
    for arch in arches {
        let kernel = crate::build_kernel(arch, Some("qemu-bench"))?;
        let console = qemu::boot_test(arch, &kernel, qemu::Expect::Pass, ICOUNT_ARGS)?;
        let results = parse_console(&console)?;
        if results.is_empty() {
            return Err(format!(
                "{}: the kernel printed no benchmark results",
                arch.name
            ));
        }
        for (name, ns_per_iteration) in results {
            measured.insert((arch.name.to_owned(), name), ns_per_iteration);
        }
    }

    let path = baseline_path();
    let baseline = match std::fs::read_to_string(&path) {
        Ok(text) => parse_baseline(&text)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Results::new(),
        Err(e) => return Err(format!("could not read {}: {e}", path.display())),
    };

    let ran: Vec<&str> = arches.iter().map(|a| a.name).collect();
    if update {
        let mut merged: Results = baseline
            .into_iter()
            .filter(|((arch, _), _)| !ran.contains(&arch.as_str()))
            .collect();
        merged.extend(measured);
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

fn parse_baseline(text: &str) -> Result<Results, String> {
    let mut results = Results::new();
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let words: Vec<&str> = line.split_whitespace().collect();
        let [arch, name, value] = words[..] else {
            return Err(format!("malformed baseline line `{line}`"));
        };
        let value: f64 = value
            .parse()
            .map_err(|_| format!("malformed baseline line `{line}`"))?;
        results.insert((arch.to_owned(), name.to_owned()), value);
    }
    Ok(results)
}

fn write_baseline(path: &Path, results: &Results) -> Result<(), String> {
    let mut text = BASELINE_HEADER.to_owned();
    for ((arch, name), value) in results {
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
fn compare(baseline: &Results, measured: &Results, ran: &[&str]) -> Comparison {
    let mut report = Comparison::default();
    for ((arch, name), &now) in measured {
        let key = (arch.clone(), name.clone());
        let Some(&before) = baseline.get(&key) else {
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
        let note = if now > limit {
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
    for (arch, name) in baseline.keys() {
        if ran.contains(&arch.as_str()) && !measured.contains_key(&(arch.clone(), name.clone())) {
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

    fn results(entries: &[(&str, &str, f64)]) -> Results {
        entries
            .iter()
            .map(|&(a, n, v)| ((a.to_owned(), n.to_owned()), v))
            .collect()
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
        let original = results(&[("aarch64", "a", 12.0), ("x86_64", "b", 3.25)]);
        let dir = std::env::temp_dir().join(format!("eggos-bench-test-{}", std::process::id()));
        let path = dir.join("baseline.txt");
        write_baseline(&path, &original).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_dir(&dir).unwrap();
        assert!(text.starts_with("# EggOS boot-time benchmark baseline"));
        assert!(text.ends_with("aarch64 a 12.00\nx86_64 b 3.25\n"));
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
}
