//! Sets the Linux ABI's `cfg(linux_abi_<want>)` flags from the `wants` module of
//! `src/linux/abi/<arch>.rs` (D218), and links the kernel binary (`cfg(eggos_kernel)`) with its
//! architecture's linker script from the HAL.

use std::env;
use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let arch = env::var("CARGO_CFG_TARGET_ARCH").expect("cargo sets CARGO_CFG_TARGET_ARCH");
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("set by cargo"));
    linux_abi_cfgs(&manifest_dir.join("src").join("linux").join("abi"), &arch);
    println!("cargo::rustc-check-cfg=cfg(eggos_vdso)");
    println!("cargo:rerun-if-env-changed=EGGOS_VDSO");
    if env::var_os("EGGOS_VDSO").is_some() {
        println!("cargo::rustc-cfg=eggos_vdso");
    }
    if env::var_os("CARGO_CFG_EGGOS_KERNEL").is_none() {
        return;
    }
    let script = manifest_dir
        .join("../hal/src/arch")
        .join(&arch)
        .join("linker.ld");
    assert!(
        script.exists(),
        "no linker script for architecture `{arch}` at {}",
        script.display()
    );
    println!("cargo:rerun-if-changed={}", script.display());
    println!("cargo:rustc-link-arg-bins=-T{}", script.display());
    if arch == X86_64_WITH_ABSOLUTE_ENTRY_CODE {
        println!("cargo:rustc-link-arg-bins=--no-pie");
    }
}

/// x86_64's 32-bit entry code uses absolute addresses, so its kernel is never linked as a PIE,
/// even when a `RUSTFLAGS` variable replaces `.cargo/config.toml`'s `relocation-model=static`.
const X86_64_WITH_ABSOLUTE_ENTRY_CODE: &str = "x86_64";

fn linux_abi_cfgs(folder: &Path, arch: &str) {
    println!("cargo:rerun-if-changed={}", folder.display());
    let mut names = std::collections::BTreeSet::new();
    let entries = std::fs::read_dir(folder).expect("kernel/src/linux/abi exists");
    for entry in entries {
        let path = entry.expect("a readable directory entry").path();
        if path.extension().is_some_and(|extension| extension == "rs") {
            println!("cargo:rerun-if-changed={}", path.display());
            let text = std::fs::read_to_string(&path).expect("a readable ABI file");
            names.extend(wants(&text).into_iter().map(|(name, _)| name));
        }
    }
    for name in &names {
        println!("cargo::rustc-check-cfg=cfg(linux_abi_{name})");
    }
    let Ok(text) = std::fs::read_to_string(folder.join(format!("{arch}.rs"))) else {
        return;
    };
    for (name, wanted) in wants(&text) {
        if wanted {
            println!("cargo::rustc-cfg=linux_abi_{name}");
        }
    }
}

/// The `pub const <NAME>: bool = <value>;` lines of `text`'s `pub mod wants { ... }`, each name
/// in lower case.
fn wants(text: &str) -> Vec<(String, bool)> {
    let mut lines = text
        .lines()
        .map(str::trim)
        .skip_while(|line| *line != "pub mod wants {")
        .skip(1);
    let mut found = Vec::new();
    for line in lines.by_ref() {
        if line == "}" {
            break;
        }
        let Some(rest) = line.strip_prefix("pub const ") else {
            continue;
        };
        let Some((name, value)) = rest.split_once(": bool = ") else {
            continue;
        };
        let wanted = match value {
            "true;" => true,
            "false;" => false,
            _ => panic!("`wants::{name}` must be `true` or `false`, written out"),
        };
        found.push((name.to_ascii_lowercase(), wanted));
    }
    found
}
