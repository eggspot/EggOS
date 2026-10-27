//! `cargo xtask release` and `cargo xtask screenshot` (D116): the files a release ships, one
//! kernel image per architecture with its initramfs embedded, `SHA256SUMS`, the run scripts and
//! the release notes' skeleton, and the README's picture of the desktop.

use crate::{ARCHES, Arch, png, qemu, sha256};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The workspace version, `[workspace.package]` in the root `Cargo.toml`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

const SUMS_FILE: &str = "SHA256SUMS";
const SHELL_SCRIPT: &str = "run-eggos.sh";
const POWERSHELL_SCRIPT: &str = "run-eggos.ps1";
const NOTES_FILE: &str = "RELEASE-NOTES.md";
const SCREENSHOT_PATH: &str = "docs/images/desktop.png";

#[derive(Debug)]
struct Image {
    arch: &'static Arch,
    file_name: String,
    bytes: u64,
    sha256: String,
    boot_to_prompt: Duration,
}

fn image_name(arch: &Arch) -> String {
    format!("eggos-{VERSION}-{}.elf", arch.name)
}

fn output_dir() -> PathBuf {
    crate::target_directory().join("release-dist")
}

/// Builds and boots the release image of every architecture, then writes the images,
/// `SHA256SUMS`, `run-eggos.sh`, `run-eggos.ps1` and `RELEASE-NOTES.md` to
/// `target/release-dist/`.
pub fn run() -> Result<(), String> {
    let dir = output_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut images = Vec::new();
    for arch in ARCHES {
        let kernel = crate::build_kernel(arch, None)?;
        let boot_to_prompt = qemu::boot_to_prompt_time(arch, &kernel)?;
        let contents = std::fs::read(&kernel).map_err(|e| format!("{}: {e}", kernel.display()))?;
        let file_name = image_name(arch);
        write(&dir.join(&file_name), &contents)?;
        images.push(Image {
            arch,
            file_name,
            bytes: contents.len() as u64,
            sha256: sha256::hex_digest(&contents),
            boot_to_prompt,
        });
    }
    write(&dir.join(SUMS_FILE), sums(&images).as_bytes())?;
    let scripts = script_arches();
    write(&dir.join(SHELL_SCRIPT), shell_script(&scripts).as_bytes())?;
    set_executable(&dir.join(SHELL_SCRIPT))?;
    write(
        &dir.join(POWERSHELL_SCRIPT),
        powershell_script(&scripts).as_bytes(),
    )?;
    write(&dir.join(NOTES_FILE), release_notes(&images).as_bytes())?;
    for image in &images {
        eprintln!(
            "xtask: {}: {} bytes, sha256 {}, boot to prompt {} ms",
            image.file_name,
            image.bytes,
            image.sha256,
            image.boot_to_prompt.as_millis()
        );
    }
    eprintln!("xtask: wrote the release files to {}", dir.display());
    Ok(())
}

/// Boots `arch`'s release image headless and writes the desktop at the shell's prompt to
/// `docs/images/desktop.png`.
pub fn screenshot(arch: &Arch) -> Result<(), String> {
    let kernel = crate::build_kernel(arch, None)?;
    let picture = qemu::screenshot(arch, &kernel)?;
    let encoded = png::encode(&picture);
    let path = crate::workspace_root().join(SCREENSHOT_PATH);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    write(&path, &encoded)?;
    eprintln!(
        "xtask: wrote {} ({}x{}, {} bytes)",
        path.display(),
        picture.width,
        picture.height,
        encoded.len()
    );
    Ok(())
}

fn write(path: &Path, contents: &[u8]) -> Result<(), String> {
    std::fs::write(path, contents).map_err(|e| format!("could not write {}: {e}", path.display()))
}

#[cfg(unix)]
fn set_executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> Result<(), String> {
    Ok(())
}

fn sums(images: &[Image]) -> String {
    images.iter().fold(String::new(), |mut text, image| {
        let _ = writeln!(text, "{}  {}", image.sha256, image.file_name);
        text
    })
}

#[derive(Debug)]
struct ScriptArch {
    name: &'static str,
    window: Vec<String>,
    serial: Vec<String>,
    shell_folder: Vec<String>,
    powershell_folder: Vec<String>,
    shell_disk: Vec<String>,
    powershell_disk: Vec<String>,
}

const SHELL_FOLDER: &str = "$folder";

const POWERSHELL_FOLDER: &str = "$Folder";

const SHELL_DISK: &str = "$disk_path";

const POWERSHELL_DISK: &str = "$DiskPath";

fn script_arches() -> Vec<ScriptArch> {
    ARCHES
        .iter()
        .map(|arch| ScriptArch {
            name: arch.name,
            window: qemu::interactive_args(arch, true),
            serial: qemu::interactive_args(arch, false),
            shell_folder: qemu::host_folder_args(arch, SHELL_FOLDER),
            powershell_folder: qemu::host_folder_args(arch, POWERSHELL_FOLDER),
            shell_disk: qemu::disk_args(arch, SHELL_DISK),
            powershell_disk: qemu::disk_args(arch, POWERSHELL_DISK),
        })
        .collect()
}

fn double_quoted(args: &[String]) -> String {
    args.iter()
        .map(|arg| {
            assert!(
                !arg.contains(['"', '`', '\\']),
                "a QEMU argument holds a quote, a backtick or a backslash: {arg}"
            );
            format!("\"{arg}\"")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn single_quoted(args: &[String]) -> String {
    args.iter()
        .map(|arg| {
            assert!(!arg.contains('\''), "a QEMU argument holds a quote: {arg}");
            format!("'{arg}'")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn shell_script(arches: &[ScriptArch]) -> String {
    let names: Vec<&str> = arches.iter().map(|arch| arch.name).collect();
    let mut cases = String::new();
    let mut folder_cases = String::new();
    let mut disk_cases = String::new();
    for arch in arches {
        let _ = writeln!(
            disk_cases,
            "    {}) set -- \"$@\" {} ;;",
            arch.name,
            double_quoted(&arch.shell_disk)
        );
        let _ = writeln!(
            cases,
            "  {}:window) set -- {} ;;\n  {}:serial) set -- {} ;;",
            arch.name,
            single_quoted(&arch.window),
            arch.name,
            single_quoted(&arch.serial)
        );
        let _ = writeln!(
            folder_cases,
            "    {}) set -- \"$@\" {} ;;",
            arch.name,
            double_quoted(&arch.shell_folder)
        );
    }
    format!(
        r#"#!/bin/sh
set -eu
version='{VERSION}'
usage="usage: run-eggos.sh [{choices}] [--serial] [--apps <folder>] [--disk <image> | --no-disk] [--format-disk]"
case "$(uname -m)" in
  arm64 | aarch64) arch=aarch64 ;;
  riscv64) arch=riscv64 ;;
  *) arch=x86_64 ;;
esac
mode=window
apps=''
disk=''
no_disk=''
format_disk=''
while [ "$#" -gt 0 ]; do
  case "$1" in
    --serial) mode=serial ;;
    --apps)
      if [ "$#" -lt 2 ]; then echo "$usage" >&2; exit 2; fi
      shift
      apps="$1" ;;
    --disk)
      if [ "$#" -lt 2 ]; then echo "$usage" >&2; exit 2; fi
      shift
      disk="$1" ;;
    --no-disk) no_disk=yes ;;
    --format-disk) format_disk=yes ;;
    {alternatives}) arch="$1" ;;
    -h | --help) echo "$usage"; exit 0 ;;
    *) echo "$usage" >&2; exit 2 ;;
  esac
  shift
done
if [ -n "$apps" ]; then
  if [ ! -d "$apps" ]; then
    echo "run-eggos.sh: $apps is not a folder" >&2
    exit 1
  fi
  folder=$(CDPATH= cd -- "$apps" && pwd | sed 's/,/,,/g')
fi
if [ -n "$no_disk" ] && [ -n "$disk$format_disk" ]; then
  echo "$usage" >&2
  exit 2
fi
dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
file="eggos-$version-$arch.elf"
image="$dir/$file"
if [ ! -f "$image" ]; then
  echo "run-eggos.sh: $file not found: download it next to this script" >&2
  exit 1
fi
if [ -f "$dir/{SUMS_FILE}" ]; then
  line=$(grep -F "  $file" "$dir/{SUMS_FILE}" || true)
  if command -v sha256sum >/dev/null 2>&1; then
    check='sha256sum -c -'
  elif command -v shasum >/dev/null 2>&1; then
    check='shasum -a 256 -c -'
  else
    check=''
  fi
  if [ -z "$line" ]; then
    echo "run-eggos.sh: $file is not listed in {SUMS_FILE}" >&2
    exit 1
  elif [ -z "$check" ]; then
    echo "run-eggos.sh: no sha256sum or shasum found: the checksum was not verified" >&2
  elif (cd "$dir" && printf '%s\n' "$line" | $check >/dev/null 2>&1); then
    echo "run-eggos.sh: $file: checksum OK"
  else
    echo "run-eggos.sh: $file does not match its checksum in {SUMS_FILE}: download it again" >&2
    exit 1
  fi
else
  echo "run-eggos.sh: no {SUMS_FILE} next to this script: the checksum was not verified" >&2
fi
qemu="qemu-system-$arch"
if ! command -v "$qemu" >/dev/null 2>&1; then
  echo "run-eggos.sh: $qemu not found: install QEMU (macOS: brew install qemu; Debian or Ubuntu: sudo apt install qemu-system)" >&2
  exit 1
fi
case "$arch:$mode" in
{cases}esac
if [ -n "$apps" ]; then
  case "$arch" in
{folder_cases}  esac
  echo "run-eggos.sh: $apps is at /media/host in EggOS, read-only."
fi
if [ -z "$no_disk" ]; then
  if [ -z "$disk" ]; then disk="$dir/eggos-$arch-disk.img"; fi
  if [ -n "$format_disk" ]; then
    : > "$disk"
    echo "run-eggos.sh: emptied $disk: EggOS formats it at boot."
  fi
  if [ ! -s "$disk" ]; then
    dd if=/dev/null of="$disk" bs=1 seek={disk_bytes} 2>/dev/null
  fi
  disk_path=$(CDPATH= cd -- "$(dirname -- "$disk")" && pwd)/$(basename -- "$disk")
  disk_path=$(printf '%s' "$disk_path" | sed 's/,/,,/g')
  case "$arch" in
{disk_cases}  esac
  echo "run-eggos.sh: $disk holds /home and /Applications in EggOS (--no-disk: none)."
fi
if [ "$mode" = window ]; then
  echo "run-eggos.sh: booting EggOS $version ($arch) in a QEMU window; its console is here too. Quit: close the window, or Ctrl-A then X here."
else
  echo "run-eggos.sh: booting EggOS $version ($arch) on this terminal. Quit: Ctrl-A then X."
fi
exec "$qemu" "$@" -kernel "$image"
"#,
        choices = names.join("|"),
        alternatives = names.join(" | "),
        disk_bytes = qemu::DISK_IMAGE_BYTES,
    )
}

fn powershell_script(arches: &[ScriptArch]) -> String {
    let names: Vec<String> = arches
        .iter()
        .map(|arch| format!("'{}'", arch.name))
        .collect();
    let mut cases = String::new();
    let mut folder_cases = String::new();
    let mut disk_cases = String::new();
    for arch in arches {
        let _ = writeln!(
            disk_cases,
            "        '{}' {{ @({}) }}",
            arch.name,
            double_quoted(&arch.powershell_disk).replace("\" \"", "\", \"")
        );
        let list = |args: &[String]| single_quoted(args).replace("' '", "', '");
        let _ = writeln!(
            cases,
            "    '{}' {{ if ($Serial) {{ @({}) }} else {{ @({}) }} }}",
            arch.name,
            list(&arch.serial),
            list(&arch.window)
        );
        let _ = writeln!(
            folder_cases,
            "        '{}' {{ @({}) }}",
            arch.name,
            double_quoted(&arch.powershell_folder).replace("\" \"", "\", \"")
        );
    }
    format!(
        r#"[CmdletBinding(PositionalBinding = $false)]
param(
    [string]$Arch,
    [switch]$Serial,
    [string]$Apps,
    [string]$Disk,
    [switch]$NoDisk,
    [switch]$FormatDisk,
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$Rest
)
$ErrorActionPreference = 'Stop'
$Version = '{VERSION}'
$Arches = @({names})
$Usage = "usage: run-eggos.ps1 [-Arch] <{choices}> [-Serial] [-Apps <folder>] [-Disk <image> | -NoDisk] [-FormatDisk]"
for ($i = 0; $i -lt $Rest.Count; $i++) {{
    $argument = $Rest[$i]
    if ($argument -eq '--serial') {{
        $Serial = $true
    }} elseif ($argument -eq '--apps' -and $i + 1 -lt $Rest.Count) {{
        $i++
        $Apps = $Rest[$i]
    }} elseif ($argument -eq '--disk' -and $i + 1 -lt $Rest.Count) {{
        $i++
        $Disk = $Rest[$i]
    }} elseif ($argument -eq '--no-disk') {{
        $NoDisk = $true
    }} elseif ($argument -eq '--format-disk') {{
        $FormatDisk = $true
    }} elseif ($Arches -contains $argument -and -not $Arch) {{
        $Arch = $argument
    }} else {{
        Write-Error "run-eggos.ps1: $Usage"
        exit 2
    }}
}}
if ($NoDisk -and ($Disk -or $FormatDisk)) {{
    Write-Error "run-eggos.ps1: $Usage"
    exit 2
}}
if ($Arch -and $Arches -notcontains $Arch) {{
    Write-Error "run-eggos.ps1: $Usage"
    exit 2
}}
if (-not $Arch) {{
    $hostArch = $null
    try {{ $hostArch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString() }} catch {{ }}
    if (-not $hostArch) {{ $hostArch = $env:PROCESSOR_ARCHITECTURE }}
    $Arch = if ($hostArch -match '^(Arm64|ARM64)$') {{ 'aarch64' }} else {{ 'x86_64' }}
}}
$File = "eggos-$Version-$Arch.elf"
$Image = Join-Path $PSScriptRoot $File
if (-not (Test-Path -LiteralPath $Image)) {{
    Write-Error "run-eggos.ps1: $File not found: download it next to this script"
    exit 1
}}
$Sums = Join-Path $PSScriptRoot '{SUMS_FILE}'
if (Test-Path -LiteralPath $Sums) {{
    $line = Get-Content -LiteralPath $Sums | Where-Object {{ $_.EndsWith("  $File") }} | Select-Object -First 1
    if (-not $line) {{
        Write-Error "run-eggos.ps1: $File is not listed in {SUMS_FILE}"
        exit 1
    }}
    $expected = $line.Substring(0, 64)
    $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $Image).Hash.ToLowerInvariant()
    if ($actual -ne $expected) {{
        Write-Error "run-eggos.ps1: $File does not match its checksum in {SUMS_FILE}: download it again"
        exit 1
    }}
    Write-Host "run-eggos.ps1: ${{File}}: checksum OK"
}} else {{
    Write-Warning "run-eggos.ps1: no {SUMS_FILE} next to this script: the checksum was not verified"
}}
$Qemu = "qemu-system-$Arch"
if (-not (Get-Command $Qemu -ErrorAction SilentlyContinue)) {{
    $installed = $null
    if ($env:ProgramFiles) {{ $installed = Join-Path $env:ProgramFiles "qemu\$Qemu.exe" }}
    if ($installed -and (Test-Path -LiteralPath $installed)) {{
        $Qemu = $installed
    }} else {{
        Write-Error "run-eggos.ps1: $Qemu not found: install QEMU (https://www.qemu.org/download/) and add it to PATH"
        exit 1
    }}
}}
$QemuArgs = switch ($Arch) {{
{cases}}}
if ($Apps) {{
    if (-not (Test-Path -LiteralPath $Apps -PathType Container)) {{
        Write-Error "run-eggos.ps1: $Apps is not a folder"
        exit 1
    }}
    $Folder = (Resolve-Path -LiteralPath $Apps).ProviderPath.Replace(',', ',,')
    $QemuArgs += switch ($Arch) {{
{folder_cases}    }}
    Write-Host "run-eggos.ps1: $Apps is at /media/host in EggOS, read-only."
}}
if (-not $NoDisk) {{
    if (-not $Disk) {{ $Disk = Join-Path $PSScriptRoot "eggos-$Arch-disk.img" }}
    $missing = -not (Test-Path -LiteralPath $Disk) -or (Get-Item -LiteralPath $Disk).Length -eq 0
    if ($FormatDisk -or $missing) {{
        $QemuImg = Join-Path (Split-Path -Parent (Get-Command $Qemu).Source) 'qemu-img.exe'
        if (Test-Path -LiteralPath $QemuImg) {{
            & $QemuImg create -q -f raw $Disk {disk_bytes}
        }} else {{
            $stream = [System.IO.File]::Open($Disk, [System.IO.FileMode]::Create)
            $stream.SetLength({disk_bytes})
            $stream.Close()
        }}
        if ($FormatDisk) {{ Write-Host "run-eggos.ps1: emptied ${{Disk}}: EggOS formats it at boot." }}
    }}
    $DiskPath = (Resolve-Path -LiteralPath $Disk).ProviderPath.Replace(',', ',,')
    $QemuArgs += switch ($Arch) {{
{disk_cases}    }}
    Write-Host "run-eggos.ps1: $Disk holds /home and /Applications in EggOS (-NoDisk: none)."
}}
if ($Serial) {{
    Write-Host "run-eggos.ps1: booting EggOS $Version ($Arch) on this terminal. Quit: Ctrl-A then X."
}} else {{
    Write-Host "run-eggos.ps1: booting EggOS $Version ($Arch) in a QEMU window; its console is here too. Quit: close the window, or Ctrl-A then X here."
}}
& $Qemu @QemuArgs -kernel $Image
exit $LASTEXITCODE
"#,
        names = names.join(", "),
        disk_bytes = qemu::DISK_IMAGE_BYTES,
        choices = arches
            .iter()
            .map(|arch| arch.name)
            .collect::<Vec<_>>()
            .join("|"),
    )
}

fn release_notes(images: &[Image]) -> String {
    let mut table = String::new();
    for image in images {
        let _ = writeln!(
            table,
            "| {} | `{}` | {} bytes ({:.0} KiB) | {} ms |",
            image.arch.name,
            image.file_name,
            image.bytes,
            image.bytes as f64 / 1024.0,
            image.boot_to_prompt.as_millis()
        );
    }
    format!(
        r#"# EggOS {VERSION}

## What's new

To be written in the release PR: what users can do now that they could not in the last release.

## Download and run

Download the image for your architecture, `{SUMS_FILE}` and the run script for your system into
one folder, then run the script. It checks the image against `{SUMS_FILE}` and boots it in QEMU,
which must be installed (`qemu-system-<arch>` on `PATH`).

| Architecture | File | Size | Boot to prompt |
|--------------|------|------|----------------|
{table}
Boot to prompt is the wall-clock time from QEMU's start to the shell's first prompt, measured
by `cargo xtask release` on the machine that built this release (QEMU without acceleration,
4 CPUs, the display attached). `cargo xtask bench` gates the same path as a deterministic
instruction count (`boot_to_prompt`) and the image sizes (`release_image_bytes`).

macOS and Linux:

```
sh run-eggos.sh            # the host's architecture, in a QEMU window
sh run-eggos.sh riscv64    # another architecture
sh run-eggos.sh --serial   # no window: the console on this terminal
```

Windows (PowerShell):

```
.\run-eggos.ps1
.\run-eggos.ps1 -Arch riscv64 -Serial
```

The window shows the EggOS desktop with the shell in a terminal window; the same session is on
the terminal the script runs in. Quit with Ctrl-A then X on that terminal, or close the window.
From a source checkout, `cargo xtask run --arch <arch>` boots the same image.

## Known limits

See [docs/limitations.md](https://github.com/eggspot/EggOS/blob/v{VERSION}/docs/limitations.md)
for what this release does not do yet.
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scripts_pass_qemu_exactly_the_arguments_cargo_xtask_run_uses() {
        let arches = script_arches();
        let shell = shell_script(&arches);
        let powershell = powershell_script(&arches);
        for arch in ARCHES {
            let window = qemu::interactive_args(arch, true);
            let serial = qemu::interactive_args(arch, false);
            assert!(shell.contains(&format!(
                "  {}:window) set -- {} ;;\n",
                arch.name,
                single_quoted(&window)
            )));
            assert!(shell.contains(&format!(
                "  {}:serial) set -- {} ;;\n",
                arch.name,
                single_quoted(&serial)
            )));
            let list = |args: &[String]| {
                args.iter()
                    .map(|arg| format!("'{arg}'"))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            assert!(powershell.contains(&format!(
                "    '{}' {{ if ($Serial) {{ @({}) }} else {{ @({}) }} }}\n",
                arch.name,
                list(&serial),
                list(&window)
            )));
        }
        for arch in ARCHES {
            let folder = qemu::host_folder_args(arch, "$folder")
                .iter()
                .map(|arg| format!("\"{arg}\""))
                .collect::<Vec<_>>()
                .join(" ");
            assert!(shell.contains(&format!("    {}) set -- \"$@\" {folder} ;;\n", arch.name)));
            let folder = qemu::host_folder_args(arch, "$Folder")
                .iter()
                .map(|arg| format!("\"{arg}\""))
                .collect::<Vec<_>>()
                .join(", ");
            assert!(powershell.contains(&format!("        '{}' {{ @({folder}) }}\n", arch.name)));
            let disk = qemu::disk_args(arch, "$disk_path")
                .iter()
                .map(|arg| format!("\"{arg}\""))
                .collect::<Vec<_>>()
                .join(" ");
            assert!(shell.contains(&format!("    {}) set -- \"$@\" {disk} ;;\n", arch.name)));
            let disk = qemu::disk_args(arch, "$DiskPath")
                .iter()
                .map(|arg| format!("\"{arg}\""))
                .collect::<Vec<_>>()
                .join(", ");
            assert!(powershell.contains(&format!("        '{}' {{ @({disk}) }}\n", arch.name)));
        }
        assert!(shell.starts_with("#!/bin/sh\n"));
        assert!(shell.contains(&format!("version='{VERSION}'")));
        assert!(powershell.contains(&format!("$Version = '{VERSION}'")));
    }

    #[test]
    fn the_scripts_carry_no_comments_but_the_shebang() {
        let arches = script_arches();
        for line in shell_script(&arches).lines().skip(1) {
            assert!(!line.trim_start().starts_with('#'), "{line}");
        }
        for line in powershell_script(&arches).lines() {
            assert!(!line.trim_start().starts_with('#'), "{line}");
            assert!(!line.contains("<#"), "{line}");
        }
    }

    #[test]
    fn sums_are_in_sha256sum_s_format() {
        let images = [Image {
            arch: &ARCHES[0],
            file_name: image_name(&ARCHES[0]),
            bytes: 3,
            sha256: sha256::hex_digest(b"abc"),
            boot_to_prompt: Duration::from_millis(55),
        }];
        assert_eq!(
            sums(&images),
            format!(
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad  eggos-{VERSION}-aarch64.elf\n"
            )
        );
        let notes = release_notes(&images);
        assert!(notes.contains(&format!(
            "| aarch64 | `eggos-{VERSION}-aarch64.elf` | 3 bytes (0 KiB) | 55 ms |"
        )));
        assert!(notes.contains("## What's new"));
    }
}
