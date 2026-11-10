//! The web address a person types in the Downloads window and the file a download of it is
//! saved as (docs/ux/downloads-v0.3.md section 6).

use alloc::string::String;
use core::fmt::Write;
use eggos_download_list::host_of;

/// What a download is saved as when its address names no file.
pub const INDEX_NAME: &str = "index.html";
/// The most numbered names tried for one download.
pub const MOST_COPIES: u32 = 9999;

const SCHEMES: [&str; 2] = ["http://", "https://"];

/// `typed` without the spaces around it, when it starts with `http://` or `https://` in any
/// case and names a host.
pub fn web_address(typed: &str) -> Option<&str> {
    let address = typed.trim();
    let known = SCHEMES.iter().any(|scheme| {
        address
            .as_bytes()
            .get(..scheme.len())
            .is_some_and(|start| start.eq_ignore_ascii_case(scheme.as_bytes()))
    });
    (known && !host_of(address.as_bytes()).is_empty()).then_some(address)
}

/// The file name a download of `address` is saved as: the last segment of its path without
/// the query or fragment, or [`INDEX_NAME`] when the path is empty or ends in `/`.
pub fn save_name(address: &str) -> &str {
    let rest = address.find("://").map_or(address, |at| &address[at + 3..]);
    let rest = &rest[..rest.find(['?', '#']).unwrap_or(rest.len())];
    let Some(slash) = rest.find('/') else {
        return INDEX_NAME;
    };
    match rest[slash..].rsplit('/').next() {
        None | Some("" | "." | "..") => INDEX_NAME,
        Some(name) => name,
    }
}

/// Where a download called `name` is saved in `folder`: `folder/name` unless `taken` says that
/// path is taken, else the first free of `folder/<stem> (2)<ext>`, `folder/<stem> (3)<ext>`,
/// ...; `None` when every one up to [`MOST_COPIES`] is taken.
pub fn free_path(folder: &str, name: &str, mut taken: impl FnMut(&str) -> bool) -> Option<String> {
    let (stem, extension) = match name.rfind('.') {
        Some(at) if at > 0 => name.split_at(at),
        _ => (name, ""),
    };
    let mut path = String::new();
    for copy in 1..=MOST_COPIES {
        path.clear();
        let _ = match copy {
            1 => write!(path, "{folder}/{name}"),
            copy => write!(path, "{folder}/{stem} ({copy}){extension}"),
        };
        if !taken(&path) {
            return Some(path);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_web_address_starts_with_http_or_https_in_any_case_and_names_a_host() {
        assert_eq!(
            web_address("  https://example.com/eggos.elf \n"),
            Some("https://example.com/eggos.elf")
        );
        assert_eq!(
            web_address("HTTP://10.0.2.2:8000/a"),
            Some("HTTP://10.0.2.2:8000/a")
        );
        assert_eq!(web_address("example.com/x"), None);
        assert_eq!(web_address("ftp://example.com/x"), None);
        assert_eq!(web_address("https://"), None);
        assert_eq!(web_address("https:///path"), None);
        assert_eq!(web_address(""), None);
    }

    #[test]
    fn the_saved_name_is_the_last_path_segment_without_its_query() {
        assert_eq!(save_name("https://example.com/eggos.elf"), "eggos.elf");
        assert_eq!(save_name("https://example.com/"), "index.html");
        assert_eq!(save_name("https://example.com"), "index.html");
        assert_eq!(save_name("https://example.com?x=1"), "index.html");
        assert_eq!(save_name("https://example.com/a/b?x=1"), "b");
        assert_eq!(save_name("https://example.com/a/b#part"), "b");
        assert_eq!(save_name("https://example.com/a/"), "index.html");
        assert_eq!(save_name("https://example.com/a/.."), "index.html");
    }

    #[test]
    fn a_taken_name_gets_the_first_free_number_before_its_extension() {
        let folder = "/home/root/Downloads";
        assert_eq!(
            free_path(folder, "eggos.elf", |_| false).as_deref(),
            Some("/home/root/Downloads/eggos.elf")
        );
        let taken = ["/home/root/Downloads/eggos.elf"];
        assert_eq!(
            free_path(folder, "eggos.elf", |path| taken.contains(&path)).as_deref(),
            Some("/home/root/Downloads/eggos (2).elf")
        );
        let taken = ["/home/root/Downloads/b", "/home/root/Downloads/b (2)"];
        assert_eq!(
            free_path(folder, "b", |path| taken.contains(&path)).as_deref(),
            Some("/home/root/Downloads/b (3)")
        );
        assert_eq!(
            free_path(folder, ".profile", |path| path.ends_with("/.profile")).as_deref(),
            Some("/home/root/Downloads/.profile (2)")
        );
        assert_eq!(free_path(folder, "x", |_| true), None);
    }
}
