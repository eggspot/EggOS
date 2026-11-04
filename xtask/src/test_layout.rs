//! The integration-test layout rule that `cargo xtask lint` enforces (CLAUDE.md, "Rules"; D217):
//! a package keeps all its integration tests in one test program, `tests/all/main.rs` with its
//! modules under `tests/all/`, built by a single `[[test]] name = "all"` with
//! `autotests = false`, so the build folder grows with the number of packages, not of test files.

use std::path::Path;

const TEST_PROGRAM: &str = "all";
const TEST_PROGRAM_PATH: &str = "tests/all/main.rs";

#[derive(Debug, PartialEq, Eq)]
pub enum TestsEntry {
    File(String),
    Directory { name: String, has_main: bool },
}

#[derive(Debug, Default, PartialEq, Eq)]
struct TestTarget {
    name: Option<String>,
    path: Option<String>,
}

#[derive(Debug, Default)]
struct Manifest {
    is_package: bool,
    autotests_off: bool,
    test_targets: Vec<TestTarget>,
}

pub fn check<'a>(root: &Path, manifests: impl Iterator<Item = &'a str>) -> Result<usize, String> {
    let mut problems = Vec::new();
    let mut packages = 0;
    for manifest_path in manifests {
        let Some(text) = crate::lint::read_if_present(&root.join(manifest_path))? else {
            continue;
        };
        let package_dir = manifest_path
            .strip_suffix("Cargo.toml")
            .unwrap_or(manifest_path)
            .trim_end_matches('/');
        let entries = tests_entries(&root.join(package_dir).join("tests"))?;
        packages += usize::from(parse(&text).is_package);
        problems.extend(package_problems(package_dir, &text, entries.as_deref()));
    }
    if problems.is_empty() {
        return Ok(packages);
    }
    Err(format!(
        "{}\n(CLAUDE.md: integration tests go in the package's single `tests/all` program, \
         `[[test]] name = \"{TEST_PROGRAM}\"` and `path = \"{TEST_PROGRAM_PATH}\"` with \
         `autotests = false`; never add a top-level `tests/*.rs` file)",
        problems.join("\n")
    ))
}

fn tests_entries(tests: &Path) -> Result<Option<Vec<TestsEntry>>, String> {
    let read = match std::fs::read_dir(tests) {
        Ok(read) => read,
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) =>
        {
            return Ok(None);
        }
        Err(e) => return Err(format!("{}: {e}", tests.display())),
    };
    let mut entries = Vec::new();
    for entry in read {
        let entry = entry.map_err(|e| format!("{}: {e}", tests.display()))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        entries.push(if path.is_dir() {
            TestsEntry::Directory {
                has_main: path.join("main.rs").is_file(),
                name,
            }
        } else {
            TestsEntry::File(name)
        });
    }
    entries.sort_by(|a, b| entry_name(a).cmp(entry_name(b)));
    Ok(Some(entries))
}

fn entry_name(entry: &TestsEntry) -> &str {
    match entry {
        TestsEntry::File(name) | TestsEntry::Directory { name, .. } => name,
    }
}

pub fn package_problems(
    package_dir: &str,
    manifest_text: &str,
    tests_entries: Option<&[TestsEntry]>,
) -> Vec<String> {
    let manifest = parse(manifest_text);
    if !manifest.is_package {
        return Vec::new();
    }
    let at = |rest: &str| {
        if package_dir.is_empty() {
            rest.to_owned()
        } else {
            format!("{package_dir}/{rest}")
        }
    };
    let mut problems = Vec::new();
    match &manifest.test_targets[..] {
        [] => {}
        [target] => {
            if target.name.as_deref() != Some(TEST_PROGRAM) {
                problems.push(format!(
                    "{}: its [[test]] target must be named `{TEST_PROGRAM}`",
                    at("Cargo.toml")
                ));
            }
            if target
                .path
                .as_deref()
                .is_some_and(|path| path != TEST_PROGRAM_PATH)
            {
                problems.push(format!(
                    "{}: its [[test]] target's path must be `{TEST_PROGRAM_PATH}`",
                    at("Cargo.toml")
                ));
            }
        }
        targets => problems.push(format!(
            "{}: {} [[test]] targets; keep one, `{TEST_PROGRAM}`",
            at("Cargo.toml"),
            targets.len()
        )),
    }
    let Some(entries) = tests_entries else {
        return problems;
    };
    if !manifest.autotests_off {
        problems.push(format!(
            "{}: has a tests/ folder but no `autotests = false` in [package]",
            at("Cargo.toml")
        ));
    }
    for entry in entries {
        match entry {
            TestsEntry::File(name) if name.ends_with(".rs") => problems.push(format!(
                "{}: a separate test program; move it into tests/{TEST_PROGRAM}/ as a module",
                at(&format!("tests/{name}"))
            )),
            TestsEntry::Directory {
                name,
                has_main: true,
            } if name != TEST_PROGRAM => problems.push(format!(
                "{}: a separate test program; move it into tests/{TEST_PROGRAM}/ as a module",
                at(&format!("tests/{name}/main.rs"))
            )),
            TestsEntry::Directory {
                name,
                has_main: true,
            } if manifest.test_targets.is_empty() => problems.push(format!(
                "{}: never built; add [[test]] name = \"{TEST_PROGRAM}\" to {}",
                at(&format!("tests/{name}/main.rs")),
                at("Cargo.toml")
            )),
            _ => {}
        }
    }
    problems
}

fn parse(text: &str) -> Manifest {
    let mut manifest = Manifest::default();
    let mut table = "";
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            table = line;
            if line == "[package]" {
                manifest.is_package = true;
            } else if line == "[[test]]" {
                manifest.test_targets.push(TestTarget::default());
            }
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let (key, value) = (key.trim(), value.trim());
        match table {
            "[package]" if key == "autotests" => manifest.autotests_off = value == "false",
            "[[test]]" => {
                let Some(target) = manifest.test_targets.last_mut() else {
                    continue;
                };
                let value = Some(value.trim_matches('"').to_owned());
                match key {
                    "name" => target.name = value,
                    "path" => target.path = value,
                    _ => {}
                }
            }
            _ => {}
        }
    }
    manifest
}

#[cfg(test)]
mod tests {
    use super::*;

    const PACKAGE: &str = "[package]\nname = \"eggos-font\"\nedition.workspace = true\n";
    const SINGLE_PROGRAM: &str = "[package]\nname = \"eggos-font\"\nautotests = false\n\n\
        [[test]]\nname = \"all\"\npath = \"tests/all/main.rs\"\n";

    fn file(name: &str) -> TestsEntry {
        TestsEntry::File(name.to_owned())
    }

    fn directory(name: &str, has_main: bool) -> TestsEntry {
        TestsEntry::Directory {
            name: name.to_owned(),
            has_main,
        }
    }

    #[test]
    fn a_package_without_a_tests_folder_passes() {
        assert!(package_problems("font", PACKAGE, None).is_empty());
    }

    #[test]
    fn the_single_all_program_with_its_modules_and_fixtures_passes() {
        let entries = [directory("all", true), directory("fixtures", false)];
        assert!(package_problems("font", SINGLE_PROGRAM, Some(&entries)).is_empty());
    }

    #[test]
    fn a_stray_top_level_test_file_is_flagged() {
        let entries = [directory("all", true), file("x.rs"), file("data.bin")];
        let problems = package_problems("font", SINGLE_PROGRAM, Some(&entries));
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].starts_with("font/tests/x.rs: "), "{problems:?}");
    }

    #[test]
    fn an_all_rs_file_is_flagged_so_the_layout_stays_uniform() {
        let entries = [file("all.rs")];
        let problems = package_problems("font", SINGLE_PROGRAM, Some(&entries));
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].starts_with("font/tests/all.rs: "), "{problems:?}");
    }

    #[test]
    fn a_second_test_program_folder_is_flagged() {
        let entries = [directory("all", true), directory("other", true)];
        let problems = package_problems("font", SINGLE_PROGRAM, Some(&entries));
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].starts_with("font/tests/other/main.rs: "));
    }

    #[test]
    fn a_tests_folder_needs_autotests_off() {
        let manifest = "[package]\nname = \"eggos-font\"\n\n[[test]]\nname = \"all\"\n";
        let problems = package_problems("font", manifest, Some(&[directory("all", true)]));
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].contains("autotests = false"), "{problems:?}");
    }

    #[test]
    fn autotests_set_in_another_table_does_not_count() {
        let manifest = "[package]\nname = \"eggos-font\"\n\n[lib]\nautotests = false\n\n\
            [[test]]\nname = \"all\"\n";
        let problems = package_problems("font", manifest, Some(&[directory("all", true)]));
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].contains("autotests = false"), "{problems:?}");
    }

    #[test]
    fn an_all_program_without_its_test_target_is_flagged_as_never_built() {
        let manifest = "[package]\nname = \"eggos-font\"\nautotests = false\n";
        let problems = package_problems("font", manifest, Some(&[directory("all", true)]));
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].contains("never built"), "{problems:?}");
    }

    #[test]
    fn a_test_target_with_another_name_or_path_is_flagged() {
        let manifest = "[package]\nautotests = false\n\n[[test]]\nname = \"integration\"\n\
            path = \"tests/integration.rs\"\n";
        let problems = package_problems("font", manifest, None);
        assert_eq!(problems.len(), 2, "{problems:?}");
    }

    #[test]
    fn two_test_targets_are_flagged() {
        let manifest = format!("{SINGLE_PROGRAM}\n[[test]]\nname = \"more\"\n");
        let problems = package_problems("font", &manifest, None);
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].contains("2 [[test]] targets"), "{problems:?}");
    }

    #[test]
    fn a_virtual_workspace_manifest_is_not_a_package() {
        let manifest = "[workspace]\nmembers = [\"tests/linux-static\"]\n";
        let entries = [directory("linux-static", false), file("stray.rs")];
        assert!(package_problems("", manifest, Some(&entries)).is_empty());
    }

    #[test]
    fn the_root_package_reports_paths_without_a_leading_slash() {
        let problems = package_problems("", PACKAGE, Some(&[file("x.rs")]));
        assert_eq!(problems.len(), 2, "{problems:?}");
        assert!(problems[0].starts_with("Cargo.toml: "), "{problems:?}");
        assert!(problems[1].starts_with("tests/x.rs: "), "{problems:?}");
    }
}
