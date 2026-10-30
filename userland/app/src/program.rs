//! What a file is, read from its first bytes (D134): a Windows PE32+ executable, a 64-bit
//! Mach-O executable or a fat (universal) file holding several, or a 64-bit ELF program; the
//! system it was built for and its CPU; and why this machine cannot run it, when it cannot.
//! The formats are read from their public specifications: Microsoft's PE/COFF specification,
//! Apple's Mach-O reference and the System V ELF specification.

use crate::architecture::Architecture;
use eggos_app_manifest::Source;

/// How many bytes of a file [`recognise`] reads at most.
pub const HEAD_BYTES: usize = 4096;

/// A program this machine runs: the system and the CPU it was built for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Program {
    pub source: Source,
    pub architecture: Architecture,
}

impl Program {
    /// How the installer names it: `Windows program, x86_64`, `macOS program, arm64` or
    /// `Linux program`.
    pub fn kind(&self) -> (&'static str, Option<&'static str>) {
        match self.source {
            Source::Windows => ("Windows program", Some(self.architecture.windows_name())),
            Source::Macos => ("macOS program", Some(self.architecture.apple_name())),
            Source::Linux => ("Linux program", None),
        }
    }
}

/// Why a file is no program this machine runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotRunnable {
    /// Not a format EggOS runs at all.
    NotAProgram,
    /// A library (a DLL, a dylib, a shared object without an entry), not a program.
    Library,
    /// A 32-bit program.
    ThirtyTwoBit,
    /// A Linux program that needs a dynamic loader.
    NeedsDynamicLoader,
    /// A program built for another CPU.
    OtherCpu(Architecture),
    /// A Windows or macOS program on a machine its layer does not run on.
    LayerNotOnThisCpu(Source),
    /// A Windows or macOS program whose layer is not installed (D18).
    LayerMissing(Source),
}

impl NotRunnable {
    /// Whether the reason is about this machine: the installer then says `EggOS cannot run
    /// this on <arch>: <reason>`, else `Could not install <name>: <reason>`.
    pub fn is_about_this_machine(self) -> bool {
        matches!(
            self,
            Self::ThirtyTwoBit
                | Self::NeedsDynamicLoader
                | Self::OtherCpu(_)
                | Self::LayerNotOnThisCpu(_)
        )
    }
}

fn u16_le(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        bytes.get(at..at.checked_add(2)?)?.try_into().ok()?,
    ))
}

fn u32_le(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        bytes.get(at..at.checked_add(4)?)?.try_into().ok()?,
    ))
}

fn u64_le(bytes: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(
        bytes.get(at..at.checked_add(8)?)?.try_into().ok()?,
    ))
}

fn u32_be(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(
        bytes.get(at..at.checked_add(4)?)?.try_into().ok()?,
    ))
}

mod pe {
    pub const NEW_HEADER_OFFSET: usize = 0x3c;
    pub const SIGNATURE: &[u8] = b"PE\0\0";
    pub const MACHINE: usize = 4;
    pub const CHARACTERISTICS: usize = 22;
    pub const OPTIONAL_MAGIC: usize = 24;
    pub const PE32_PLUS: u16 = 0x20b;
    pub const PE32: u16 = 0x10b;
    pub const EXECUTABLE_IMAGE: u16 = 0x0002;
    pub const DLL: u16 = 0x2000;
    pub const AMD64: u16 = 0x8664;
    pub const ARM64: u16 = 0xaa64;
    pub const I386: u16 = 0x014c;
    pub const ARMNT: u16 = 0x01c4;
    pub const RISCV64: u16 = 0x5064;
}

mod elf {
    pub const MAGIC: &[u8] = b"\x7fELF";
    pub const CLASS: usize = 4;
    pub const CLASS_64: u8 = 2;
    pub const DATA: usize = 5;
    pub const LITTLE_ENDIAN: u8 = 1;
    pub const TYPE: usize = 16;
    pub const MACHINE: usize = 18;
    pub const PROGRAM_HEADERS: usize = 32;
    pub const PROGRAM_HEADER_SIZE: usize = 54;
    pub const PROGRAM_HEADER_COUNT: usize = 56;
    pub const EXECUTABLE: u16 = 2;
    pub const SHARED: u16 = 3;
    pub const INTERPRETER: u32 = 3;
    pub const X86_64: u16 = 62;
    pub const AARCH64: u16 = 183;
    pub const RISCV: u16 = 243;
    pub const I386: u16 = 3;
    pub const ARM: u16 = 40;
}

mod mach_o {
    pub const MAGIC_64: [u8; 4] = [0xcf, 0xfa, 0xed, 0xfe];
    pub const MAGIC_32: [u8; 4] = [0xce, 0xfa, 0xed, 0xfe];
    pub const CPU_TYPE: usize = 4;
    pub const FILE_TYPE: usize = 12;
    pub const EXECUTE: u32 = 2;
    pub const X86_64: u32 = 0x0100_0007;
    pub const ARM64: u32 = 0x0100_000c;
    pub const I386: u32 = 7;
    pub const ARM: u32 = 12;
    pub const FAT_MAGIC: u32 = 0xcafe_babe;
    pub const FAT_MAGIC_64: u32 = 0xcafe_babf;
    pub const FAT_ARCH_SIZE: usize = 20;
    pub const FAT_ARCH_64_SIZE: usize = 32;
    pub const MAX_FAT_ARCHITECTURES: u32 = 32;
}

fn pe_architecture(machine: u16) -> Architecture {
    match machine {
        pe::AMD64 => Architecture::X86_64,
        pe::ARM64 => Architecture::Aarch64,
        pe::I386 => Architecture::I386,
        pe::ARMNT => Architecture::Arm,
        pe::RISCV64 => Architecture::Riscv64,
        _ => Architecture::Other,
    }
}

fn elf_architecture(machine: u16) -> Architecture {
    match machine {
        elf::X86_64 => Architecture::X86_64,
        elf::AARCH64 => Architecture::Aarch64,
        elf::RISCV => Architecture::Riscv64,
        elf::I386 => Architecture::I386,
        elf::ARM => Architecture::Arm,
        _ => Architecture::Other,
    }
}

fn mach_o_architecture(cpu: u32) -> Architecture {
    match cpu {
        mach_o::X86_64 => Architecture::X86_64,
        mach_o::ARM64 => Architecture::Aarch64,
        mach_o::I386 => Architecture::I386,
        mach_o::ARM => Architecture::Arm,
        _ => Architecture::Other,
    }
}

/// Whether a program built for `built_for` runs on `machine` through ahead-of-time translation
/// (D191): x64 Windows programs on aarch64, whose layer translates them at their first launch.
pub fn runs_translated(source: Source, built_for: Architecture, machine: Architecture) -> bool {
    matches!(
        (source, built_for, machine),
        (Source::Windows, Architecture::X86_64, Architecture::Aarch64)
    )
}

fn foreign(
    source: Source,
    built_for: Architecture,
    machine: Architecture,
) -> Result<Program, NotRunnable> {
    if !machine.runs_foreign_layers() {
        return Err(NotRunnable::LayerNotOnThisCpu(source));
    }
    if !built_for.is_64_bit() {
        return Err(NotRunnable::ThirtyTwoBit);
    }
    if built_for != machine && !runs_translated(source, built_for, machine) {
        return Err(NotRunnable::OtherCpu(built_for));
    }
    Ok(Program {
        source,
        architecture: built_for,
    })
}

fn windows(head: &[u8], machine: Architecture) -> Result<Program, NotRunnable> {
    let header = u32_le(head, pe::NEW_HEADER_OFFSET).ok_or(NotRunnable::NotAProgram)? as usize;
    let signature_end = header
        .checked_add(pe::SIGNATURE.len())
        .ok_or(NotRunnable::NotAProgram)?;
    if head.get(header..signature_end) != Some(pe::SIGNATURE) {
        return Err(NotRunnable::NotAProgram);
    }
    let field = |offset: usize| {
        header
            .checked_add(offset)
            .and_then(|at| u16_le(head, at))
            .ok_or(NotRunnable::NotAProgram)
    };
    let built_for = pe_architecture(field(pe::MACHINE)?);
    let characteristics = field(pe::CHARACTERISTICS)?;
    if characteristics & pe::DLL != 0 {
        return Err(NotRunnable::Library);
    }
    if characteristics & pe::EXECUTABLE_IMAGE == 0 {
        return Err(NotRunnable::NotAProgram);
    }
    match field(pe::OPTIONAL_MAGIC)? {
        pe::PE32_PLUS => foreign(Source::Windows, built_for, machine),
        pe::PE32 if machine.runs_foreign_layers() => Err(NotRunnable::ThirtyTwoBit),
        pe::PE32 => Err(NotRunnable::LayerNotOnThisCpu(Source::Windows)),
        _ => Err(NotRunnable::NotAProgram),
    }
}

fn linux(head: &[u8], machine: Architecture) -> Result<Program, NotRunnable> {
    if head.get(elf::DATA) != Some(&elf::LITTLE_ENDIAN) {
        return Err(NotRunnable::NotAProgram);
    }
    let kind = u16_le(head, elf::TYPE).ok_or(NotRunnable::NotAProgram)?;
    if kind != elf::EXECUTABLE && kind != elf::SHARED {
        return Err(NotRunnable::NotAProgram);
    }
    let built_for = elf_architecture(u16_le(head, elf::MACHINE).ok_or(NotRunnable::NotAProgram)?);
    if head.get(elf::CLASS) != Some(&elf::CLASS_64) {
        return Err(NotRunnable::ThirtyTwoBit);
    }
    if built_for != machine {
        return Err(NotRunnable::OtherCpu(built_for));
    }
    let first = u64_le(head, elf::PROGRAM_HEADERS).ok_or(NotRunnable::NotAProgram)?;
    let size = u16_le(head, elf::PROGRAM_HEADER_SIZE).ok_or(NotRunnable::NotAProgram)?;
    let count = u16_le(head, elf::PROGRAM_HEADER_COUNT).ok_or(NotRunnable::NotAProgram)?;
    let mut has_segments = false;
    for index in 0..u64::from(count) {
        let at = first
            .checked_add(index * u64::from(size))
            .and_then(|at| usize::try_from(at).ok())
            .ok_or(NotRunnable::NotAProgram)?;
        let kind = u32_le(head, at).ok_or(NotRunnable::NotAProgram)?;
        if kind == elf::INTERPRETER {
            return Err(NotRunnable::NeedsDynamicLoader);
        }
        has_segments = true;
    }
    match has_segments {
        true => Ok(Program {
            source: Source::Linux,
            architecture: built_for,
        }),
        false => Err(NotRunnable::NotAProgram),
    }
}

fn mac(head: &[u8], machine: Architecture) -> Result<Program, NotRunnable> {
    let built_for =
        mach_o_architecture(u32_le(head, mach_o::CPU_TYPE).ok_or(NotRunnable::NotAProgram)?);
    if u32_le(head, mach_o::FILE_TYPE) != Some(mach_o::EXECUTE) {
        return Err(NotRunnable::Library);
    }
    foreign(Source::Macos, built_for, machine)
}

fn fat(head: &[u8], machine: Architecture, entry_size: usize) -> Result<Program, NotRunnable> {
    let count = u32_be(head, 4).ok_or(NotRunnable::NotAProgram)?;
    let mut first = None;
    for index in 0..count as usize {
        let cpu = u32_be(head, 8 + index * entry_size).ok_or(NotRunnable::NotAProgram)?;
        let built_for = mach_o_architecture(cpu);
        if built_for == machine {
            return foreign(Source::Macos, built_for, machine);
        }
        first.get_or_insert(built_for);
    }
    foreign(Source::Macos, first.unwrap_or(Architecture::Other), machine)
}

fn is_fat(head: &[u8]) -> Option<usize> {
    let entry_size = match u32_be(head, 0)? {
        mach_o::FAT_MAGIC => mach_o::FAT_ARCH_SIZE,
        mach_o::FAT_MAGIC_64 => mach_o::FAT_ARCH_64_SIZE,
        _ => return None,
    };
    let count = u32_be(head, 4)?;
    (1..=mach_o::MAX_FAT_ARCHITECTURES)
        .contains(&count)
        .then_some(entry_size)
}

/// The dynamic loader a 64-bit little-endian ELF program whose first bytes are `head` names
/// (its `PT_INTERP`, D172), without its terminating NUL; `None` when it names none or the name
/// is not within `head`.
pub fn interpreter(head: &[u8]) -> Option<&[u8]> {
    if !head.starts_with(elf::MAGIC)
        || head.get(elf::CLASS) != Some(&elf::CLASS_64)
        || head.get(elf::DATA) != Some(&elf::LITTLE_ENDIAN)
    {
        return None;
    }
    let first = u64_le(head, elf::PROGRAM_HEADERS)?;
    let size = u64::from(u16_le(head, elf::PROGRAM_HEADER_SIZE)?);
    let count = u64::from(u16_le(head, elf::PROGRAM_HEADER_COUNT)?);
    for index in 0..count {
        let at = usize::try_from(first.checked_add(index.checked_mul(size)?)?).ok()?;
        if u32_le(head, at)? != elf::INTERPRETER {
            continue;
        }
        let offset = usize::try_from(u64_le(head, at.checked_add(8)?)?).ok()?;
        let length = usize::try_from(u64_le(head, at.checked_add(32)?)?).ok()?;
        let name = head.get(offset..offset.checked_add(length)?)?;
        let name = name.split(|&byte| byte == 0).next().unwrap_or(name);
        return (!name.is_empty()).then_some(name);
    }
    None
}

/// What the file whose first bytes are `head` (at most [`HEAD_BYTES`]) is, as a program for
/// a `machine` whose layers are all installed; the layer's presence is the caller's to check.
pub fn recognise(head: &[u8], machine: Architecture) -> Result<Program, NotRunnable> {
    if head.starts_with(b"MZ") {
        return windows(head, machine);
    }
    if head.starts_with(elf::MAGIC) {
        return linux(head, machine);
    }
    if head.starts_with(&mach_o::MAGIC_64) {
        return mac(head, machine);
    }
    if head.starts_with(&mach_o::MAGIC_32) {
        return match machine.runs_foreign_layers() {
            true => Err(NotRunnable::ThirtyTwoBit),
            false => Err(NotRunnable::LayerNotOnThisCpu(Source::Macos)),
        };
    }
    if let Some(entry_size) = is_fat(head) {
        return fat(head, machine, entry_size);
    }
    Err(NotRunnable::NotAProgram)
}

#[cfg(test)]
pub(crate) mod samples {
    pub fn pe(machine: u16, characteristics: u16, magic: u16) -> Vec<u8> {
        let mut file = vec![0u8; 512];
        file[..2].copy_from_slice(b"MZ");
        file[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        file[0x80..0x84].copy_from_slice(b"PE\0\0");
        file[0x84..0x86].copy_from_slice(&machine.to_le_bytes());
        file[0x96..0x98].copy_from_slice(&characteristics.to_le_bytes());
        file[0x98..0x9a].copy_from_slice(&magic.to_le_bytes());
        file
    }

    pub fn windows_program(machine: u16) -> Vec<u8> {
        pe(machine, 0x0022, 0x20b)
    }

    pub fn elf(class: u8, machine: u16, kind: u16, headers: &[u32]) -> Vec<u8> {
        let mut file = vec![0u8; 64 + 56 * headers.len()];
        file[..4].copy_from_slice(b"\x7fELF");
        file[4] = class;
        file[5] = 1;
        file[16..18].copy_from_slice(&kind.to_le_bytes());
        file[18..20].copy_from_slice(&machine.to_le_bytes());
        file[32..40].copy_from_slice(&64u64.to_le_bytes());
        file[54..56].copy_from_slice(&56u16.to_le_bytes());
        file[56..58].copy_from_slice(&(headers.len() as u16).to_le_bytes());
        for (index, kind) in headers.iter().enumerate() {
            let at = 64 + index * 56;
            file[at..at + 4].copy_from_slice(&kind.to_le_bytes());
        }
        file
    }

    pub fn linux_program(machine: u16) -> Vec<u8> {
        elf(2, machine, 2, &[1, 1])
    }

    pub fn dynamic_program(machine: u16, interpreter: &[u8]) -> Vec<u8> {
        let mut file = elf(2, machine, 3, &[6, 3, 1]);
        let offset = file.len();
        file.extend_from_slice(interpreter);
        file.push(0);
        let header = 64 + 56;
        file[header + 8..header + 16].copy_from_slice(&(offset as u64).to_le_bytes());
        let length = interpreter.len() as u64 + 1;
        file[header + 32..header + 40].copy_from_slice(&length.to_le_bytes());
        file
    }

    pub fn mach_o(cpu: u32, file_type: u32) -> Vec<u8> {
        let mut file = vec![0u8; 64];
        file[..4].copy_from_slice(&[0xcf, 0xfa, 0xed, 0xfe]);
        file[4..8].copy_from_slice(&cpu.to_le_bytes());
        file[12..16].copy_from_slice(&file_type.to_le_bytes());
        file
    }

    pub fn fat_file(cpus: &[u32]) -> Vec<u8> {
        let mut file = vec![0u8; 8 + 20 * cpus.len()];
        file[..4].copy_from_slice(&0xcafe_babeu32.to_be_bytes());
        file[4..8].copy_from_slice(&(cpus.len() as u32).to_be_bytes());
        for (index, cpu) in cpus.iter().enumerate() {
            let at = 8 + index * 20;
            file[at..at + 4].copy_from_slice(&cpu.to_be_bytes());
        }
        file
    }
}

#[cfg(test)]
mod tests {
    use super::samples::*;
    use super::*;
    use Architecture::{Aarch64, Riscv64, X86_64};

    fn program(source: Source, architecture: Architecture) -> Result<Program, NotRunnable> {
        Ok(Program {
            source,
            architecture,
        })
    }

    #[test]
    fn windows_programs_run_on_their_own_cpu_where_the_layer_runs() {
        let x64 = windows_program(0x8664);
        let arm64 = windows_program(0xaa64);
        assert_eq!(recognise(&x64, X86_64), program(Source::Windows, X86_64));
        assert_eq!(
            recognise(&arm64, Aarch64),
            program(Source::Windows, Aarch64)
        );
        assert_eq!(recognise(&x64, Aarch64), program(Source::Windows, X86_64));
        assert!(runs_translated(Source::Windows, X86_64, Aarch64));
        assert!(!runs_translated(Source::Windows, Aarch64, X86_64));
        assert!(!runs_translated(Source::Macos, X86_64, Aarch64));
        assert_eq!(
            recognise(&x64, Riscv64),
            Err(NotRunnable::LayerNotOnThisCpu(Source::Windows))
        );
        assert_eq!(
            recognise(&pe(0x8664, 0x2022, 0x20b), X86_64),
            Err(NotRunnable::Library)
        );
        assert_eq!(
            recognise(&pe(0x014c, 0x0102, 0x10b), X86_64),
            Err(NotRunnable::ThirtyTwoBit)
        );
        assert_eq!(
            recognise(&pe(0x8664, 0x0000, 0x20b), X86_64),
            Err(NotRunnable::NotAProgram)
        );
        let mut no_signature = x64.clone();
        no_signature[0x80] = b'X';
        assert_eq!(
            recognise(&no_signature, X86_64),
            Err(NotRunnable::NotAProgram)
        );
        let mut far = x64;
        far[0x3c..0x40].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(recognise(&far, X86_64), Err(NotRunnable::NotAProgram));
    }

    #[test]
    fn static_linux_programs_of_this_cpu_run_and_others_are_refused_with_a_reason() {
        assert_eq!(
            recognise(&linux_program(183), Aarch64),
            program(Source::Linux, Aarch64)
        );
        assert_eq!(
            recognise(&linux_program(243), Riscv64),
            program(Source::Linux, Riscv64)
        );
        assert_eq!(
            recognise(&elf(2, 62, 3, &[1, 3]), X86_64),
            Err(NotRunnable::NeedsDynamicLoader)
        );
        assert_eq!(
            recognise(&elf(2, 62, 3, &[6, 1]), X86_64),
            program(Source::Linux, X86_64)
        );
        assert_eq!(
            recognise(&elf(1, 3, 2, &[]), X86_64),
            Err(NotRunnable::ThirtyTwoBit)
        );
        assert_eq!(
            recognise(&linux_program(62), Riscv64),
            Err(NotRunnable::OtherCpu(X86_64))
        );
        assert_eq!(
            recognise(&elf(2, 62, 1, &[1]), X86_64),
            Err(NotRunnable::NotAProgram)
        );
        assert_eq!(
            recognise(&elf(2, 62, 2, &[]), X86_64),
            Err(NotRunnable::NotAProgram)
        );
    }

    #[test]
    fn a_dynamic_program_names_its_loader() {
        let program = dynamic_program(62, b"/lib/ld-musl-x86_64.so.1");
        assert_eq!(
            interpreter(&program),
            Some(&b"/lib/ld-musl-x86_64.so.1"[..])
        );
        assert_eq!(
            recognise(&program, X86_64),
            Err(NotRunnable::NeedsDynamicLoader)
        );
        assert_eq!(interpreter(&linux_program(62)), None);
        assert_eq!(interpreter(&program[..program.len() - 30]), None);
        let mut far = program.clone();
        far[64 + 56 + 8..64 + 56 + 16].copy_from_slice(&u64::MAX.to_le_bytes());
        assert_eq!(interpreter(&far), None);
    }

    #[test]
    fn mach_o_programs_thin_or_fat_run_where_the_layer_runs() {
        let arm = mach_o(0x0100_000c, 2);
        assert_eq!(recognise(&arm, Aarch64), program(Source::Macos, Aarch64));
        assert_eq!(recognise(&arm, X86_64), Err(NotRunnable::OtherCpu(Aarch64)));
        assert_eq!(
            recognise(&arm, Riscv64),
            Err(NotRunnable::LayerNotOnThisCpu(Source::Macos))
        );
        assert_eq!(
            recognise(&mach_o(0x0100_000c, 6), Aarch64),
            Err(NotRunnable::Library)
        );
        let universal = fat_file(&[0x0100_0007, 0x0100_000c]);
        assert_eq!(
            recognise(&universal, Aarch64),
            program(Source::Macos, Aarch64)
        );
        assert_eq!(
            recognise(&universal, X86_64),
            program(Source::Macos, X86_64)
        );
        assert_eq!(
            recognise(&fat_file(&[0x0100_0007]), Aarch64),
            Err(NotRunnable::OtherCpu(X86_64))
        );
        let java_class = [0xca, 0xfe, 0xba, 0xbe, 0, 0, 0, 52];
        assert_eq!(
            recognise(&java_class, Aarch64),
            Err(NotRunnable::NotAProgram)
        );
        assert_eq!(
            recognise(&[0xce, 0xfa, 0xed, 0xfe], X86_64),
            Err(NotRunnable::ThirtyTwoBit)
        );
    }

    #[test]
    fn anything_else_is_not_a_program_and_no_byte_string_panics() {
        for head in [&b""[..], b"#!/bin/sh\n", b"MZ", b"\x7fEL", b"plain text"] {
            assert_eq!(recognise(head, X86_64), Err(NotRunnable::NotAProgram));
        }
        let mut state = 0x2545_f491_u32;
        let seeds = [
            windows_program(0x8664),
            linux_program(62),
            mach_o(0x0100_0007, 2),
            fat_file(&[0x0100_0007, 7]),
        ];
        for round in 0..2000 {
            let mut file = seeds[round % seeds.len()].clone();
            for _ in 0..4 {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                let at = state as usize % file.len();
                file[at] = (state >> 8) as u8;
            }
            file.truncate(state as usize % (file.len() + 1));
            for machine in [X86_64, Aarch64, Riscv64] {
                let _ = recognise(&file, machine);
            }
        }
        let extremes = [
            0u64,
            1,
            u64::MAX,
            u64::MAX - 3,
            u64::MAX - 7,
            u64::from(u32::MAX),
            0x8000_0000,
        ];
        let fields: [(usize, usize, usize, bool); 6] = [
            (0, 0x3c, 4, false),
            (1, 32, 8, false),
            (1, 54, 2, false),
            (1, 56, 2, false),
            (3, 4, 4, true),
            (2, 4, 4, false),
        ];
        for (seed, at, width, big_endian) in fields {
            for value in extremes {
                let mut file = seeds[seed].clone();
                let bytes = match big_endian {
                    true => value.to_be_bytes(),
                    false => value.to_le_bytes(),
                };
                let bytes = match big_endian {
                    true => &bytes[8 - width..],
                    false => &bytes[..width],
                };
                file[at..at + width].copy_from_slice(bytes);
                for machine in [X86_64, Aarch64, Riscv64] {
                    let _ = recognise(&file, machine);
                }
            }
        }
    }

    #[test]
    fn program_headers_or_a_pe_header_past_the_end_of_memory_are_no_program() {
        for offset in [u64::MAX, u64::MAX - 3] {
            let mut file = linux_program(62);
            file[32..40].copy_from_slice(&offset.to_le_bytes());
            assert_eq!(recognise(&file, X86_64), Err(NotRunnable::NotAProgram));
        }
        let mut file = windows_program(0x8664);
        file[0x3c..0x40].copy_from_slice(&(u32::MAX - 1).to_le_bytes());
        assert_eq!(recognise(&file, X86_64), Err(NotRunnable::NotAProgram));
    }

    #[test]
    fn reasons_about_the_machine_are_told_apart() {
        assert!(NotRunnable::OtherCpu(X86_64).is_about_this_machine());
        assert!(NotRunnable::LayerNotOnThisCpu(Source::Macos).is_about_this_machine());
        assert!(!NotRunnable::NotAProgram.is_about_this_machine());
        assert!(!NotRunnable::LayerMissing(Source::Windows).is_about_this_machine());
        let windows = Program {
            source: Source::Windows,
            architecture: Aarch64,
        };
        assert_eq!(windows.kind(), ("Windows program", Some("ARM64")));
    }
}
