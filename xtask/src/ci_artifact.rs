//! Test programs another job of the same GitHub Actions run builds (the Mach-O and MSVC ones,
//! D142 and D146): a QEMU job fetches them just before the boot test that runs them, so their
//! build on the macOS and Windows runners overlaps the kernels' build and the earlier boot tests.

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

const WAIT: Duration = Duration::from_secs(25 * 60);
const POLL: Duration = Duration::from_secs(5);
const TOKEN_VARIABLES: [&str; 2] = ["GH_TOKEN", "GITHUB_TOKEN"];

#[derive(Debug, PartialEq, Eq)]
enum Job {
    NotListed,
    Running,
    Ended(String),
}

#[derive(Debug, PartialEq, Eq)]
enum Next {
    Download,
    Wait,
    Fail(String),
}

fn job_state(jq_output: &str) -> Job {
    match jq_output
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
    {
        None if jq_output.lines().next().is_some() => Job::Running,
        None => Job::NotListed,
        Some(conclusion) => Job::Ended(conclusion.to_string()),
    }
}

fn lists_artifact(jq_output: &str, artifact: &str) -> bool {
    jq_output.lines().any(|line| line.trim() == artifact)
}

fn next(artifact: &str, uploaded: bool, job: &Job) -> Next {
    match (uploaded, job) {
        (true, _) => Next::Download,
        (false, Job::NotListed | Job::Running) => Next::Wait,
        (false, Job::Ended(conclusion)) if conclusion == "success" => {
            Next::Fail(format!("{artifact}: its job ended without it"))
        }
        (false, Job::Ended(conclusion)) => {
            Next::Fail(format!("{artifact}: its job ended: {conclusion}"))
        }
    }
}

fn gave_up(artifact: &str, job: &Job, last_error: Option<&str>) -> String {
    let job = match job {
        Job::NotListed => "its job is not listed in this run",
        Job::Running => "its job is still running",
        Job::Ended(_) => "its job ended",
    };
    let mut message = format!(
        "{artifact}: no artifact after {} minutes ({job})",
        WAIT.as_secs() / 60
    );
    if let Some(error) = last_error {
        message.push_str("; last gh error: ");
        message.push_str(error);
    }
    message
}

/// In a GitHub Actions run, waits for the artifact `name` that the run's job of the same name
/// uploads and downloads it into `directory`. Does nothing when `directory` exists already or
/// outside GitHub Actions; fails when no `GH_TOKEN` is set, when the job ends without the
/// artifact or after 25 minutes.
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
    if !TOKEN_VARIABLES
        .iter()
        .any(|variable| std::env::var_os(variable).is_some_and(|token| !token.is_empty()))
    {
        return Err(format!(
            "{name}: set GH_TOKEN for this step to fetch the artifact of run {run}"
        ));
    }
    let jobs = format!("repos/{repository}/actions/runs/{run}/jobs?per_page=100");
    let select = format!(".jobs[] | select(.name == \"{name}\") | .conclusion // \"\"");
    let artifacts = format!("repos/{repository}/actions/runs/{run}/artifacts?per_page=100");
    let started = Instant::now();
    let mut last_error = None;
    loop {
        let job = match gh(&["api", &jobs, "--jq", &select]) {
            Ok(output) => job_state(&output),
            Err(error) => {
                last_error = Some(error);
                Job::NotListed
            }
        };
        let uploaded = match gh(&["api", &artifacts, "--jq", ".artifacts[].name"]) {
            Ok(output) => lists_artifact(&output, name),
            Err(error) => {
                last_error = Some(error);
                false
            }
        };
        match next(name, uploaded, &job) {
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
                return Err(gave_up(name, &job, last_error.as_deref()));
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
    fn the_job_query_tells_a_missing_job_from_a_running_and_an_ended_one() {
        assert_eq!(job_state(""), Job::NotListed);
        assert_eq!(job_state("\n"), Job::Running);
        assert_eq!(job_state("success\n"), Job::Ended("success".into()));
        assert_eq!(job_state("failure"), Job::Ended("failure".into()));
    }

    #[test]
    fn only_an_artifact_of_exactly_that_name_counts_as_uploaded() {
        let names = "bench-pr-7-abc-aarch64\nmacos-programs\n";
        assert!(lists_artifact(names, "macos-programs"));
        assert!(!lists_artifact(names, "windows-programs"));
        assert!(!lists_artifact(
            "windows-programs-old\n",
            "windows-programs"
        ));
        assert!(!lists_artifact("", "windows-programs"));
    }

    #[test]
    fn an_uploaded_artifact_is_downloaded_whatever_its_job_says() {
        for job in [
            Job::NotListed,
            Job::Running,
            Job::Ended("success".into()),
            Job::Ended("failure".into()),
        ] {
            assert_eq!(next("windows-programs", true, &job), Next::Download);
        }
    }

    #[test]
    fn an_unlisted_or_running_job_is_waited_for() {
        assert_eq!(next("windows-programs", false, &Job::Running), Next::Wait);
        assert_eq!(next("windows-programs", false, &Job::NotListed), Next::Wait);
    }

    #[test]
    fn an_ended_job_without_the_artifact_fails_whatever_its_conclusion() {
        for conclusion in [
            "success",
            "failure",
            "cancelled",
            "skipped",
            "timed_out",
            "neutral",
            "action_required",
            "stale",
        ] {
            assert!(matches!(
                next("windows-programs", false, &Job::Ended(conclusion.into())),
                Next::Fail(reason) if reason.starts_with("windows-programs: its job ended")
            ));
        }
    }

    #[test]
    fn giving_up_names_the_job_state_and_the_last_gh_error() {
        let message = gave_up("macos-programs", &Job::NotListed, Some("gh api: HTTP 403"));
        assert!(message.starts_with("macos-programs: no artifact after 25 minutes"));
        assert!(message.contains("not listed"));
        assert!(message.ends_with("last gh error: gh api: HTTP 403"));
        assert!(!gave_up("macos-programs", &Job::Running, None).contains("gh error"));
    }
}
