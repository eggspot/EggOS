//! What the installer says (docs/ux/desktop-v0.3-gui.md section 9, as D134 settles it): the
//! kind of a program, the reasons it cannot be installed or uninstalled, and the lines of the
//! command and the dialogs, written into fixed buffers.

use crate::architecture::Architecture;
use crate::archive::ArchiveError;
use crate::installer::{Plan, Problem};
use crate::program::{NotRunnable, Program};
use core::fmt::Write;
use eggos_app_manifest::Source;

/// A line of at most 256 bytes; what does not fit is dropped.
#[derive(Clone, Copy)]
pub struct Line {
    bytes: [u8; 256],
    length: usize,
}

impl core::fmt::Debug for Line {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(self.as_str(), formatter)
    }
}

impl Default for Line {
    fn default() -> Self {
        Self::new()
    }
}

impl Line {
    /// An empty line.
    pub const fn new() -> Self {
        Self {
            bytes: [0; 256],
            length: 0,
        }
    }

    /// The text.
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.length]).unwrap_or("")
    }

    /// Adds `bytes` as text: as they are when they are UTF-8, else escaped.
    pub fn push_bytes(&mut self, bytes: &[u8]) {
        match core::str::from_utf8(bytes) {
            Ok(text) => {
                let _ = self.write_str(text);
            }
            Err(_) => {
                let _ = write!(self, "{}", bytes.escape_ascii());
            }
        }
    }
}

impl Write for Line {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        let room = self.bytes.len() - self.length;
        let mut end = text.len().min(room);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        self.bytes[self.length..self.length + end].copy_from_slice(&text.as_bytes()[..end]);
        self.length += end;
        Ok(())
    }
}

fn layer(source: Source) -> &'static str {
    match source {
        Source::Windows => "Windows",
        Source::Macos => "macOS",
        Source::Linux => "Linux",
    }
}

/// How a program is named: `Windows program, x86_64`, `macOS program, arm64`, `Linux program`.
pub fn kind(program: Program) -> Line {
    let mut line = Line::new();
    match program.kind() {
        (kind, Some(architecture)) => {
            let _ = write!(line, "{kind}, {architecture}");
        }
        (kind, None) => {
            let _ = line.write_str(kind);
        }
    }
    line
}

fn not_runnable(line: &mut Line, reason: NotRunnable) {
    let _ = match reason {
        NotRunnable::NotAProgram => line.write_str("not a program EggOS can run"),
        NotRunnable::Library => line.write_str("it is a library, not a program"),
        NotRunnable::ThirtyTwoBit => {
            line.write_str("it is a 32-bit program, and EggOS runs 64-bit programs only")
        }
        NotRunnable::NeedsDynamicLoader => line.write_str(
            "it needs a dynamic loader, and EggOS runs static Linux programs only for now",
        ),
        NotRunnable::OtherCpu(architecture) => write!(
            line,
            "it is built for {}, and running programs built for another CPU is not built yet",
            architecture.name()
        ),
        NotRunnable::LayerNotOnThisCpu(source) => write!(
            line,
            "the {} layer runs on x86_64 and aarch64 only",
            layer(source)
        ),
        NotRunnable::LayerMissing(source) => write!(
            line,
            "it needs the {} layer, which is not installed",
            layer(source)
        ),
    };
}

fn archive_reason(error: ArchiveError) -> &'static str {
    match error {
        ArchiveError::NotArchive | ArchiveError::Unsupported => {
            "cannot read it: not a zip archive EggOS can open"
        }
        ArchiveError::UnsafeName => "it holds a file whose name points outside its folder",
        ArchiveError::Corrupt => "the zip archive is damaged",
        ArchiveError::Unreadable => "cannot read it: the archive could not be read",
        ArchiveError::Stopped => "its files could not be written",
    }
}

/// The reason `problem` gives, as it follows `Could not install <name>: `.
pub fn reason(problem: Problem) -> Line {
    let mut line = Line::new();
    let _ = match problem {
        Problem::NotRunnable(reason) => {
            not_runnable(&mut line, reason);
            Ok(())
        }
        Problem::Unreadable(error) => {
            let _ = line.write_str("cannot read it: ");
            line.push_bytes(error.message());
            Ok(())
        }
        Problem::FolderNotApp => line.write_str("it is a folder, not a program or an .app"),
        Problem::BundleWithoutProgram => {
            line.write_str("its Contents/MacOS does not hold its one program")
        }
        Problem::InvalidName => line.write_str(
            "an app's name is 1 to 32 printable ASCII characters, without / or ' (use --name)",
        ),
        Problem::InvalidProgramPath => {
            line.write_str("its program's path holds a quote (') or is longer than 192 bytes")
        }
        Problem::PathTooLong => line.write_str("a path in it is longer than 512 bytes"),
        Problem::AlreadyInstalled => line.write_str("it is already installed"),
        Problem::NotInstalled => line.write_str("it is not installed"),
        Problem::ReadOnly => {
            line.write_str("/Applications is read-only because EggOS started without a disk")
        }
        Problem::DiskFull => line.write_str("not enough space on the disk (ENOSPC)"),
        Problem::TooDeep => line.write_str("its folders nest more than 16 deep"),
        Problem::UnsupportedEntry => {
            line.write_str("it holds a symbolic link or a device, which an app cannot")
        }
        Problem::Archive(error) => line.write_str(archive_reason(error)),
        Problem::NoProgramInArchive => line.write_str("it holds no program EggOS can run"),
        Problem::SeveralPrograms(listing) => write!(
            line,
            "it holds several programs ({}): choose one with --entry <path>",
            listing.as_str()
        ),
        Problem::NoSuchEntry => line.write_str("it holds no program at the path --entry gives"),
        Problem::Failed(error) => {
            line.push_bytes(error.message());
            Ok(())
        }
    };
    line
}

/// Whether `problem` is about this machine, not the file.
pub fn is_about_this_machine(problem: Problem) -> bool {
    matches!(problem, Problem::NotRunnable(reason) if reason.is_about_this_machine())
}

/// What a failed install says: `EggOS cannot run <subject> on <arch>: <reason>` when the
/// machine is why, else `Could not install <name>: <reason>`.
pub fn install_failed(name: &str, subject: &str, machine: Architecture, problem: Problem) -> Line {
    let mut line = Line::new();
    let reason = reason(problem);
    let _ = match is_about_this_machine(problem) {
        true => write!(
            line,
            "EggOS cannot run {subject} on {}: {}",
            machine.name(),
            reason.as_str()
        ),
        false => write!(line, "Could not install {name}: {}", reason.as_str()),
    };
    line
}

/// What a failed uninstall says.
pub fn uninstall_failed(name: &str, problem: Problem) -> Line {
    let mut line = Line::new();
    let _ = write!(
        line,
        "Could not uninstall {name}: {}",
        reason(problem).as_str()
    );
    line
}

/// What a finished install says.
pub fn installed(plan: &Plan) -> Line {
    let mut line = Line::new();
    let _ = write!(line, "Installed {}.", plan.name());
    line
}

#[cfg(test)]
mod tests {
    use super::*;
    use eggos_runtime::errno::Errno;

    #[test]
    fn reasons_read_as_the_design_words_them() {
        let windows_on_riscv =
            Problem::NotRunnable(NotRunnable::LayerNotOnThisCpu(Source::Windows));
        assert_eq!(
            install_failed("win-paint", "this", Architecture::Riscv64, windows_on_riscv).as_str(),
            "EggOS cannot run this on riscv64: the Windows layer runs on x86_64 and aarch64 only"
        );
        assert_eq!(
            install_failed(
                "tool",
                "tool.exe",
                Architecture::Aarch64,
                Problem::NotRunnable(NotRunnable::OtherCpu(Architecture::X86_64))
            )
            .as_str(),
            "EggOS cannot run tool.exe on aarch64: it is built for x86_64, and running programs \
             built for another CPU is not built yet"
        );
        assert_eq!(
            install_failed(
                "notes",
                "this",
                Architecture::X86_64,
                Problem::NotRunnable(NotRunnable::NotAProgram)
            )
            .as_str(),
            "Could not install notes: not a program EggOS can run"
        );
        assert_eq!(
            reason(Problem::DiskFull).as_str(),
            "not enough space on the disk (ENOSPC)"
        );
        assert_eq!(
            reason(Problem::Unreadable(Errno::ENOENT)).as_str(),
            "cannot read it: No such file or directory"
        );
        assert_eq!(
            uninstall_failed("x", Problem::NotInstalled).as_str(),
            "Could not uninstall x: it is not installed"
        );
        let program = Program {
            source: Source::Macos,
            architecture: Architecture::Aarch64,
        };
        assert_eq!(kind(program).as_str(), "macOS program, arm64");
        let linux = Program {
            source: Source::Linux,
            architecture: Architecture::Riscv64,
        };
        assert_eq!(kind(linux).as_str(), "Linux program");
    }

    #[test]
    fn a_line_keeps_what_fits_and_escapes_what_is_not_text() {
        let mut line = Line::new();
        line.push_bytes(b"caf\xc3\xa9 \xff");
        assert_eq!(line.as_str(), "caf\\xc3\\xa9 \\xff");
        let mut long = Line::new();
        for _ in 0..100 {
            let _ = long.write_str("abc");
        }
        assert_eq!(long.as_str().len(), 256);
    }
}
