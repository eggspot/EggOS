//! `cargo xtask release` and `cargo xtask screenshot` (D116): the files a release ships, one
//! kernel image per architecture with its initramfs embedded, `SHA256SUMS`, the run scripts and
//! the release notes' skeleton, and the README's picture of the desktop.

use crate::qmp::Picture;
use crate::{ARCHES, Arch, qemu, sha256};
use eggos_png::{Encoder, EncoderWorkspace, PngSink};
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
    let changes = merged_changes()?;
    write(
        &dir.join(NOTES_FILE),
        release_notes(&images, &changes).as_bytes(),
    )?;
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

struct PngBytes(Vec<u8>);

impl PngSink for PngBytes {
    type Error = core::convert::Infallible;

    fn write(&mut self, bytes: &[u8]) -> Result<(), Self::Error> {
        self.0.extend_from_slice(bytes);
        Ok(())
    }
}

/// `picture` as a PNG file, by the encoder `wm capture` uses (D141).
pub fn encode_png(picture: &Picture) -> Result<Vec<u8>, String> {
    let failed = |error| format!("encoding the screenshot: {error:?}");
    let mut work = Box::new(EncoderWorkspace::new());
    let mut encoder = Encoder::new(
        picture.width,
        picture.height,
        &mut work,
        PngBytes(Vec::new()),
    )
    .map_err(failed)?;
    for row in picture.pixels.chunks(picture.width.max(1) as usize) {
        encoder.write_row(row).map_err(failed)?;
    }
    Ok(encoder.finish().map_err(failed)?.0)
}

/// Boots `arch`'s release image headless and writes the desktop at the shell's prompt to
/// `docs/images/desktop.png`.
pub fn screenshot(arch: &Arch) -> Result<(), String> {
    let kernel = crate::build_kernel(arch, None)?;
    let picture = qemu::screenshot(arch, &kernel)?;
    let encoded = encode_png(&picture)?;
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

/// Writes `kernel` as `arch`'s release image, its `SHA256SUMS` line and both run scripts into
/// `dir`, as `cargo xtask release` does, for a test that runs the scripts.
pub fn write_test_dist(arch: &Arch, kernel: &Path, dir: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let contents = std::fs::read(kernel).map_err(|e| format!("{}: {e}", kernel.display()))?;
    let file_name = image_name(arch);
    write(&dir.join(&file_name), &contents)?;
    let sums = format!("{}  {file_name}\n", sha256::hex_digest(&contents));
    write(&dir.join(SUMS_FILE), sums.as_bytes())?;
    let scripts = script_arches();
    write(&dir.join(SHELL_SCRIPT), shell_script(&scripts).as_bytes())?;
    set_executable(&dir.join(SHELL_SCRIPT))?;
    write(
        &dir.join(POWERSHELL_SCRIPT),
        powershell_script(&scripts).as_bytes(),
    )?;
    Ok(dir.join(match cfg!(windows) {
        true => POWERSHELL_SCRIPT,
        false => SHELL_SCRIPT,
    }))
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
    shell_mcp: Vec<String>,
    shell_mcp_headless: Vec<String>,
    powershell_mcp: Vec<String>,
    powershell_mcp_headless: Vec<String>,
}

const SHELL_FOLDER: &str = "$folder";

const POWERSHELL_FOLDER: &str = "$Folder";

const SHELL_DISK: &str = "$disk_path";

const POWERSHELL_DISK: &str = "$DiskPath";

const SHELL_CONSOLE_LOG: &str = "$console_log";

const SHELL_GRANTS: &str = "$mcp_allow";

const POWERSHELL_GRANTS: &str = "$McpAllowList";

const POWERSHELL_CONSOLE_LOG: &str = "$ConsoleLog";

const SHELL_COMMAND_LINE: &str = "$command_line";

const POWERSHELL_COMMAND_LINE: &str = "$CommandLine";

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
            shell_mcp: qemu::mcp_args(arch, true, SHELL_CONSOLE_LOG),
            shell_mcp_headless: qemu::mcp_args(arch, false, SHELL_CONSOLE_LOG),
            powershell_mcp: qemu::mcp_args(arch, true, POWERSHELL_CONSOLE_LOG),
            powershell_mcp_headless: qemu::mcp_args(arch, false, POWERSHELL_CONSOLE_LOG),
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
            "  {}:window) set -- {} ;;\n  {}:serial) set -- {} ;;\n  {}:mcp) set -- {} ;;\n  {}:mcp-headless) set -- {} ;;",
            arch.name,
            single_quoted(&arch.window),
            arch.name,
            single_quoted(&arch.serial),
            arch.name,
            double_quoted(&arch.shell_mcp),
            arch.name,
            double_quoted(&arch.shell_mcp_headless)
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
usage="usage: run-eggos.sh [{choices}] [--serial | --mcp [--no-window] [--mcp-allow view,write,run,install]] [--apps <folder>] [--disk <image> | --no-disk] [--format-disk]"
case "$(uname -m)" in
  arm64 | aarch64) arch=aarch64 ;;
  riscv64) arch=riscv64 ;;
  *) arch=x86_64 ;;
esac
mode=window
mcp=''
no_window=''
mcp_allow=''
mcp_allow_given=''
apps=''
disk=''
no_disk=''
format_disk=''
while [ "$#" -gt 0 ]; do
  case "$1" in
    --serial) mode=serial ;;
    --mcp) mcp=yes ;;
    --no-window) no_window=yes ;;
    --mcp-allow)
      if [ "$#" -lt 2 ]; then echo "$usage" >&2; exit 2; fi
      shift
      mcp_allow="$1"
      mcp_allow_given=yes ;;
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
if [ -n "$no_window$mcp_allow_given" ] && [ -z "$mcp" ]; then
  echo "$usage" >&2
  exit 2
fi
if [ -n "$mcp_allow_given" ]; then
  case ",$mcp_allow," in
    *,,*)
      echo "run-eggos.sh: --mcp-allow takes view, write, run or install, separated by commas" >&2
      exit 2 ;;
  esac
  set -f
  old_ifs=$IFS
  IFS=,
  for grant in $mcp_allow; do
    case "$grant" in
      view | write | run | install) ;;
      *)
        echo "run-eggos.sh: --mcp-allow: unknown grant '$grant' (view, write, run or install, lowercase)" >&2
        exit 2 ;;
    esac
  done
  IFS=$old_ifs
  set +f
fi
if [ -n "$mcp" ]; then
  if [ "$mode" = serial ]; then
    echo "$usage" >&2
    exit 2
  fi
  mode=mcp
  if [ -n "$no_window" ]; then mode=mcp-headless; fi
fi
say() {{
  if [ -n "$mcp" ]; then printf '%s\n' "$*" >&2; else printf '%s\n' "$*"; fi
}}
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
    say "run-eggos.sh: $file: checksum OK"
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
console_log=$(printf '%s' "$dir/eggos-$arch-serial.log" | sed 's/,/,,/g')
case "$arch:$mode" in
{cases}esac
if [ -n "$apps" ]; then
  case "$arch" in
{folder_cases}  esac
  say "run-eggos.sh: $apps is at /media/host in EggOS, read-only."
fi
if [ -z "$no_disk" ]; then
  if [ -z "$disk" ]; then disk="$dir/eggos-$arch-disk.img"; fi
  if [ -n "$format_disk" ]; then
    : > "$disk"
    say "run-eggos.sh: emptied $disk: EggOS formats it at boot."
  fi
  if [ ! -s "$disk" ]; then
    dd if=/dev/null of="$disk" bs=1 seek={disk_bytes} >&2 2>/dev/null
  fi
  if command -v qemu-io >/dev/null 2>&1; then
    if ! probe=$(qemu-io -f raw -c quit "$disk" 2>&1 </dev/null); then
      case "$probe" in
        *lock*)
          echo "run-eggos.sh: another EggOS is using the disk image $disk: quit it first, or start this one with --disk <another image> or --no-disk." >&2
          exit 1 ;;
      esac
    fi
  fi
  disk_path=$(CDPATH= cd -- "$(dirname -- "$disk")" && pwd)/$(basename -- "$disk")
  disk_path=$(printf '%s' "$disk_path" | sed 's/,/,,/g')
  case "$arch" in
{disk_cases}  esac
  say "run-eggos.sh: $disk holds /home, /Applications and /var in EggOS (--no-disk: none)."
fi
command_line=''
if [ "$mode" = mcp-headless ]; then
  command_line='{offscreen_word}'
fi
if [ -n "$mcp_allow" ]; then
  command_line="${{command_line:+$command_line }}{grant_word}"
  say "run-eggos.sh: the MCP client may also: $mcp_allow (run can do anything a program can)."
fi
if [ -n "$command_line" ]; then
  set -- "$@" {command_line_args}
fi
case "$mode" in
  window)
    say "run-eggos.sh: booting EggOS $version ($arch) in a QEMU window; its console is here too. Quit: close the window, or Ctrl-A then X here."
    if [ "$(uname -s)" = Darwin ]; then
      say "run-eggos.sh: {MAC_HOST_KEYS}"
    fi
    ;;
  serial) say "run-eggos.sh: booting EggOS $version ($arch) on this terminal. Quit: Ctrl-A then X." ;;
  mcp-headless) say "run-eggos.sh: booting EggOS $version ($arch) with no screen for an MCP client on this script's standard input and output: EggOS draws its desktop in memory, where apps open for the agent; the console is in $dir/eggos-$arch-serial.log." ;;
  *) say "run-eggos.sh: booting EggOS $version ($arch) for an MCP client on this script's standard input and output; the console is in $dir/eggos-$arch-serial.log." ;;
esac
exec "$qemu" "$@" -kernel "$image"
"#,
        choices = names.join("|"),
        alternatives = names.join(" | "),
        disk_bytes = qemu::DISK_IMAGE_BYTES,
        offscreen_word = qemu::OFFSCREEN_WORD,
        grant_word = qemu::grant_word(SHELL_GRANTS),
        command_line_args = double_quoted(&qemu::command_line_args(SHELL_COMMAND_LINE)),
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
        let expanded = |args: &[String]| double_quoted(args).replace("\" \"", "\", \"");
        let _ = writeln!(
            cases,
            "    '{}' {{ if ($Mcp -and $NoWindow) {{ @({}) }} elseif ($Mcp) {{ @({}) }} elseif ($Serial) {{ @({}) }} else {{ @({}) }} }}",
            arch.name,
            expanded(&arch.powershell_mcp_headless),
            expanded(&arch.powershell_mcp),
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
        r#"$ErrorActionPreference = 'Stop'
$Version = '{VERSION}'
$Arches = @({names})
$Usage = "usage: run-eggos.ps1 [-Arch] <{choices}> [-Serial | -Mcp [-NoWindow] [-McpAllow view,write,run,install]] [-Apps <folder>] [-Disk <image> | -NoDisk] [-FormatDisk]"
$Arch = ''
$Serial = $false
$Mcp = $false
$NoWindow = $false
$McpAllow = $null
$Apps = ''
$Disk = ''
$NoDisk = $false
$FormatDisk = $false
$UsageError = $false
$Words = @($args)
for ($i = 0; $i -lt $Words.Count; $i++) {{
    $argument = @($Words[$i]) -join ','
    $value = $null
    if ($i + 1 -lt $Words.Count) {{ $value = @($Words[$i + 1]) -join ',' }}
    if ($argument -in @('-Serial', '--serial')) {{
        $Serial = $true
    }} elseif ($argument -in @('-Mcp', '--mcp')) {{
        $Mcp = $true
    }} elseif ($argument -in @('-NoWindow', '--no-window')) {{
        $NoWindow = $true
    }} elseif ($argument -in @('-NoDisk', '--no-disk')) {{
        $NoDisk = $true
    }} elseif ($argument -in @('-FormatDisk', '--format-disk')) {{
        $FormatDisk = $true
    }} elseif ($argument -in @('-McpAllow', '--mcp-allow') -and $null -ne $value) {{
        $McpAllow = $value
        $i++
    }} elseif ($argument -in @('-Apps', '--apps') -and $null -ne $value) {{
        $Apps = $value
        $i++
    }} elseif ($argument -in @('-Disk', '--disk') -and $null -ne $value) {{
        $Disk = $value
        $i++
    }} elseif ($argument -in @('-Arch', '--arch') -and $null -ne $value -and -not $Arch) {{
        $Arch = $value
        $i++
    }} elseif ($Arches -contains $argument -and -not $Arch) {{
        $Arch = $argument
    }} elseif ($argument -in @('-h', '-Help', '--help')) {{
        [Console]::Out.WriteLine($Usage)
        exit 0
    }} else {{
        $UsageError = $true
    }}
}}
$Arch = $Arch.ToLowerInvariant()
if ($UsageError -or ($Arch -and $Arches -notcontains $Arch) -or ($NoDisk -and ($Disk -or $FormatDisk)) -or ($NoWindow -and -not $Mcp) -or ($Mcp -and $Serial) -or (($null -ne $McpAllow) -and -not $Mcp)) {{
    [Console]::Error.WriteLine("run-eggos.ps1: $Usage")
    exit 2
}}
$McpAllowList = ''
if ($null -ne $McpAllow) {{
    $GrantNames = @(($McpAllow -join ',') -split ',')
    foreach ($GrantName in $GrantNames) {{
        if ($GrantName -cnotin @('view', 'write', 'run', 'install')) {{
            [Console]::Error.WriteLine("run-eggos.ps1: -McpAllow: unknown grant '$GrantName' (view, write, run or install, lowercase, separated by commas)")
            exit 2
        }}
    }}
    $McpAllowList = $GrantNames -join ','
}}
function Say([string]$Text) {{
    if ($Mcp) {{ [Console]::Error.WriteLine($Text) }} else {{ Write-Host $Text }}
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
    [Console]::Error.WriteLine("run-eggos.ps1: $File not found: download it next to this script")
    exit 1
}}
$Sums = Join-Path $PSScriptRoot '{SUMS_FILE}'
if (Test-Path -LiteralPath $Sums) {{
    $line = Get-Content -LiteralPath $Sums | Where-Object {{ $_.EndsWith("  $File") }} | Select-Object -First 1
    if (-not $line) {{
        [Console]::Error.WriteLine("run-eggos.ps1: $File is not listed in {SUMS_FILE}")
        exit 1
    }}
    $expected = $line.Substring(0, 64)
    $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $Image).Hash.ToLowerInvariant()
    if ($actual -ne $expected) {{
        [Console]::Error.WriteLine("run-eggos.ps1: $File does not match its checksum in {SUMS_FILE}: download it again")
        exit 1
    }}
    Say "run-eggos.ps1: ${{File}}: checksum OK"
}} else {{
    [Console]::Error.WriteLine("run-eggos.ps1: no {SUMS_FILE} next to this script: the checksum was not verified")
}}
$Qemu = "qemu-system-$Arch"
if (-not (Get-Command $Qemu -ErrorAction SilentlyContinue)) {{
    $installed = $null
    if ($env:ProgramFiles) {{ $installed = Join-Path $env:ProgramFiles "qemu\$Qemu.exe" }}
    if ($installed -and (Test-Path -LiteralPath $installed)) {{
        $Qemu = $installed
    }} else {{
        [Console]::Error.WriteLine("run-eggos.ps1: $Qemu not found: install QEMU (https://www.qemu.org/download/) and add it to PATH")
        exit 1
    }}
}}
$ConsoleLog = (Join-Path $PSScriptRoot "eggos-$Arch-serial.log").Replace(',', ',,')
$QemuArgs = switch ($Arch) {{
{cases}}}
if ($Apps) {{
    if (-not (Test-Path -LiteralPath $Apps -PathType Container)) {{
        [Console]::Error.WriteLine("run-eggos.ps1: $Apps is not a folder")
        exit 1
    }}
    $Folder = (Resolve-Path -LiteralPath $Apps).ProviderPath.Replace(',', ',,')
    $QemuArgs += switch ($Arch) {{
{folder_cases}    }}
    Say "run-eggos.ps1: $Apps is at /media/host in EggOS, read-only."
}}
if (-not $NoDisk) {{
    if (-not $Disk) {{ $Disk = Join-Path $PSScriptRoot "eggos-$Arch-disk.img" }}
    $Disk = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($Disk)
    $missing = -not (Test-Path -LiteralPath $Disk) -or (Get-Item -LiteralPath $Disk).Length -eq 0
    if ($FormatDisk -or $missing) {{
        $QemuImg = Join-Path (Split-Path -Parent (Get-Command $Qemu).Source) 'qemu-img.exe'
        $created = $false
        if (Test-Path -LiteralPath $QemuImg) {{
            & $QemuImg create -q -f raw $Disk {disk_bytes} | ForEach-Object {{ [Console]::Error.WriteLine($_) }}
            $created = $LASTEXITCODE -eq 0
        }}
        if (-not $created) {{
            $stream = [System.IO.File]::Open($Disk, [System.IO.FileMode]::Create)
            $stream.SetLength({disk_bytes})
            $stream.Close()
        }}
        if ($FormatDisk) {{ Say "run-eggos.ps1: emptied ${{Disk}}: EggOS formats it at boot." }}
    }}
    try {{
        $probe = [System.IO.File]::Open($Disk, [System.IO.FileMode]::Open, [System.IO.FileAccess]::ReadWrite, [System.IO.FileShare]::None)
        $probe.Close()
    }} catch [System.IO.IOException] {{
        [Console]::Error.WriteLine("run-eggos.ps1: another EggOS is using the disk image ${{Disk}}: quit it first, or start this one with -Disk <another image> or -NoDisk.")
        exit 1
    }}
    $DiskPath = (Resolve-Path -LiteralPath $Disk).ProviderPath.Replace(',', ',,')
    $QemuArgs += switch ($Arch) {{
{disk_cases}    }}
    Say "run-eggos.ps1: $Disk holds /home, /Applications and /var in EggOS (-NoDisk: none)."
}}
$CommandLineWords = @()
if ($Mcp -and $NoWindow) {{
    $CommandLineWords += '{offscreen_word}'
}}
if ($McpAllowList) {{
    $CommandLineWords += "{grant_word}"
    Say "run-eggos.ps1: the MCP client may also: $McpAllowList (run can do anything a program can)."
}}
if ($CommandLineWords.Count -gt 0) {{
    $CommandLine = $CommandLineWords -join ' '
    $QemuArgs += @({command_line_args})
}}
if ($Mcp -and $NoWindow) {{
    Say "run-eggos.ps1: booting EggOS $Version ($Arch) with no screen for an MCP client on this script's standard input and output: EggOS draws its desktop in memory, where apps open for the agent; the console is in $(Join-Path $PSScriptRoot "eggos-$Arch-serial.log")."
}} elseif ($Mcp) {{
    Say "run-eggos.ps1: booting EggOS $Version ($Arch) for an MCP client on this script's standard input and output; the console is in $(Join-Path $PSScriptRoot "eggos-$Arch-serial.log")."
}} elseif ($Serial) {{
    Say "run-eggos.ps1: booting EggOS $Version ($Arch) on this terminal. Quit: Ctrl-A then X."
}} else {{
    Say "run-eggos.ps1: booting EggOS $Version ($Arch) in a QEMU window; its console is here too. Quit: close the window, or Ctrl-A then X here."
}}
& $Qemu @QemuArgs -kernel $Image
exit $LASTEXITCODE
"#,
        names = names.join(", "),
        disk_bytes = qemu::DISK_IMAGE_BYTES,
        offscreen_word = qemu::OFFSCREEN_WORD,
        grant_word = qemu::grant_word(POWERSHELL_GRANTS),
        command_line_args = double_quoted(&qemu::command_line_args(POWERSHELL_COMMAND_LINE))
            .replace("\" \"", "\", \""),
        choices = arches
            .iter()
            .map(|arch| arch.name)
            .collect::<Vec<_>>()
            .join("|"),
    )
}

const REPOSITORY_URL: &str = "https://github.com/eggspot/EggOS";

/// What a Mac host keeps and what reaches EggOS: `run-eggos.sh` says it on macOS and the release
/// notes' `Keys` end with it (docs/ux/desktop-v0.3-gui.md 14.3).
pub const MAC_HOST_KEYS: &str = "Inside EggOS, Cmd is the Super key: Cmd alone opens apps, \
     Option+Tab switches windows, Option+F4 closes. Cmd+Tab, Cmd+Space and Cmd+Q stay with macOS.";

/// The QEMU release the run scripts need at least: the one CI tests with (Ubuntu 24.04's).
pub const QEMU_MINIMUM_VERSION: &str = "8.2";

fn git(args: &[&str]) -> Result<String, String> {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(crate::workspace_root())
        .output()
        .map_err(|e| format!("could not run git: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    String::from_utf8(output.stdout).map_err(|_| format!("git {} printed no UTF-8", args.join(" ")))
}

fn merged_changes() -> Result<String, String> {
    let needs_history = |error: String| {
        format!(
            "{error}: the release notes list what the pull requests merged since the last \
             release tag let users do, so they need the history and its tags (git fetch --tags \
             --unshallow)"
        )
    };
    let previous = git(&[
        "describe",
        "--tags",
        "--abbrev=0",
        "--match",
        "v[0-9]*",
        "HEAD^",
    ])
    .map_err(needs_history)?;
    let previous = previous.trim();
    let range = format!("{previous}..HEAD");
    let subjects = git(&["log", "--first-parent", "--reverse", "--format=%s", &range])
        .map_err(needs_history)?;
    Ok(whats_new(previous, subjects.lines()))
}

fn whats_new<'a>(previous: &str, subjects: impl Iterator<Item = &'a str>) -> String {
    let changes: Vec<String> = subjects.filter_map(user_change).collect();
    if changes.is_empty() {
        return format!("Nothing users can do that they could not in {previous}.\n");
    }
    let mut text = format!("Since {previous}, users can now:\n\n");
    for change in changes {
        let _ = writeln!(text, "{change}");
    }
    text
}

fn user_change(subject: &str) -> Option<String> {
    let subject = subject.strip_prefix("feat: ").unwrap_or(subject);
    let change = subject
        .strip_prefix("Users can now ")
        .or_else(|| subject.strip_prefix("users can now "))?;
    let (change, pull_request) = split_pull_request(change);
    let change = change
        .rsplit_once(" (D")
        .filter(|(_, decisions)| decisions.ends_with(')'))
        .map_or(change, |(before, _)| before);
    let mut line = format!("- {change}");
    if let Some(number) = pull_request {
        let _ = write!(line, " ([#{number}]({REPOSITORY_URL}/pull/{number}))");
    }
    Some(line)
}

fn split_pull_request(subject: &str) -> (&str, Option<&str>) {
    subject
        .strip_suffix(')')
        .and_then(|rest| rest.rsplit_once(" (#"))
        .filter(|(_, number)| !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit()))
        .map_or((subject, None), |(before, number)| (before, Some(number)))
}

fn release_notes(images: &[Image], whats_new: &str) -> String {
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

EggOS is pre-alpha: it boots in QEMU only, to try, not to use. Every process in it runs as root
until users and permissions come (M10).

## What's new

{whats_new}
## Download and run

Download the image for your architecture, `{SUMS_FILE}` and the run script for your system into
one folder, then run the script. It checks the image against `{SUMS_FILE}` and boots it in QEMU
{QEMU_MINIMUM_VERSION} or later, which must be installed (`qemu-system-<arch>` on `PATH`). QEMU
emulates the CPU (TCG, no acceleration) on every host.

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
the terminal the script runs in. `poweroff` turns EggOS off and ends QEMU, `reboot` restarts it
in the same window. Quit with Ctrl-A then X on that terminal, or close the window. Your files in
`/home` and your apps in `/Applications` are kept in `eggos-<arch>-disk.img` next to the script.
From a source checkout, `cargo xtask run --arch <arch>` boots the same image.

## What runs

- The desktop in the Windows, macOS or Linux layout (Settings picks it), the Terminal with the
  `eggsh` shell and the core utilities, Files, Settings and the Text Editor.
- Windows programs on x86_64 (x64) and aarch64 (ARM64): console programs and GUI programs on
  kernel32, user32 and gdi32, with the system's buttons, text boxes, lists, menus, timers and
  Open and Save dialogs, and C programs built by MSVC on its C runtime (`/MD`).
- macOS programs on aarch64 (arm64) and x86_64 (x86_64), for macOS 13 or later: console
  programs on libSystem, Objective-C programs on Foundation's core, and AppKit apps built in
  code, with buttons, text fields, alerts and the main menu in the top bar.
- Linux static programs on every architecture, console programs and Wayland GUI programs.

## Install an app

Pass a folder with `--apps <folder>` (`-Apps` in PowerShell): it appears read-only at
`/media/host`. Then `app install /media/host/<file>` at the prompt, or `Install from host
share...` in the launcher, installs a program with its own icon into `/Applications`, where it
survives a reboot:

- Windows programs (`.exe`, or a `.zip` holding one): x64 programs on x86_64, ARM64 programs on
  aarch64, console and GUI, on the Windows layer the x86_64 and aarch64 images carry.
- macOS programs (a Mach-O tool, an `.app`, or a `.zip` holding one, for macOS 13 or later):
  arm64 on aarch64, x86_64 on x86_64, console programs and AppKit apps built in code, on the
  macOS layer the same images carry.
- Linux static programs, console or Wayland GUI, on every architecture.

The release ships no apps of its own. To try some, build ours from a checkout with
`cargo xtask sample-apps --arch <arch>` and pass the folder it prints: `win-paint.exe`,
`win-controls.exe`, `linux-paint` and, built on a Mac, `mac-paint.zip`. See docs/apps.md.

## Keys

The layout you pick in Settings decides the habits, as on the system you came from:

- The launcher: Super (Windows and Linux layouts) or Cmd+Space (macOS layout); type a name and
  press Enter.
- Windows: Alt+Tab switches (Cmd+Tab in the macOS layout); Alt+F4 or Cmd+W closes (Ctrl+W in
  the Windows and Linux layouts); Super+Up maximises, Super+Down restores or minimises,
  Super+Left and Super+Right snap to a half, Cmd+M minimises (Super+H in the Linux layout),
  Super+D shows the desktop, Super+X opens the power menu and Alt+Space the window menu.
- The Text Editor takes Ctrl or Cmd with S (save), F (find), Z (undo), X, C and V in every
  layout.
- The terminal the script runs in: Ctrl-A then X quits QEMU.
- {MAC_HOST_KEYS}

The keyboard layout is US only.

## Connect an AI agent (MCP)

```
claude mcp add eggos -- sh /path/to/run-eggos.sh --mcp
claude mcp add eggos -- powershell -NoProfile -ExecutionPolicy Bypass -File C:\path\run-eggos.ps1 -Mcp
```

Claude Code, or any MCP client, then starts EggOS and talks to its built-in MCP server. Add
`--no-window` (`-NoWindow`) to run EggOS with no screen at all: it draws its desktop in memory,
so apps still open for the agent, which sees and operates their windows as before.

| Grant (`--mcp-allow`, `-McpAllow`) | Lets the agent |
|-------|----------------|
| none (the default) | list and read files in `/home` and `/media/host`, list processes, windows and installed apps, read the clock |
| `view` | see what any app window shows, covered and minimised ones too, passwords and messages included |
| `write` | write files below `/home/root` and move them to the Trash |
| `install` | install apps from `/media/host`, open and uninstall installed apps, open a file in the Text Editor |
| `run` | run any program, and click, type and scroll in any window, the terminal's root shell included: full control of the machine, so it implies the others |

What grants do not protect:

- Grants hold for the whole boot and for every agent: you set them when you start EggOS and
  change them by restarting it. There are no per-agent grants, and no MCP over the network.
- Every process runs as root until M10, so any app you install can do what the `run` grant
  allows, and an agent with `run` can do anything, erasing the audit log included.
- Replacing or trashing a file and uninstalling an app ask you first when the client can ask
  (Claude Code can). Otherwise the call needs `confirm: true`, which guards against an agent's
  accident and is not your approval: the agent sets it itself. Keep your client's own
  permission prompts on for these tools.
- What EggOS returns (files, names, window titles and captures) is data from the machine, not
  instructions; the server says so in every result.
- Every call is recorded in `/var/log/agent-audit.log`, which `agentlog` shows. See
  docs/mcp.md.

## Known limits

- QEMU only: no real hardware, no network, no sound, no USB; the US keyboard layout; UTC only.
- Everything is in the base for now: the desktop and the Windows and macOS layers cannot be
  left out until the component manager comes (M14).
- Linux programs must be static; programs built for another CPU and 32-bit programs are
  refused; no Android apps (`.apk`) or iOS apps; riscv64 runs no Windows or macOS programs.
- One host folder, read-only.

See [docs/limitations.md]({REPOSITORY_URL}/blob/v{VERSION}/docs/limitations.md) for everything
this release does not do yet.
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
            let expanded = |args: &[String]| {
                args.iter()
                    .map(|arg| format!("\"{arg}\""))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            assert!(powershell.contains(&format!(
                "    '{}' {{ if ($Mcp -and $NoWindow) {{ @({}) }} elseif ($Mcp) {{ @({}) }} elseif ($Serial) {{ @({}) }} else {{ @({}) }} }}\n",
                arch.name,
                expanded(&qemu::mcp_args(arch, false, "$ConsoleLog")),
                expanded(&qemu::mcp_args(arch, true, "$ConsoleLog")),
                list(&serial),
                list(&window)
            )));
            assert!(shell.contains(&format!(
                "  {}:mcp) set -- {} ;;\n  {}:mcp-headless) set -- {} ;;\n",
                arch.name,
                double_quoted(&qemu::mcp_args(arch, true, "$console_log")),
                arch.name,
                double_quoted(&qemu::mcp_args(arch, false, "$console_log"))
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
    fn with_mcp_the_scripts_print_nothing_on_standard_output() {
        let arches = script_arches();
        let shell = shell_script(&arches);
        let after_parsing = &shell[shell.find("say() {").unwrap()..];
        for line in after_parsing.lines() {
            let line = line.trim_start();
            if line.starts_with("echo ") {
                assert!(line.ends_with(">&2"), "{line}");
            }
        }
        assert!(shell.contains("printf '%s\\n' \"$*\" >&2"));
        assert!(shell.contains("--mcp) mcp=yes ;;"));
        assert!(
            shell.contains("if [ -n \"$no_window$mcp_allow_given\" ] && [ -z \"$mcp\" ]; then")
        );
        assert!(shell.contains("qemu-io -f raw -c quit \"$disk\""));
        assert!(shell.contains("another EggOS is using the disk image $disk"));
        let powershell = powershell_script(&arches);
        let after_say = &powershell[powershell.find("function Say").unwrap()..];
        assert!(!after_say.contains("Write-Host \"run-eggos"));
        assert!(!after_say.contains("Write-Warning"));
        assert!(powershell.contains("& $Qemu @QemuArgs -kernel $Image\nexit $LASTEXITCODE\n"));
        assert!(powershell.contains("[System.IO.FileShare]::None"));
    }

    #[test]
    fn the_mcp_boot_has_the_channel_on_stdio_the_console_in_a_file_and_no_monitor() {
        for arch in ARCHES {
            let args = qemu::mcp_args(arch, false, "/tmp/a,,b.log").join(" ");
            assert!(args.contains("-display none"), "{args}");
            assert!(
                args.contains(
                    "-chardev file,id=console,path=/tmp/a,,b.log -serial chardev:console"
                )
            );
            assert!(args.contains("-monitor none"));
            assert!(args.contains("-chardev stdio,id=mcp,signal=off"));
            assert!(args.ends_with(&format!(
                "-device {},id=agent -device virtserialport,bus=agent.0,nr=1,chardev=mcp,name=org.eggos.mcp",
                arch.agent_bus_device
            )));
            assert!(!args.contains("stdio,") || args.matches("stdio").count() == 1);
            assert!(
                !qemu::mcp_args(arch, true, "x")
                    .join(" ")
                    .contains("-display none")
            );
        }
    }

    #[test]
    fn grants_reach_the_kernel_command_line_only_when_valid_and_asked_for() {
        let arches = script_arches();
        let shell = shell_script(&arches);
        let powershell = powershell_script(&arches);
        assert!(shell.contains("    --mcp-allow)\n"));
        assert!(
            shell.contains("if [ -n \"$no_window$mcp_allow_given\" ] && [ -z \"$mcp\" ]; then")
        );
        assert!(shell.contains("    *,,*)\n"));
        assert!(shell.contains("      view | write | run | install) ;;\n"));
        assert!(shell.contains("set -f\n"));
        assert!(shell.contains(
            "if [ -n \"$mcp_allow\" ]; then\n  command_line=\"${command_line:+$command_line }eggos.mcp_allow=$mcp_allow\"\n"
        ));
        assert!(powershell.contains("$McpAllow = $null\n"));
        assert!(powershell.contains("$GrantNames = @(($McpAllow -join ',') -split ',')"));
        assert!(
            powershell.contains("if ($GrantName -cnotin @('view', 'write', 'run', 'install')) {")
        );
        assert!(powershell.contains("(($null -ne $McpAllow) -and -not $Mcp)"));
        assert!(powershell.contains(
            "if ($McpAllowList) {\n    $CommandLineWords += \"eggos.mcp_allow=$McpAllowList\"\n"
        ));
        assert!(powershell.contains(
            "    } elseif ($argument -in @('-McpAllow', '--mcp-allow') -and $null -ne $value) {\n"
        ));
        assert_eq!(qemu::grant_word("write,run"), "eggos.mcp_allow=write,run");
        assert_eq!(qemu::check_grants("write,run,install"), Ok(()));
        assert_eq!(qemu::check_grants("view"), Ok(()));
        for wrong in [
            "",
            "write,",
            ",run",
            "Write",
            "exec",
            "write,,run",
            "write run",
        ] {
            assert!(qemu::check_grants(wrong).is_err(), "{wrong}");
        }
    }

    #[test]
    fn the_scripts_pass_one_command_line_with_the_offscreen_word_first_then_the_grants() {
        let arches = script_arches();
        let shell = shell_script(&arches);
        let powershell = powershell_script(&arches);
        assert_eq!(shell.matches("-append").count(), 1, "{shell}");
        assert_eq!(powershell.matches("-append").count(), 1, "{powershell}");
        let built = [
            "command_line=''\n",
            "if [ \"$mode\" = mcp-headless ]; then\n  command_line='eggos.display=offscreen'\nfi\n",
            "  command_line=\"${command_line:+$command_line }eggos.mcp_allow=$mcp_allow\"\n",
            "if [ -n \"$command_line\" ]; then\n  set -- \"$@\" \"-append\" \"$command_line\"\nfi\n",
        ];
        let mut from = shell.find("IFS=$old_ifs\n  set +f\n").unwrap();
        for part in built {
            let at = shell[from..].find(part).unwrap_or_else(|| panic!("{part}"));
            from += at + part.len();
        }
        let modes = shell.find("case \"$arch:$mode\" in").unwrap();
        assert!(modes < shell.find("command_line=''").unwrap());
        assert!(
            shell.find("set -- \"$@\" \"-append\"").unwrap()
                < shell.find("exec \"$qemu\"").unwrap()
        );
        assert!(shell.contains(
            "  mcp-headless) say \"run-eggos.sh: booting EggOS $version ($arch) with no screen"
        ));
        let built = [
            "$CommandLineWords = @()\n",
            "if ($Mcp -and $NoWindow) {\n    $CommandLineWords += 'eggos.display=offscreen'\n}\n",
            "    $CommandLineWords += \"eggos.mcp_allow=$McpAllowList\"\n",
            "if ($CommandLineWords.Count -gt 0) {\n    $CommandLine = $CommandLineWords -join ' '\n    $QemuArgs += @(\"-append\", \"$CommandLine\")\n}\n",
            "& $Qemu @QemuArgs -kernel $Image\n",
        ];
        let mut from = powershell
            .find("$McpAllowList = $GrantNames -join ','")
            .unwrap();
        for part in built {
            let at = powershell[from..]
                .find(part)
                .unwrap_or_else(|| panic!("{part}"));
            from += at + part.len();
        }
    }

    #[test]
    fn a_powershell_usage_error_is_one_line_on_standard_error_and_status_2() {
        let powershell = powershell_script(&script_arches());
        assert!(!powershell.contains("Write-Error"), "{powershell}");
        assert!(!powershell.contains("CmdletBinding"));
        assert!(!powershell.contains("param("));
        assert!(powershell.contains("$Words = @($args)\n"));
        assert!(powershell.contains("    } else {\n        $UsageError = $true\n    }\n"));
        assert!(
            powershell.contains("if ($UsageError -or ($Arch -and $Arches -notcontains $Arch) -or ")
        );
        assert!(
            powershell.contains(
                "    [Console]::Error.WriteLine(\"run-eggos.ps1: $Usage\")\n    exit 2\n"
            )
        );
        let parsed = powershell.find("$Arch = $Arch.ToLowerInvariant()").unwrap();
        assert!(parsed < powershell.find("function Say").unwrap());
        for switch in ["Serial", "Mcp", "NoWindow", "NoDisk", "FormatDisk"] {
            assert!(powershell.contains(&format!("$argument -in @('-{switch}', '--")));
        }
        for option in ["McpAllow", "Apps", "Disk", "Arch"] {
            assert!(powershell.contains(&format!("$argument -in @('-{option}', '--")));
        }
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
        let notes = release_notes(&images, "Since v0.2.0, users can now:\n\n- boot\n");
        assert!(notes.contains(&format!(
            "| aarch64 | `eggos-{VERSION}-aarch64.elf` | 3 bytes (0 KiB) | 55 ms |"
        )));
        assert!(notes.contains("## What's new\n\nSince v0.2.0, users can now:\n\n- boot\n\n"));
        assert!(notes.contains("## Install an app"));
        assert!(notes.contains("cargo xtask sample-apps --arch <arch>"));
    }

    #[test]
    fn the_release_notes_have_every_section_the_release_needs() {
        let notes = release_notes(&[], "Since v0.2.0, users can now:\n\n- boot\n");
        let mut from = 0;
        for section in [
            "## What's new",
            "## Download and run",
            "## What runs",
            "## Install an app",
            "## Keys",
            "## Connect an AI agent (MCP)",
            "## Known limits",
        ] {
            let at = notes[from..]
                .find(&format!("\n{section}\n"))
                .unwrap_or_else(|| panic!("{section}"));
            from += at + section.len();
        }
        assert!(!notes.contains("To be written"));
        assert!(notes.contains(&format!("QEMU\n{QEMU_MINIMUM_VERSION} or later")));
        assert!(notes.contains("TCG, no acceleration"));
        assert!(notes.contains("runs as root"));
        assert!(notes.contains("is not your approval"));
        assert!(notes.contains("`reboot` restarts it"));
        assert!(notes.contains(&format!("- {MAC_HOST_KEYS}\n")));
        let script = shell_script(&[]);
        assert!(script.contains("if [ \"$(uname -s)\" = Darwin ]; then"));
        assert!(script.contains(&format!("say \"run-eggos.sh: {MAC_HOST_KEYS}\"")));
        for grant in ["`view`", "`write`", "`install`", "`run`"] {
            assert!(notes.contains(&format!("| {grant} |")), "{grant}");
        }
    }

    #[test]
    fn what_s_new_lists_what_merged_pull_requests_let_users_do() {
        assert_eq!(
            user_change(
                "Users can now run Windows programs with buttons (D145, v0.3.0 item 42) (#148)"
            ),
            Some(format!(
                "- run Windows programs with buttons ([#148]({REPOSITORY_URL}/pull/148))"
            ))
        );
        assert_eq!(
            user_change("feat: users can now pipe commands (D125, v0.3.0 items 22-23) (#124)"),
            Some(format!(
                "- pipe commands ([#124]({REPOSITORY_URL}/pull/124))"
            ))
        );
        assert_eq!(
            user_change("Users can now see the time"),
            Some("- see the time".to_owned())
        );
        assert_eq!(user_change("ci: parallel jobs (D122) (#121)"), None);
        assert_eq!(user_change("feat: the driver model (D110) (#110)"), None);
        let subjects = [
            "feat: execve from a file (D103, v0.3.0 item 5) (#103)",
            "Users can now keep files across reboots (D131, v0.3.0 item 25) (#129)",
            "Users can now install apps (D134) (#132)",
        ];
        assert_eq!(
            whats_new("v0.2.0", subjects.into_iter()),
            format!(
                "Since v0.2.0, users can now:\n\n\
                 - keep files across reboots ([#129]({REPOSITORY_URL}/pull/129))\n\
                 - install apps ([#132]({REPOSITORY_URL}/pull/132))\n"
            )
        );
        assert_eq!(
            whats_new("v0.2.0", subjects[..1].iter().copied()),
            "Nothing users can do that they could not in v0.2.0.\n"
        );
    }

    #[test]
    fn the_docs_tell_users_to_start_the_shell_script_with_sh_and_agree_on_the_mcp_server() {
        let root = crate::workspace_root();
        let read = |path: &str| {
            std::fs::read_to_string(root.join(path)).unwrap_or_else(|e| panic!("{path}: {e}"))
        };
        let notes = release_notes(&[], "");
        let documents = [
            ("README.md", read("README.md")),
            ("docs/mcp.md", read("docs/mcp.md")),
            ("RELEASE-NOTES.md", notes),
        ];
        for (name, text) in &documents {
            let lines: Vec<&str> = text
                .lines()
                .filter(|line| line.contains("claude mcp add") && line.contains("run-eggos.sh"))
                .collect();
            assert!(!lines.is_empty(), "{name} has no `claude mcp add` line");
            for line in lines {
                assert!(line.contains("-- sh /"), "{name}: {line}");
            }
        }
        let limitations = read("docs/limitations.md");
        assert!(!limitations.contains("No MCP server"));
        assert!(!limitations.contains("come with the MCP server"));
        let boot_claims = ["tenth of a second", "a few seconds"];
        for path in ["README.md", "docs/limitations.md", "docs/mcp.md"] {
            let text = read(path);
            for claim in boot_claims {
                assert!(!text.contains(claim), "{path}: {claim}");
            }
        }
        for path in ["README.md", "docs/limitations.md"] {
            assert!(
                read(path).contains(&format!("QEMU {QEMU_MINIMUM_VERSION} or later")),
                "{path}"
            );
        }
    }
}
