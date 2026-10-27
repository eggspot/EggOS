//! The CPU architectures the installer tells apart: the machine's, as `uname -m` names it, and
//! the one a program was built for.

/// A CPU architecture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Architecture {
    /// 64-bit x86 (`x86_64`, Windows' `x64`).
    X86_64,
    /// 64-bit Arm (`aarch64`, Windows' `ARM64`, Apple's `arm64`).
    Aarch64,
    /// 64-bit RISC-V.
    Riscv64,
    /// 32-bit x86.
    I386,
    /// 32-bit Arm.
    Arm,
    /// Anything else.
    Other,
}

impl Architecture {
    /// The machine named `machine`, as `uname` reports it.
    pub fn from_machine(machine: &[u8]) -> Self {
        match machine {
            b"x86_64" => Self::X86_64,
            b"aarch64" | b"arm64" => Self::Aarch64,
            b"riscv64" => Self::Riscv64,
            b"i386" | b"i486" | b"i586" | b"i686" => Self::I386,
            b"arm" | b"armv7l" => Self::Arm,
            _ => Self::Other,
        }
    }

    /// Its name as Linux and the manifest's `architecture=` say it.
    pub const fn name(self) -> &'static str {
        match self {
            Self::X86_64 => "x86_64",
            Self::Aarch64 => "aarch64",
            Self::Riscv64 => "riscv64",
            Self::I386 => "i386",
            Self::Arm => "arm",
            Self::Other => "an unknown CPU",
        }
    }

    /// Its name as Windows says it (`x86_64`, `ARM64`).
    pub const fn windows_name(self) -> &'static str {
        match self {
            Self::Aarch64 => "ARM64",
            other => other.name(),
        }
    }

    /// Its name as Apple says it (`x86_64`, `arm64`).
    pub const fn apple_name(self) -> &'static str {
        match self {
            Self::Aarch64 => "arm64",
            other => other.name(),
        }
    }

    /// Whether the Windows and macOS layers run on it (D118, D120, D124).
    pub const fn runs_foreign_layers(self) -> bool {
        matches!(self, Self::X86_64 | Self::Aarch64)
    }

    /// Whether it is a 64-bit architecture.
    pub const fn is_64_bit(self) -> bool {
        matches!(self, Self::X86_64 | Self::Aarch64 | Self::Riscv64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn machines_are_named_as_uname_names_them() {
        assert_eq!(Architecture::from_machine(b"x86_64"), Architecture::X86_64);
        assert_eq!(
            Architecture::from_machine(b"aarch64"),
            Architecture::Aarch64
        );
        assert_eq!(
            Architecture::from_machine(b"riscv64"),
            Architecture::Riscv64
        );
        assert_eq!(Architecture::from_machine(b"s390x"), Architecture::Other);
        assert_eq!(Architecture::Aarch64.windows_name(), "ARM64");
        assert_eq!(Architecture::Aarch64.apple_name(), "arm64");
        assert_eq!(Architecture::X86_64.apple_name(), "x86_64");
        assert!(!Architecture::Riscv64.runs_foreign_layers());
    }
}
