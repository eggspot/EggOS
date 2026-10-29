//! Test programs another job of the same GitHub Actions run builds (the Mach-O and MSVC ones,
//! D142 and D146): a QEMU job fetches them just before the boot test that runs them, so their
//! build on the macOS and Windows runners overlaps the kernels' build and the earlier boot tests.

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

const WAIT: Duration = Duration::from_secs(25 * 60);
const POLL: Duration = Duration::from_secs(5);

#[derive(Debug, PartialEq, Eq)]
enum Next {
    Download,
    Wait,
    Fail(String),
}

fn next(artifact: &str, uploaded: bool, conclusion: &str) -> Next {
    match (uploaded, conclusion) {
        (true, _) => Next::Download,
        (false, "success") => Next::Fail(format!("{artifact}: its job ended without it")),
        (false, "failure" | "cancelled" | "skipped" | "timed_out") => {
            Next::Fail(format!("{artifact}: its job ended: {conclusion}"))
        }
        (false, _) => Next::Wait,
    }
}

/// In a GitHub Actions run, waits for the artifact `name` that the run's job of the same name
/// uploads and downloads it into `directory`. Does nothing when `directory` exists already or
/// outside GitHub Actions; fails when the job ends without the artifact or after 25 minutes.
pub fn fetch_when_built(name: &str, directory: &Path) -> Result<(), String> {
    let (Ok(run), Ok(repository)) = (
        std::env::var("GITHUB_RUN_ID"),
        std::env::var("GITHUB_REPOSITORY"),
    ) else {
        return Ok(());
    };
    if directory.exists() {
        return Ok(());
    }
    let started = Instant::now();
    loop {
        let jobs = format!("repos/{repository}/actions/runs/{run}/jobs");
        let select = format!(".jobs[] | select(.name == \"{name}\") | .conclusion // \"\"");
        let conclusion = gh(&["api", &jobs, "--jq", &select]).unwrap_or_default();
        let artifacts = format!("repos/{repository}/actions/runs/{run}/artifacts");
        let uploaded = gh(&["api", &artifacts, "--jq", ".artifacts[].name"])
            .is_ok_and(|names| names.lines().any(|line| line == name));
        match next(name, uploaded, conclusion.trim()) {
            Next::Download => {
                let directory = directory.to_string_lossy();
                let arguments: [&str; 9] = [
                    "run",
                    "download",
                    &run,
                    "--repo",
                    &repository,
                    "--name",
                    name,
                    "--dir",
                    &directory,
                ];
                return gh(&arguments).map(|_| ());
            }
            Next::Fail(reason) => return Err(reason),
            Next::Wait if started.elapsed() >= WAIT => {
                return Err(format!(
                    "{name}: no artifact after {} minutes",
                    WAIT.as_secs() / 60
                ));
            }
            Next::Wait => {
                eprintln!("xtask: waiting for {name}");
                std::thread::sleep(POLL);
            }
        }
    }
}

fn gh(arguments: &[&str]) -> Result<String, String> {
    let output = Command::new("gh")
        .args(arguments)
        .output()
        .map_err(|error| format!("gh: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "gh {}: {}",
            arguments.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_uploaded_artifact_is_downloaded_whatever_its_job_says() {
        assert_eq!(next("windows-programs", true, ""), Next::Download);
        assert_eq!(next("windows-programs", true, "success"), Next::Download);
    }

    #[test]
    fn a_running_job_is_waited_for_and_an_ended_one_without_the_artifact_fails() {
        assert_eq!(next("windows-programs", false, ""), Next::Wait);
        for conclusion in ["success", "failure", "cancelled", "skipped", "timed_out"] {
            assert!(matches!(
                next("windows-programs", false, conclusion),
                Next::Fail(_)
            ));
        }
    }
}
