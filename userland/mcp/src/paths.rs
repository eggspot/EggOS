//! Paths an agent names, resolved before they are checked: absolute only, no NUL, `.`, `..`
//! and repeated slashes folded, every component refused if it is a symbolic link (none exist
//! yet, but one must never lead out), and the result inside one of the allowed roots, compared
//! component by component (so `/homework` is not in `/home`). The resolved path is what the
//! command gets and what the audit log shows.

#![deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]

/// The roots the read-only tools reach: the homes, the host's shared folder and the work disk
/// the host attached (D166).
pub const READ_ROOTS: [&str; 3] = ["/home", "/media/host", "/work"];

/// The home folder, the one place the write tools reach in 0.3 (D137): its Trash is the one
/// `trash` and Files use, so whatever is trashed can be restored.
pub const HOME: &str = "/home/root";

/// The roots `tee` and `trash` reach: strictly below the home folder.
pub const WRITE_ROOTS: [&str; 1] = [HOME];

/// The home folder's Trash, which belongs to `trash` and Files: the write tools never reach it.
pub const TRASH: &str = "/home/root/.local/share/Trash";

/// The roots `app_install` installs from: the host's shared folder, which the user fills.
pub const INSTALL_ROOTS: [&str; 1] = ["/media/host"];

/// Every path: the root.
pub const ANYWHERE: [&str; 1] = ["/"];

/// The longest path taken, in bytes.
pub const MAX_PATH_BYTES: usize = 4095;

/// Why a path is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathRefusal {
    /// It does not start with `/`.
    NotAbsolute,
    /// It holds a NUL byte.
    Nul,
    /// It is longer than [`MAX_PATH_BYTES`].
    TooLong,
    /// One of its components is a symbolic link.
    Link,
    /// Resolved, it is outside every root.
    OutsideRoots,
    /// Resolved, it is a root itself where something strictly inside one is needed.
    IsRoot,
    /// Resolved, it is in a place the tool never reaches (the Trash).
    Excluded,
}

impl PathRefusal {
    /// What is wrong, for the agent.
    pub fn message(self) -> &'static str {
        match self {
            PathRefusal::NotAbsolute => "the path must be absolute (start with /)",
            PathRefusal::Nul => "the path holds a NUL character",
            PathRefusal::TooLong => "the path is longer than 4095 bytes",
            PathRefusal::Link => "the path goes through a symbolic link",
            PathRefusal::OutsideRoots => "the path is outside the folders this tool may reach",
            PathRefusal::IsRoot => "the path names the folder itself, not something inside it",
            PathRefusal::Excluded => "the path is in the Trash, which belongs to trash and Files",
        }
    }
}

/// Resolves `path` into `out` and checks it is inside one of `roots`; `is_link` says whether
/// a resolved prefix is a symbolic link (an error, such as a missing file, counts as not a
/// link: the command reports it).
pub fn resolve<'o>(
    path: &str,
    out: &'o mut [u8],
    roots: &[&str],
    is_link: &mut dyn FnMut(&str) -> bool,
) -> Result<&'o str, PathRefusal> {
    let bytes = path.as_bytes();
    if bytes.contains(&0) {
        return Err(PathRefusal::Nul);
    }
    if bytes.len() > MAX_PATH_BYTES {
        return Err(PathRefusal::TooLong);
    }
    if bytes.first() != Some(&b'/') {
        return Err(PathRefusal::NotAbsolute);
    }
    let mut length = 0;
    for component in path.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                let parent = out
                    .get(..length)
                    .and_then(|done| done.iter().rposition(|&byte| byte == b'/'))
                    .unwrap_or(0);
                length = parent;
            }
            name => {
                let end = length + 1 + name.len();
                let place = out.get_mut(length..end).ok_or(PathRefusal::TooLong)?;
                if let Some((slash, rest)) = place.split_first_mut() {
                    *slash = b'/';
                    rest.copy_from_slice(name.as_bytes());
                }
                length = end;
                let prefix = text(out, length);
                if is_link(prefix) {
                    return Err(PathRefusal::Link);
                }
            }
        }
    }
    if length == 0 {
        if let Some(slash) = out.first_mut() {
            *slash = b'/';
        }
        length = 1;
    }
    let resolved = text(out, length);
    if !roots.iter().any(|root| is_inside(resolved, root)) {
        return Err(PathRefusal::OutsideRoots);
    }
    Ok(resolved)
}

fn text(out: &[u8], length: usize) -> &str {
    out.get(..length)
        .and_then(|bytes| core::str::from_utf8(bytes).ok())
        .unwrap_or("")
}

/// Whether `path` (resolved) is `root` or inside it; everything is inside `/`.
pub fn is_inside(path: &str, root: &str) -> bool {
    root == "/"
        || path
            .strip_prefix(root)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

/// The checks after resolution: when `strictly` is set, `resolved` must not be a root itself;
/// it must not be in any of `excluded`.
pub fn check_place(
    resolved: &str,
    roots: &[&str],
    strictly: bool,
    excluded: &[&str],
) -> Result<(), PathRefusal> {
    if strictly && roots.contains(&resolved) {
        return Err(PathRefusal::IsRoot);
    }
    if excluded.iter().any(|place| is_inside(resolved, place)) {
        return Err(PathRefusal::Excluded);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::indexing_slicing, clippy::unwrap_used, clippy::panic)]

    use super::*;

    fn resolved(path: &str) -> Result<String, PathRefusal> {
        let mut out = [0u8; MAX_PATH_BYTES + 1];
        resolve(path, &mut out, &READ_ROOTS, &mut |_| false).map(str::to_owned)
    }

    #[test]
    fn paths_inside_the_roots_are_folded_and_kept() {
        assert_eq!(resolved("/home"), Ok("/home".into()));
        assert_eq!(resolved("/home/"), Ok("/home".into()));
        assert_eq!(resolved("/home/./a//b/"), Ok("/home/a/b".into()));
        assert_eq!(resolved("/home/a/../b"), Ok("/home/b".into()));
        assert_eq!(
            resolved("/media/host/readme.txt"),
            Ok("/media/host/readme.txt".into())
        );
        assert_eq!(resolved("//home"), Ok("/home".into()));
        assert_eq!(
            resolved("/work/target/../build.log"),
            Ok("/work/build.log".into())
        );
    }

    #[test]
    fn paths_that_leave_the_roots_are_refused_after_resolution() {
        for path in [
            "/home/../etc",
            "/home/../etc/eggos/init.conf",
            "/media/host/..",
            "/media/host/../../home/../etc",
            "/homework",
            "/media/hostile",
            "/workshop",
            "/work/..",
            "/",
            "/..",
            "/home/../../../..",
            "/etc",
        ] {
            assert_eq!(resolved(path), Err(PathRefusal::OutsideRoots), "{path}");
        }
    }

    #[test]
    fn relative_nul_and_long_paths_are_refused() {
        assert_eq!(resolved("home"), Err(PathRefusal::NotAbsolute));
        assert_eq!(resolved(""), Err(PathRefusal::NotAbsolute));
        assert_eq!(resolved("./home"), Err(PathRefusal::NotAbsolute));
        assert_eq!(resolved("/home/a\0b"), Err(PathRefusal::Nul));
        let long = format!("/home/{}", "a".repeat(MAX_PATH_BYTES));
        assert_eq!(resolved(&long), Err(PathRefusal::TooLong));
        let mut small = [0u8; 8];
        assert_eq!(
            resolve("/home/abcdef", &mut small, &READ_ROOTS, &mut |_| false),
            Err(PathRefusal::TooLong)
        );
    }

    #[test]
    fn a_symbolic_link_anywhere_on_the_way_is_refused() {
        let mut out = [0u8; 64];
        let mut asked = Vec::new();
        let result = resolve("/home/a/b/c", &mut out, &READ_ROOTS, &mut |prefix| {
            asked.push(prefix.to_owned());
            prefix == "/home/a"
        });
        assert_eq!(result, Err(PathRefusal::Link));
        assert_eq!(asked, ["/home", "/home/a"]);
    }

    #[test]
    fn being_inside_is_checked_component_by_component() {
        assert!(is_inside("/home", "/home"));
        assert!(is_inside("/home/x", "/home"));
        assert!(!is_inside("/homex", "/home"));
        assert!(!is_inside("/hom", "/home"));
        assert!(is_inside("/bin/uname", "/"));
        assert!(is_inside("/", "/"));
    }

    fn written(path: &str) -> Result<String, PathRefusal> {
        let mut out = [0u8; MAX_PATH_BYTES + 1];
        let resolved = resolve(path, &mut out, &WRITE_ROOTS, &mut |_| false)?;
        check_place(resolved, &WRITE_ROOTS, true, &[TRASH])?;
        Ok(resolved.to_owned())
    }

    #[test]
    fn writes_reach_strictly_below_the_home_folder_and_never_the_trash() {
        assert_eq!(
            written("/home/root/note.txt"),
            Ok("/home/root/note.txt".into())
        );
        assert_eq!(written("/home/root/a/../b"), Ok("/home/root/b".into()));
        assert_eq!(
            written("/home/root/.local/x"),
            Ok("/home/root/.local/x".into())
        );
        assert_eq!(written("/home/root"), Err(PathRefusal::IsRoot));
        assert_eq!(written("/home/root/"), Err(PathRefusal::IsRoot));
        assert_eq!(written("/home/root/x/.."), Err(PathRefusal::IsRoot));
        for outside in [
            "/home",
            "/home/other/x",
            "/home/rootless",
            "/home/root/../../etc/x",
            "/var/log/agent-audit.log",
        ] {
            assert_eq!(
                written(outside),
                Err(PathRefusal::OutsideRoots),
                "{outside}"
            );
        }
        for trash in [
            "/home/root/.local/share/Trash",
            "/home/root/.local/share/Trash/info/x.trashinfo",
            "/home/root/.local/share/Trash/files/../info/y",
            "/home/root/.local/share/./Trash/files",
        ] {
            assert_eq!(written(trash), Err(PathRefusal::Excluded), "{trash}");
        }
        let mut out = [0u8; 64];
        assert_eq!(
            resolve("/bin/../bin/uname", &mut out, &ANYWHERE, &mut |_| false),
            Ok("/bin/uname")
        );
        assert_eq!(
            resolve("uname", &mut out, &ANYWHERE, &mut |_| false),
            Err(PathRefusal::NotAbsolute)
        );
    }
}
