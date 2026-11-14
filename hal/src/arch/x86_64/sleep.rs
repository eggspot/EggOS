//! Suspend to RAM, ACPI's S3 (D597): each CPU saves its context, the boot CPU writes the
//! FACS's waking vector (the start-up trampoline) and enters S3 through PM1 control; on wake
//! the firmware runs the trampoline in real mode, which comes back here in long mode, and each
//! CPU restores its context and returns from [`save_then`] a second time. The other CPUs are
//! held first and started again through the trampoline after the boot CPU is back.

use super::{
    DIRECT_MAP_OFFSET, X86_64, acpi_table, console_input, message_interrupts, power_button,
    read_msr, remapping, table, timer, write_msr,
};
use crate::Platform;
use crate::acpi;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};

const MAX_HELD: usize = 64;
const IA32_FS_BASE: u32 = 0xc000_0100;
const IA32_GS_BASE: u32 = 0xc000_0101;
const IA32_KERNEL_GS_BASE: u32 = 0xc000_0102;
const IA32_PAT: u32 = 0x277;
const FACS_WAKING_VECTOR: usize = 12;
const FACS_X_WAKING_VECTOR: usize = 24;
const FADT_FIRMWARE_CTRL: usize = 36;
const FADT_X_FIRMWARE_CTRL: usize = 132;
const SLEEP_TIMEOUT_POLLS: u32 = 10_000_000;

/// A CPU's context while the machine sleeps. The first fields are read by the assembly below at
/// these offsets.
#[repr(C, align(64))]
pub(super) struct SavedCpu {
    stack: u64,
    page_table_root: u64,
    gs_base: u64,
    fs_base: u64,
    kernel_gs_base: u64,
    pat: u64,
    fp: FxArea,
}

#[repr(C, align(16))]
struct FxArea([u8; 512]);

struct SavedCpus([UnsafeCell<SavedCpu>; MAX_HELD]);

// SAFETY: each CPU writes only its own entry, by its APIC ID, before it sleeps and reads it when
// it wakes; another CPU reads an entry's address only, to hand it to the trampoline.
unsafe impl Sync for SavedCpus {}

static SAVED: SavedCpus = SavedCpus(
    [const {
        UnsafeCell::new(SavedCpu {
            stack: 0,
            page_table_root: 0,
            gs_base: 0,
            fs_base: 0,
            kernel_gs_base: 0,
            pat: 0,
            fp: FxArea([0; 512]),
        })
    }; MAX_HELD],
);

static HELD: [AtomicBool; MAX_HELD] = [const { AtomicBool::new(false) }; MAX_HELD];

core::arch::global_asm!(
    ".pushsection .text.sleep, \"ax\"",
    ".global eggos_save_then",
    "eggos_save_then:",
    "push rbx",
    "push rbp",
    "push r12",
    "push r13",
    "push r14",
    "push r15",
    "mov [rdi], rsp",
    "mov rax, cr3",
    "mov [rdi + 8], rax",
    "push rdi",
    "call rsi",
    "pop rdi",
    "pop r15",
    "pop r14",
    "pop r13",
    "pop r12",
    "pop rbp",
    "pop rbx",
    "xor eax, eax",
    "ret",
    ".global eggos_resume_cpu",
    "eggos_resume_cpu:",
    "mov rax, [rbx + 8]",
    "mov cr3, rax",
    "mov rsp, [rbx]",
    "lgdt [rip + {gdt_pointer}]",
    "mov ax, 0x10",
    "mov ds, ax",
    "mov es, ax",
    "mov ss, ax",
    "lea rax, [rip + 2f]",
    "push 0x08",
    "push rax",
    "retfq",
    "2:",
    "mov rdi, rbx",
    "sub rsp, 8",
    "call {restore}",
    "add rsp, 8",
    "pop r15",
    "pop r14",
    "pop r13",
    "pop r12",
    "pop rbp",
    "pop rbx",
    "mov eax, 1",
    "ret",
    ".popsection",
    restore = sym restore_cpu,
    gdt_pointer = sym RESUME_GDT_POINTER,
);

/// The GDT a waking CPU loads before it has its own: kernel code at 0x08, kernel data at 0x10,
/// as the boot GDT has them.
static RESUME_GDT: [u64; 3] = [0, 0x00af_9b00_0000_ffff, 0x00cf_9300_0000_ffff];

#[repr(C, packed)]
struct GdtPointer {
    limit: u16,
    base: u64,
}

struct ResumeGdtPointer(UnsafeCell<GdtPointer>);

// SAFETY: written once by the boot CPU before any CPU sleeps, read by waking CPUs after.
unsafe impl Sync for ResumeGdtPointer {}

static RESUME_GDT_POINTER: ResumeGdtPointer = ResumeGdtPointer(UnsafeCell::new(GdtPointer {
    limit: 0,
    base: 0,
}));

fn prepare_resume_gdt() {
    // SAFETY: written before any CPU is held or sleeps, so no waking CPU reads it meanwhile.
    unsafe {
        *RESUME_GDT_POINTER.0.get() = GdtPointer {
            limit: (core::mem::size_of_val(&RESUME_GDT) - 1) as u16,
            base: RESUME_GDT.as_ptr() as u64,
        };
    }
}

unsafe extern "C" {
    fn eggos_save_then(saved: *mut SavedCpu, then: extern "C" fn(*mut SavedCpu)) -> u64;
}

fn this_cpu_slot() -> Result<usize, &'static str> {
    let id = usize::from(timer::local_apic_id());
    (id < MAX_HELD)
        .then_some(id)
        .ok_or("a CPU whose APIC ID is 64 or more cannot sleep yet")
}

fn slot_of(cpu: u64) -> Result<usize, &'static str> {
    usize::try_from(cpu)
        .ok()
        .filter(|&id| id < MAX_HELD)
        .ok_or("a CPU whose APIC ID is 64 or more cannot sleep yet")
}

/// Saves this CPU's context in `saved`, then runs `then`; returns `false` when `then` returned
/// (the CPU did not sleep), `true` when it comes back from sleep through [`restore_cpu`].
fn save_then(saved: *mut SavedCpu, then: extern "C" fn(*mut SavedCpu)) -> bool {
    // SAFETY: `saved` is this CPU's own entry of `SAVED`, which nothing else writes; the
    // assembly saves the callee-saved registers and the stack, which `restore_cpu`'s return
    // path reloads, so this call returns a second time as if `then` had returned.
    unsafe { eggos_save_then(saved, then) != 0 }
}

fn save_extra(saved: &mut SavedCpu) {
    saved.gs_base = read_msr(IA32_GS_BASE);
    saved.fs_base = read_msr(IA32_FS_BASE);
    saved.kernel_gs_base = read_msr(IA32_KERNEL_GS_BASE);
    saved.pat = read_msr(IA32_PAT);
    // SAFETY: `fp` is a 512-byte, 16-byte-aligned area `fxsave` writes; the kernel's own FP
    // state is whatever a program left loaded, which the restore puts back.
    unsafe { core::arch::asm!("clts", "fxsave64 [{area}]", area = in(reg) saved.fp.0.as_mut_ptr(), options(nostack)) };
}

extern "C" fn restore_cpu(saved: &SavedCpu) {
    // SAFETY: these are the values the CPU had before it slept, architectural MSRs.
    unsafe {
        write_msr(IA32_PAT, saved.pat);
        write_msr(IA32_FS_BASE, saved.fs_base);
        write_msr(IA32_KERNEL_GS_BASE, saved.kernel_gs_base);
    }
    // SAFETY: the GS base is this CPU's per-CPU block, as before it slept; setting it again
    // rebuilds its GDT, TSS and `syscall` MSRs, all lost in S3.
    unsafe { X86_64::set_cpu_local(saved.gs_base as usize) };
    super::exception::load();
    timer::init_this_cpu();
    // SAFETY: as in `save_extra`: the area `fxsave` wrote before the CPU slept.
    unsafe { core::arch::asm!("clts", "fxrstor64 [{area}]", area = in(reg) saved.fp.0.as_ptr(), options(nostack)) };
}

/// On a CPU other than the boot CPU: saves its context, says so, and halts until
/// [`wake_held`] starts it again (D597). Returns once it runs again.
pub(super) fn hold() {
    let Ok(slot) = this_cpu_slot() else {
        return;
    };
    prepare_resume_gdt();
    let saved = SAVED.0[slot].get();
    // SAFETY: this CPU's own entry, which nothing else touches while it runs.
    save_extra(unsafe { &mut *saved });
    extern "C" fn halt(_: *mut SavedCpu) {
        let slot = usize::from(timer::local_apic_id());
        // SAFETY: `wbinvd` writes the caches back, so nothing this CPU changed is lost when it
        // loses power.
        unsafe { core::arch::asm!("wbinvd", options(nostack)) };
        HELD[slot].store(true, Ordering::Release);
        loop {
            // SAFETY: masked interrupts and `hlt`: the CPU waits for INIT and start-up.
            unsafe { core::arch::asm!("cli", "hlt", options(nomem, nostack)) };
        }
    }
    save_then(saved, halt);
    HELD[slot].store(false, Ordering::Release);
}

/// Whether the CPU with APIC ID `cpu` has saved its context and halted.
pub(super) fn is_held(cpu: u64) -> bool {
    slot_of(cpu).is_ok_and(|slot| HELD[slot].load(Ordering::Acquire))
}

/// Starts the held CPU `cpu` again from its saved context, through the trampoline (D597).
pub(super) fn wake_held(cpu: u64) -> Result<(), &'static str> {
    let slot = slot_of(cpu)?;
    super::resume_through_trampoline(cpu, SAVED.0[slot].get() as u64)
}

fn facs() -> Option<usize> {
    let fadt = acpi_table(b"FACP")?;
    let word = |at: usize, bytes: usize| {
        fadt.get(at..at + bytes).map(|field| {
            field
                .iter()
                .rev()
                .fold(0u64, |value, &byte| value << 8 | u64::from(byte))
        })
    };
    let address = word(FADT_X_FIRMWARE_CTRL, 8)
        .filter(|&address| address != 0)
        .or_else(|| word(FADT_FIRMWARE_CTRL, 4))
        .filter(|&address| address != 0)?;
    Some(DIRECT_MAP_OFFSET + address as usize)
}

/// Puts the machine in S3 from the boot CPU, the others held (D597): `Ok` once it has woken
/// and every part the HAL drives is set up again.
pub(super) fn suspend() -> Result<(), &'static str> {
    let fadt = acpi_table(b"FACP")
        .and_then(acpi::parse_fadt)
        .ok_or("the firmware gives no FADT")?;
    let sleep_type = table(fadt.dsdt, b"DSDT")
        .and_then(|dsdt| acpi::find_sleep_type(&dsdt[acpi::TABLE_HEADER_LENGTH..], b"_S3_"))
        .ok_or("the firmware does not offer S3")?;
    let facs = facs().ok_or("the firmware gives no FACS")?;
    super::prepare_resume_trampoline()?;
    prepare_resume_gdt();
    let slot = this_cpu_slot()?;
    let saved = SAVED.0[slot].get();
    // SAFETY: the FACS is firmware memory the direct map covers (D554); its waking vectors are
    // the operating system's to write. The 32-bit vector is the trampoline, entered in real
    // mode; the 64-bit one stays 0 so the firmware uses it.
    unsafe {
        ((facs + FACS_WAKING_VECTOR) as *mut u32).write_volatile(super::TRAMPOLINE as u32);
        ((facs + FACS_X_WAKING_VECTOR) as *mut u64).write_volatile(0);
    }
    super::set_resume_slot(saved as u64);
    // SAFETY: this CPU's own entry.
    save_extra(unsafe { &mut *saved });
    SLEEP_TARGET.store(true, Ordering::Release);
    TARGET_TYPE.store(u16::from(sleep_type.a) | u16::from(sleep_type.b) << 8, Ordering::Release);
    extern "C" fn sleep(_: *mut SavedCpu) {
        let Some(fadt) = acpi_table(b"FACP").and_then(acpi::parse_fadt) else {
            return;
        };
        let types = TARGET_TYPE.load(Ordering::Acquire);
        let sleep_type = acpi::S5SleepType {
            a: types as u8,
            b: (types >> 8) as u8,
        };
        // SAFETY: as in `hold`: the caches are written back before power goes.
        unsafe { core::arch::asm!("wbinvd", options(nostack)) };
        super::enter_sleep_state(fadt, sleep_type);
        for _ in 0..SLEEP_TIMEOUT_POLLS {
            core::hint::spin_loop();
        }
    }
    let woke = save_then(saved, sleep);
    super::set_resume_slot(0);
    if !woke {
        return Err("the machine did not sleep");
    }
    remapping::restore();
    message_interrupts::restore_pins();
    let _ = console_input::enable();
    power_button::restore();
    Ok(())
}

static SLEEP_TARGET: AtomicBool = AtomicBool::new(false);
static TARGET_TYPE: core::sync::atomic::AtomicU16 = core::sync::atomic::AtomicU16::new(0);
