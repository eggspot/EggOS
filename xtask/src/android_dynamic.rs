//! `cargo xtask android-dynamic` (D188): our Android console program (`tests/android-console`,
//! our own source, no C) linked dynamically by the Android NDK's clang, so its `PT_INTERP` is
//! Android's `/system/bin/linker64` and its C library bionic's `libc.so`, added to the release
//! image with the shell `system` runs, `/system/build.prop` and Android's runtime module
//! ([`crate::android_runtime`]: the linker and libraries, test data), and run from the shell
//! ([`crate::qemu::android_dynamic`]). aarch64 and x86_64 only: riscv64 has no runtime module
//! the kernel can run yet (D188).

use crate::android::{self, Linkage};
use crate::android_runtime::{LINKER_PATH, Runtime};
use crate::qemu::android_dynamic::PROGRAM_PATH;
use crate::{Arch, SHELL_PATH, build_release_kernel, initramfs, release_initramfs};

/// Builds and links the program, fetches the runtime module, and runs the boot test on `arch`.
pub fn check(arch: &Arch) -> Result<(), String> {
    let (Some(target), Some(_)) = (
        android::target(arch.name),
        crate::android_runtime::module(arch.name),
    ) else {
        return Err(format!(
            "{}: dynamically linked Android programs are tested on aarch64 and x86_64 only \
             (D188)",
            arch.name
        ));
    };
    let ndk = android::find_ndk().ok_or_else(|| {
        format!(
            "{}: the dynamic Android programs boot test needs the Android NDK: set \
             ANDROID_NDK_HOME, or ANDROID_HOME to an SDK with one in ndk/",
            arch.name
        )
    })?;
    let runtime = Runtime::fetch(arch.name)?;
    let program = crate::build_android_console(&ndk, target, Linkage::Dynamic)?;
    let program = program.to_string_lossy().into_owned();
    if !android::is_dynamic_program(&program) {
        return Err(format!(
            "{program}: the NDK did not link a program that {LINKER_PATH} runs"
        ));
    }
    let mut extra = vec![(PROGRAM_PATH, initramfs::Content::Program(&program))];
    extra.extend(android::system_members(SHELL_PATH, arch.name));
    extra.extend(runtime.members());
    let initramfs = release_initramfs(
        arch,
        &extra,
        &format!("initramfs-android-dynamic-{}.cpio", arch.user_target),
    )?;
    let kernel = build_release_kernel(arch, &initramfs)?;
    crate::qemu::android_dynamic::check(arch, &kernel)?;
    eprintln!(
        "xtask: {}: dynamic Android programs boot test passed",
        arch.name
    );
    Ok(())
}
