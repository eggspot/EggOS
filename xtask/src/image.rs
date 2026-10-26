//! `cargo xtask image`: the release image with files the user adds to its initramfs (D117),
//! so a static Linux program the user built runs from `eggsh` by its name. Each
//! `--add <file>` goes to `/bin/<file's name>`, or to the path that follows it with `--as`;
//! added files are executable, and one whose path is already in the image is refused.

use crate::{Arch, initramfs};
use std::path::{Path, PathBuf};

const DEFAULT_DIRECTORY: &str = "bin";

/// A file on the host and the path in the image it is added at, without its leading `/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddedFile {
    pub host: String,
    pub path: String,
}

/// Splits `args` into the `--add <file> [--as <path>]` options and the others, in order.
pub fn added_files(args: &[String]) -> Result<(Vec<String>, Vec<AddedFile>), String> {
    let mut rest = Vec::new();
    let mut added = Vec::new();
    let mut args = args.iter().peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--add" => {
                let host = args.next().ok_or("`--add` needs a file")?;
                let path = match args.next_if(|arg| *arg == "--as") {
                    Some(_) => image_path(args.next().ok_or("`--as` needs a path")?)?,
                    None => default_path(host)?,
                };
                added.push(AddedFile {
                    host: host.clone(),
                    path,
                });
            }
            "--as" => return Err("`--as` follows an `--add <file>`".into()),
            _ => rest.push(arg.clone()),
        }
    }
    Ok((rest, added))
}

fn default_path(host: &str) -> Result<String, String> {
    let name = Path::new(host)
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("`{host}` names no file"))?;
    image_path(&format!("{DEFAULT_DIRECTORY}/{name}"))
}

fn image_path(path: &str) -> Result<String, String> {
    let relative = path.trim_start_matches('/');
    let valid = !relative.is_empty()
        && relative
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..");
    if valid {
        Ok(relative.to_owned())
    } else {
        Err(format!(
            "`{path}` is not a path in the image (no empty, `.` or `..` parts)"
        ))
    }
}

/// The added files as the initramfs's extra members.
pub fn members(added: &[AddedFile]) -> Vec<(&str, initramfs::Content<'_>)> {
    added
        .iter()
        .map(|file| (file.path.as_str(), initramfs::Content::Program(&file.host)))
        .collect()
}

/// Builds the release image of `arch` with `added`, copies it to
/// `target/image/eggos-<arch>.elf` and returns that path.
pub fn build(arch: &Arch, added: &[AddedFile]) -> Result<PathBuf, String> {
    let kernel = crate::build_release_image(arch, added)?;
    let directory = crate::workspace_root().join("target").join("image");
    std::fs::create_dir_all(&directory).map_err(|e| format!("{}: {e}", directory.display()))?;
    let image = directory.join(format!("eggos-{}.elf", arch.name));
    std::fs::copy(&kernel, &image).map_err(|e| format!("{}: {e}", image.display()))?;
    Ok(image)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(list: &[&str]) -> Vec<String> {
        list.iter().map(|arg| arg.to_string()).collect()
    }

    fn file(host: &str, path: &str) -> AddedFile {
        AddedFile {
            host: host.into(),
            path: path.into(),
        }
    }

    #[test]
    fn added_files_go_to_bin_or_where_as_says_and_other_options_stay() {
        let (rest, added) = added_files(&strings(&[
            "--arch",
            "riscv64",
            "--add",
            "build/hello",
            "--add",
            "tool",
            "--as",
            "/usr/bin/renamed",
            "--nographic",
        ]))
        .unwrap();
        assert_eq!(rest, strings(&["--arch", "riscv64", "--nographic"]));
        assert_eq!(
            added,
            [
                file("build/hello", "bin/hello"),
                file("tool", "usr/bin/renamed")
            ]
        );
    }

    #[test]
    fn missing_values_and_paths_outside_the_image_are_refused() {
        for args in [
            &["--add"][..],
            &["--add", "x", "--as"],
            &["--as", "y"],
            &["--add", "x", "--as", "/"],
            &["--add", "x", "--as", "a//b"],
            &["--add", "x", "--as", "../etc"],
            &["--add", "x", "--as", "bin/./x"],
            &["--add", ".."],
        ] {
            assert!(added_files(&strings(args)).is_err(), "{args:?}");
        }
    }
}
