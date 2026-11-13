//! `cargo xtask release-tag <sha> [--push]` (D242): reads the merge commit's `Release:` trailer
//! and decides whether CI tags a release on it. Run by the "Tag release" workflow after CI passed
//! on `main`; prints `tag=vX.Y.Z` (and writes it to `$GITHUB_OUTPUT`) when the Release workflow
//! should publish, nothing when there is nothing to release, and fails loudly on a commit whose
//! version was bumped without a trailer or whose trailer does not match the code.

use std::process::Command;

/// What the merge commit asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// No trailer and the workspace version is already tagged: nothing to do.
    Nothing,
    /// Tag `tag` on the commit, then publish it.
    Tag(String),
    /// The tag is on the commit already but its Release is not published: publish again.
    Publish(String),
}

/// The value of the one `Release:` line in the message's trailer block (its last two
/// paragraphs, after carriage returns and trailing blank lines are dropped).
pub fn release_trailer(message: &str) -> Result<Option<String>, String> {
    let text = message.replace('\r', "");
    let paragraphs: Vec<&str> = text.trim_end().split("\n\n").collect();
    let tail = &paragraphs[paragraphs.len().saturating_sub(2)..];
    let values: Vec<&str> = tail
        .iter()
        .flat_map(|paragraph| paragraph.lines())
        .filter_map(|line| line.strip_prefix("Release:"))
        .map(str::trim)
        .collect();
    match values.as_slice() {
        [] => Ok(None),
        [value] => Ok(Some((*value).to_owned())),
        _ => Err("the commit has more than one Release: line".into()),
    }
}

/// The version a `vMAJOR.MINOR.PATCH` tag names, without leading zeros or suffixes.
pub fn parse_tag(tag: &str) -> Option<(u64, u64, u64)> {
    let numbers = tag.strip_prefix('v')?;
    let mut parts = numbers.split('.');
    let mut next = || -> Option<u64> {
        let part = parts.next()?;
        let plain = !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit());
        (plain && (part == "0" || !part.starts_with('0'))).then(|| part.parse().ok())?
    };
    let version = (next()?, next()?, next()?);
    parts.next().is_none().then_some(version)
}

/// `[workspace.package]`'s `version` in a `Cargo.toml`.
pub fn workspace_version(manifest: &str) -> Option<String> {
    let mut in_package = false;
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_package = line == "[workspace.package]";
            continue;
        }
        if in_package && let Some(value) = line.strip_prefix("version") {
            let value = value.trim_start().strip_prefix('=')?.trim();
            return Some(value.trim_matches('"').to_owned());
        }
    }
    None
}

/// Where the tag a commit's version names stands on the remote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TagState {
    Absent,
    OnCommit { published: bool },
    Elsewhere,
}

/// The decision for a commit with `trailer`, whose workspace version is `version`, whose notes
/// file exists or not, and whose version's tag stands as `state`.
pub fn decide(
    trailer: Option<&str>,
    version: &str,
    notes_exist: bool,
    state: &TagState,
) -> Result<Decision, String> {
    let wanted = format!("v{version}");
    let Some(trailer) = trailer else {
        return match state {
            TagState::Absent => Err(format!(
                "version {version} is not tagged and the commit has no Release: line: add \
                 `Release: {wanted}` to the next merge, or run `gh workflow run release.yml -f tag={wanted}` \
                 after tagging by hand"
            )),
            _ => Ok(Decision::Nothing),
        };
    };
    if parse_tag(trailer).is_none() {
        return Err(format!(
            "`Release: {trailer}` is not a vMAJOR.MINOR.PATCH version"
        ));
    }
    if trailer != wanted {
        return Err(format!(
            "`Release: {trailer}` does not match the workspace version {version}"
        ));
    }
    if !notes_exist {
        return Err(format!("docs/releases/{wanted}.md is missing"));
    }
    match state {
        TagState::Absent => Ok(Decision::Tag(wanted)),
        TagState::OnCommit { published: false } => Ok(Decision::Publish(wanted)),
        TagState::OnCommit { published: true } => Ok(Decision::Nothing),
        TagState::Elsewhere => Err(format!("{wanted} is already tagged on another commit")),
    }
}

fn git(args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(crate::workspace_root())
        .output()
        .map_err(|e| format!("could not run git: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "`git {}` failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn tag_state(tag: &str, sha: &str) -> Result<TagState, String> {
    let peeled = format!("refs/tags/{tag}^{{}}");
    let listed = git(&["ls-remote", "--tags", "origin", &peeled])?;
    let Some(commit) = listed.split_whitespace().next() else {
        return Ok(TagState::Absent);
    };
    if commit != sha {
        return Ok(TagState::Elsewhere);
    }
    let view = Command::new("gh")
        .args([
            "release", "view", tag, "--json", "isDraft", "--jq", ".isDraft",
        ])
        .current_dir(crate::workspace_root())
        .output()
        .map_err(|e| format!("could not run gh: {e}"))?;
    let published =
        view.status.success() && String::from_utf8_lossy(&view.stdout).trim() == "false";
    Ok(TagState::OnCommit { published })
}

/// Runs the command for commit `sha`; with `push`, creates and pushes the annotated tag.
pub fn run(sha: &str, push: bool) -> Result<(), String> {
    if sha.len() != 40 || !sha.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("the commit must be a full 40-digit sha".into());
    }
    git(&["merge-base", "--is-ancestor", sha, "origin/main"])?;
    let message = git(&["log", "-1", "--format=%B", sha])?;
    let trailer = release_trailer(&message)?;
    let manifest = git(&["show", &format!("{sha}:Cargo.toml")])?;
    let version =
        workspace_version(&manifest).ok_or("Cargo.toml has no [workspace.package] version")?;
    let wanted = format!("v{version}");
    let notes_exist = git(&[
        "cat-file",
        "-e",
        &format!("{sha}:docs/releases/{wanted}.md"),
    ])
    .is_ok();
    let state = tag_state(&wanted, sha)?;
    let tag = match decide(trailer.as_deref(), &version, notes_exist, &state)? {
        Decision::Nothing => String::new(),
        Decision::Publish(tag) => tag,
        Decision::Tag(tag) => {
            if push {
                git(&["tag", "-a", &tag, sha, "-m", &format!("EggOS {tag}")])?;
                git(&["push", "origin", &format!("refs/tags/{tag}")])?;
            }
            tag
        }
    };
    println!("tag={tag}");
    if let Ok(path) = std::env::var("GITHUB_OUTPUT") {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .map_err(|e| format!("{path}: {e}"))?;
        writeln!(file, "tag={tag}").map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_trailer_is_found_in_the_last_paragraphs_only() {
        let message = "Batch 8 (#170)\n\nRelease: v0.9.0 is mentioned here\n\nSummary.\n\nRelease: v0.5.0\nCo-authored-by: someone\n";
        assert_eq!(release_trailer(message).unwrap().as_deref(), Some("v0.5.0"));
        assert_eq!(
            release_trailer("Fix (#171)\n\nNo trailer.\n").unwrap(),
            None
        );
        assert!(release_trailer("x\n\nRelease: v0.5.0\nRelease: v0.5.1\n").is_err());
        assert_eq!(
            release_trailer("x\r\n\r\nRelease: v0.5.0\r\n\r\n")
                .unwrap()
                .as_deref(),
            Some("v0.5.0")
        );
    }

    #[test]
    fn only_plain_three_part_versions_are_tags() {
        assert_eq!(parse_tag("v0.5.0"), Some((0, 5, 0)));
        assert_eq!(parse_tag("v1.20.3"), Some((1, 20, 3)));
        for bad in [
            "0.5.0",
            "v1.2",
            "v01.2.3",
            "v1.2.3-rc1",
            "v1.2.3;rm",
            "v$(x).1.1",
            "v1.2.3.4",
        ] {
            assert_eq!(parse_tag(bad), None, "{bad}");
        }
    }

    #[test]
    fn the_workspace_version_is_read_from_its_table() {
        let manifest = "[workspace]\nmembers = []\n\n[workspace.package]\nversion = \"0.5.0\"\nedition = \"2024\"\n\n[package]\nversion = \"9.9.9\"\n";
        assert_eq!(workspace_version(manifest).as_deref(), Some("0.5.0"));
    }

    #[test]
    fn the_decision_follows_trailer_version_notes_and_tag() {
        let absent = TagState::Absent;
        assert_eq!(
            decide(Some("v0.5.0"), "0.5.0", true, &absent),
            Ok(Decision::Tag("v0.5.0".into()))
        );
        assert!(decide(Some("v0.5.1"), "0.5.0", true, &absent).is_err());
        assert!(decide(Some("v0.5.0"), "0.5.0", false, &absent).is_err());
        assert!(decide(None, "0.5.0", true, &absent).is_err());
        let tagged = TagState::OnCommit { published: true };
        assert_eq!(decide(None, "0.5.0", true, &tagged), Ok(Decision::Nothing));
        assert_eq!(
            decide(Some("v0.5.0"), "0.5.0", true, &tagged),
            Ok(Decision::Nothing)
        );
        let draft = TagState::OnCommit { published: false };
        assert_eq!(
            decide(Some("v0.5.0"), "0.5.0", true, &draft),
            Ok(Decision::Publish("v0.5.0".into()))
        );
        assert!(decide(Some("v0.5.0"), "0.5.0", true, &TagState::Elsewhere).is_err());
    }
}
