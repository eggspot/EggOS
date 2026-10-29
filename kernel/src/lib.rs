//! The architecture-independent EggOS kernel core.
//!
//! Nothing in this crate may depend on a particular CPU architecture or device: the machine
//! is reached only through [`eggos_hal::Platform`], so the core builds and is unit-tested on
//! the development host as well as on every bare-metal target.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod address_space;
pub mod agent_channel;
pub mod apps;
#[cfg(any(test, feature = "qemu-bench"))]
pub mod bench;
pub mod block;
#[cfg(feature = "qemu-boot-clock")]
pub mod boot_clock;
pub mod console;
pub mod cpu;
pub mod devices;
pub mod display;
pub mod elf;
pub mod exception;
pub mod heap;
pub mod init;
pub mod input;
pub mod interrupt;
pub mod linux;
pub mod memory;
pub mod net;
pub mod paging;
pub mod process;
pub mod random;
pub mod regions;
pub mod selftest;
pub mod sync;
pub mod terminal;
pub mod thread;
pub mod vfs;
pub mod wall_clock;
pub mod window;

use core::fmt::Write;
use eggos_hal::{ExitStatus, Platform};

/// The kernel heap. On the bare-metal targets it is the global allocator, so `alloc` (`Box`,
/// `Vec`, ...) works once [`paging::enable`] has run; before that, allocating fails. Host
/// builds keep the host's allocator, and tests use this heap directly.
#[cfg_attr(target_os = "none", global_allocator)]
pub static HEAP: heap::KernelHeap = heap::KernelHeap::new();

/// Kernel version, taken from the workspace version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// What a kernel does once it is up (D106), chosen by how it was built.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootMode {
    /// The release kernel: starts `/init` as process 1 ([`init`]).
    Init,
    /// A test kernel (`qemu-test`, and host tests): runs the self-tests.
    SelfTests,
    /// A benchmark kernel (`qemu-bench`): runs the self-tests, then the benchmarks.
    Benchmarks,
}

/// This kernel's [`BootMode`].
pub const BOOT_MODE: BootMode = if cfg!(feature = "qemu-bench") {
    BootMode::Benchmarks
} else if cfg!(any(test, feature = "qemu-test")) {
    BootMode::SelfTests
} else {
    BootMode::Init
};

/// Runs the architecture-independent part of boot on platform `P`, writing progress to
/// `console`, then what [`BOOT_MODE`] says.
///
/// Returns how the boot went; the architecture entry point turns that into a machine exit. A
/// release kernel returns only once init has ended, a failure.
pub fn boot<P: Platform, W: Write>(console: &mut W) -> ExitStatus {
    #[cfg(feature = "qemu-boot-clock")]
    boot_clock::start::<P>();
    cpu::init_boot_cpu::<P>();
    let free_memory = P::memory_map();
    let report = writeln!(console, "EggOS {VERSION} ({})", P::ARCH)
        .and_then(|()| writeln!(console, "clock: {} Hz", P::clock_frequency()))
        .and_then(|()| report_memory(console, &free_memory))
        .and_then(|()| report_random(console, random::seed_at_boot::<P>()))
        .and_then(|()| match paging::enable::<P>(&free_memory) {
            Ok(mut memory) => {
                writeln!(
                    console,
                    "paging: kernel page tables active, direct map at {:#x}",
                    P::direct_map_offset()
                )?;
                report_devices(console, devices::discover_and_probe::<P>(&mut memory))?;
                report_wall_clock(console, wall_clock::start::<P>())?;
                if display::mode().is_none() {
                    display::offscreen::report(
                        console,
                        display::offscreen::install_requested::<P>(),
                    )?;
                }
                report_display(console, display::show_first_frame::<P>())?;
                report_input(console, input::summary())?;
                match console::enable_input::<P>() {
                    Ok(()) => writeln!(console, "{}", console::INPUT_READY_LINE)?,
                    Err(reason) => writeln!(console, "console: no input: {reason}")?,
                }
                if let Some(report) = vfs::mount_embedded_initramfs() {
                    report_root(console, report)?;
                }
                mount_disks::<P>(console)?;
                mount_system_file_systems::<P>();
                if BOOT_MODE == BootMode::Init {
                    net::start::<P>(console)?;
                    init::run(console, &mut memory)?;
                    return Ok(false);
                }
                let passed = selftest::run(console, &selftest::all::<P>(), &mut memory)?;
                // Benchmarks only mean something on a kernel that passed its self-tests.
                #[cfg(feature = "qemu-bench")]
                if passed {
                    bench::run::<P, _>(console, &bench::all::<P>(), &mut memory)?;
                }
                Ok(passed)
            }
            Err(reason) => {
                writeln!(console, "paging: FAILED: {reason}")?;
                writeln!(console, "{}", selftest::FAIL_MARKER)?;
                Ok(false)
            }
        });
    match report {
        Ok(true) => ExitStatus::Success,
        Ok(false) | Err(_) => ExitStatus::Failure,
    }
}

const MEMORY_FILE_SYSTEMS: [(&[u8], (u32, u32)); 4] = [
    (b"/tmp", (0, 30)),
    (b"/home", (0, 31)),
    (b"/var", (0, 32)),
    (b"/run", (0, 33)),
];

fn mount_system_file_systems<P: Platform>() {
    use alloc::boxed::Box;
    let root = vfs::root();
    let exists = |path: &[u8]| vfs::resolve(root, root, path).is_ok_and(|node| node.is_directory());
    if exists(b"/proc") {
        let proc: &'static linux::proc::Proc<P> = Box::leak(Box::new(linux::proc::Proc::new()));
        let _ = vfs::mount(b"/proc", proc);
    }
    if exists(b"/dev") {
        let _ = vfs::mount(b"/dev", &vfs::devfs::DeviceFiles);
    }
    if !MEMORY_FILE_SYSTEMS
        .iter()
        .any(|(path, _)| exists(path) && !vfs::is_mount_point(path))
    {
        return;
    }
    let limit = memory::total_frames() as usize * eggos_hal::memory::FRAME_SIZE as usize / 2;
    let budget: &'static vfs::tmpfs::Budget = Box::leak(Box::new(vfs::tmpfs::Budget::new(limit)));
    for (path, device) in MEMORY_FILE_SYSTEMS {
        if !exists(path) || vfs::is_mount_point(path) {
            continue;
        }
        let permissions = if path == b"/tmp" { 0o1777 } else { 0o755 };
        if let Some(file_system) = vfs::tmpfs::Tmpfs::new(budget, device, permissions) {
            let _ = vfs::mount(path, Box::leak(Box::new(file_system)));
        }
    }
}

const HOST_FOLDER: &[u8] = b"/media/host";

const HOST_FOLDER_DEVICE: (u32, u32) = (254, 0);

const PERSISTENT_DEVICE: (u32, u32) = (254, 1);

const PERSISTENT_FOLDERS: [(&[u8], &[u8]); 3] = [
    (b"home", b"/home"),
    (b"Applications", b"/Applications"),
    (b"var", b"/var"),
];

const WORK_FOLDER_PATH: &[u8] = b"/work";

const WORK_DEVICE: (u32, u32) = (254, 2);

type BlockVolume = eggos_eggfs::Volume<vfs::eggfs::BlockDisk>;

type MountedEggFs = vfs::eggfs::EggFs<vfs::eggfs::BlockDisk>;

fn mount_disks<P: Platform>(console: &mut impl Write) -> core::fmt::Result {
    let mut persistent = false;
    let mut work = false;
    let mut host_folder = false;
    for device in block::take_all().into_iter().flatten() {
        let mut disk = vfs::eggfs::BlockDisk::new(device, devices::Deadline::after::<P>);
        let contents = eggos_eggfs::probe(&mut disk);
        let eggfs = contents == Ok(eggos_eggfs::Contents::EggFs);
        let blank_writable = contents == Ok(eggos_eggfs::Contents::Blank) && !disk.is_read_only();
        let ours = eggfs || blank_writable;
        let wanted = (eggfs && !(persistent && work)) || (blank_writable && !persistent);
        if wanted {
            match mount_eggfs::<P, _>(disk, blank_writable, console)? {
                Ok(mut volume) => {
                    let is_work = eggos_eggfs::is_work_disk(&mut volume) == Ok(true);
                    let placed = match (is_work, work, persistent) {
                        (true, false, _) => attach_work::<P, _>(volume, console)?,
                        (false, _, false) => attach_persistent::<P, _>(volume, console)?,
                        _ => Err(volume),
                    };
                    match placed {
                        Ok(()) if is_work => work = true,
                        Ok(()) => persistent = true,
                        Err(volume) => {
                            let _ = block::install(volume.into_disk().into_device());
                        }
                    }
                }
                Err(disk) => {
                    let _ = block::install(disk.into_device());
                }
            }
            continue;
        }
        let device = disk.into_device();
        if ours || host_folder {
            let _ = block::install(device);
            continue;
        }
        match mount_host_folder::<P>(device) {
            Ok(fat) => {
                host_folder = true;
                report_host_folder(console, fat)?;
            }
            Err((reason, device)) => {
                if let Some(device) = device {
                    let _ = block::install(device);
                }
                match contents {
                    Ok(eggos_eggfs::Contents::Blank) => {
                        writeln!(console, "vfs: a blank read-only disk was not mounted")?
                    }
                    _ => writeln!(
                        console,
                        "vfs: a disk holds neither EggFS nor FAT ({reason}): it was not \
                         formatted; to erase it, run with --format-disk"
                    )?,
                }
            }
        }
    }
    Ok(())
}

fn mount_eggfs<P: Platform, W: Write>(
    mut disk: vfs::eggfs::BlockDisk,
    blank: bool,
    console: &mut W,
) -> Result<Result<BlockVolume, vfs::eggfs::BlockDisk>, core::fmt::Error> {
    if blank {
        let now = linux::file_changes::now::<P>();
        if let Err(error) = eggos_eggfs::format(&mut disk, now) {
            writeln!(
                console,
                "vfs: the blank disk could not be formatted: {error:?}"
            )?;
            return Ok(Err(disk));
        }
        writeln!(console, "vfs: formatted the blank disk with EggFS")?;
    }
    let (volume, mounted) = match eggos_eggfs::Volume::mount(disk) {
        Ok(mounted) => mounted,
        Err((error, disk)) => {
            writeln!(
                console,
                "vfs: the EggFS disk was not mounted: {}",
                error.description()
            )?;
            return Ok(Err(disk));
        }
    };
    if mounted.recovered_blocks > 0 {
        writeln!(
            console,
            "vfs: EggFS restored {} blocks of its last commit from the journal",
            mounted.recovered_blocks
        )?;
    }
    Ok(Ok(volume))
}

fn mebibytes(blocks: u64) -> u64 {
    (blocks * eggos_eggfs::BLOCK_SIZE as u64) >> 20
}

fn attach_persistent<P: Platform, W: Write>(
    volume: BlockVolume,
    console: &mut W,
) -> Result<Result<(), BlockVolume>, core::fmt::Error> {
    use alloc::boxed::Box;
    let now = linux::file_changes::now::<P>();
    let (blocks, free) = volume.space();
    let sleeping = sync::Sleeping::on::<P>();
    let eggfs: &'static MountedEggFs = Box::leak(Box::new(vfs::eggfs::EggFs::new(
        volume,
        PERSISTENT_DEVICE,
        sleeping,
    )));
    let root = vfs::root();
    for (name, path) in PERSISTENT_FOLDERS {
        if !vfs::resolve(root, root, path).is_ok_and(|node| node.is_directory()) {
            continue;
        }
        match eggfs.folder(name, now) {
            Ok(folder) => {
                let _ = vfs::mount(path, Box::leak(Box::new(folder)));
            }
            Err(error) => writeln!(
                console,
                "vfs: {} was not mounted from the EggFS disk: {error:?}",
                core::str::from_utf8(path).unwrap_or("?")
            )?,
        }
    }
    writeln!(
        console,
        "vfs: EggFS disk mounted at /home, /Applications and /var ({} MiB, {} MiB free)",
        mebibytes(blocks),
        mebibytes(free)
    )?;
    start_committer::<P, _>(
        eggfs,
        "vfs: EggFS could not commit to the disk: changes are refused until the next boot",
        console,
    )?;
    Ok(Ok(()))
}

fn attach_work<P: Platform, W: Write>(
    mut volume: BlockVolume,
    console: &mut W,
) -> Result<Result<(), BlockVolume>, core::fmt::Error> {
    use alloc::boxed::Box;
    let root = vfs::root();
    if !vfs::resolve(root, root, WORK_FOLDER_PATH).is_ok_and(|node| node.is_directory()) {
        writeln!(
            console,
            "vfs: the work disk was not mounted: there is no /work to mount it on"
        )?;
        return Ok(Err(volume));
    }
    if let Err(error) = eggos_eggfs::work_folder(&mut volume) {
        writeln!(
            console,
            "vfs: the work disk was not mounted: its work folder is missing ({error:?})"
        )?;
        return Ok(Err(volume));
    }
    let now = linux::file_changes::now::<P>();
    let (blocks, free) = volume.space();
    let sleeping = sync::Sleeping::on::<P>();
    let eggfs: &'static MountedEggFs = Box::leak(Box::new(vfs::eggfs::EggFs::new(
        volume,
        WORK_DEVICE,
        sleeping,
    )));
    match eggfs.folder(eggos_eggfs::WORK_FOLDER, now) {
        Ok(folder) => {
            if vfs::mount(WORK_FOLDER_PATH, Box::leak(Box::new(folder))).is_err() {
                return writeln!(
                    console,
                    "vfs: the work disk was not mounted: no room for another mount"
                )
                .map(Ok);
            }
        }
        Err(error) => {
            return writeln!(console, "vfs: the work disk was not mounted: {error:?}").map(Ok);
        }
    }
    writeln!(
        console,
        "vfs: work disk mounted at /work ({} MiB, {} MiB free)",
        mebibytes(blocks),
        mebibytes(free)
    )?;
    start_committer::<P, _>(
        eggfs,
        "vfs: the work disk could not commit: changes to /work are refused until the next boot",
        console,
    )?;
    Ok(Ok(()))
}

fn start_committer<P: Platform, W: Write>(
    eggfs: &'static MountedEggFs,
    failure: &'static str,
    console: &mut W,
) -> core::fmt::Result {
    let committer = thread::spawn::<P, _>(move || {
        eggfs.commit_changes(&|| thread::sleep::<P>(vfs::eggfs::COMMIT_DELAY), &|| {
            let _ = writeln!(console::PlatformConsole::<P>::new(), "{failure}");
        })
    });
    if committer.is_err() {
        writeln!(
            console,
            "vfs: no committer for EggFS: changes reach the disk at sync and power-off"
        )?;
    }
    Ok(())
}

type Disk = alloc::boxed::Box<dyn block::BlockDevice>;

fn mount_host_folder<P: Platform>(
    disk: Disk,
) -> Result<&'static vfs::fat::Fat, (&'static str, Option<Disk>)> {
    use alloc::boxed::Box;
    let root = vfs::root();
    if !vfs::resolve(root, root, HOST_FOLDER).is_ok_and(|node| node.is_directory()) {
        return Err(("there is no /media/host to mount it on", Some(disk)));
    }
    let sleeping = sync::Sleeping::on::<P>();
    match vfs::fat::Fat::mount(
        disk,
        devices::Deadline::after::<P>,
        sleeping,
        HOST_FOLDER_DEVICE,
    ) {
        Ok(fat) => {
            let fat: &'static vfs::fat::Fat = Box::leak(Box::new(fat));
            vfs::mount(HOST_FOLDER, fat)
                .map(|()| fat)
                .map_err(|_| ("no room for another mount", None))
        }
        Err((error, disk)) => Err((error.description(), Some(disk))),
    }
}

fn report_host_folder<W: Write>(console: &mut W, fat: &'static vfs::fat::Fat) -> core::fmt::Result {
    write!(console, "vfs: {} disk \"", fat.fat_type().name())?;
    write_printable(console, fat.label())?;
    writeln!(console, "\" mounted read-only at /media/host")
}

fn write_printable<W: Write>(console: &mut W, bytes: &[u8]) -> core::fmt::Result {
    bytes.iter().try_for_each(|&byte| {
        let printable = byte.is_ascii_graphic() || byte == b' ';
        console.write_char(if printable { char::from(byte) } else { '?' })
    })
}

fn report_devices<W: Write>(console: &mut W, report: devices::Report) -> core::fmt::Result {
    if let Some(error) = report.discovery_error {
        writeln!(console, "devices: discovery stopped: {error}")?;
    }
    writeln!(
        console,
        "devices: {} found, {} bound, {} refused",
        report.devices, report.bound, report.refused
    )?;
    if report.dropped > 0 {
        writeln!(
            console,
            "devices: {} more not recorded: the table is full",
            report.dropped
        )?;
    }
    let mut result = Ok(());
    devices::for_each(&mut |_, device| {
        let shown =
            device.parent.is_some() || matches!(device.binding, devices::Binding::Refused(..));
        if shown && result.is_ok() {
            result = writeln!(console, "  {device}");
        }
    });
    result
}

fn report_wall_clock<W: Write>(console: &mut W, started: wall_clock::Started) -> core::fmt::Result {
    match started {
        wall_clock::Started::FromClock { name, seconds } => {
            write!(console, "clock: ")?;
            eggos_calendar::format::iso(console, seconds as i64)?;
            writeln!(console, " UTC from {name}")
        }
        wall_clock::Started::Unreadable { name, error } => writeln!(
            console,
            "clock: {name} could not be read ({}): the wall clock starts at 1970-01-01",
            error.description()
        ),
        wall_clock::Started::NoClock => writeln!(
            console,
            "clock: no real-time clock: the wall clock starts at 1970-01-01"
        ),
    }
}

fn report_display<W: Write>(
    console: &mut W,
    shown: Result<Option<display::Mode>, &'static str>,
) -> core::fmt::Result {
    match shown {
        Ok(Some(mode)) => writeln!(console, "boot: display {}x{}", mode.width, mode.height),
        Ok(None) => writeln!(console, "boot: no display"),
        Err(reason) => writeln!(console, "boot: display not shown: {reason}"),
    }
}

fn report_input<W: Write>(console: &mut W, summary: input::Summary) -> core::fmt::Result {
    let keyboard = if summary.keyboards > 0 {
        " keyboard"
    } else {
        ""
    };
    let pointer = if summary.pointers > 0 { " pointer" } else { "" };
    match (keyboard, pointer) {
        ("", "") => writeln!(console, "boot: no input"),
        _ => writeln!(console, "boot: input{keyboard}{pointer}"),
    }
}

fn report_root<W: Write>(console: &mut W, report: vfs::MountReport) -> core::fmt::Result {
    match report.error {
        None => writeln!(
            console,
            "vfs: initramfs mounted at / ({} nodes, {} bytes)",
            report.nodes, report.archive_size
        ),
        Some(error) => writeln!(console, "vfs: initramfs NOT READ ({error:?}): / is empty"),
    }
}

/// Prints how the random number generator was seeded (D56).
fn report_random<W: Write>(console: &mut W, boot_seeds: random::BootSeeds) -> core::fmt::Result {
    let random::BootSeeds {
        seeds,
        credited_bits,
        seeded,
    } = boot_seeds;
    let plural = if seeds == 1 { "" } else { "s" };
    if seeded {
        writeln!(
            console,
            "random: seeded, {credited_bits} bits credited from {seeds} seed{plural}"
        )
    } else {
        writeln!(
            console,
            "random: NOT SEEDED ({credited_bits} of {} bits credited from {seeds} seed{plural}): \
             getrandom waits, AT_RANDOM is guessable",
            random::SEEDED_BITS
        )
    }
}

/// Prints how much memory is free and where.
fn report_memory<W: Write>(console: &mut W, map: &eggos_hal::MemoryMap) -> core::fmt::Result {
    let ranges = map.ranges();
    let plural = if ranges.len() == 1 { "" } else { "s" };
    writeln!(
        console,
        "memory: {} KiB free in {} range{plural}",
        map.total() / 1024,
        ranges.len()
    )?;
    ranges
        .iter()
        .try_for_each(|range| writeln!(console, "  {range}"))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
    use eggos_hal::user::TestProgram;
    use eggos_hal::{Exception, ExceptionKind, Interrupt};

    /// A platform whose breakpoint and timer go straight to the kernel's handlers, as the
    /// architecture's vectors do on a real machine. Its clock advances one tick per read.
    pub(crate) struct TestPlatform;

    static CLOCK: AtomicU64 = AtomicU64::new(0);
    static DEADLINE: AtomicU64 = AtomicU64::new(u64::MAX);

    /// Whether the test platform's one CPU raised the wake-up interrupt on itself
    /// ([`Platform::wake_cpu`]) and has not taken it yet.
    static WAKEUP_PENDING: AtomicBool = AtomicBool::new(false);

    /// The root the test platform's last [`Platform::switch_address_space`] switched to.
    static ACTIVE_ROOT: AtomicU64 = AtomicU64::new(0);

    pub(crate) fn active_root() -> u64 {
        ACTIVE_ROOT.load(Ordering::Relaxed)
    }

    /// Overwrites the record, so a test sees whether the next switch happens.
    pub(crate) fn set_active_root(root: u64) {
        ACTIVE_ROOT.store(root, Ordering::Relaxed);
    }

    /// The test platforms' page faults that wrote have this bit in their code (as x86's error
    /// code does).
    pub(crate) const FAULT_WRITE: u64 = 1 << 1;
    /// The test platforms' faults with this bit in their code are not page faults (say, an
    /// alignment fault reported as a data fault).
    pub(crate) const NOT_A_PAGE_FAULT: u64 = 1 << 3;
    /// The test platforms' faults with this bit in their code are permission faults: the
    /// entry the CPU used raised them, so they are never stale (x86's present bit).
    pub(crate) const FAULT_PERMISSION: u64 = 1 << 0;

    /// The test platforms' `page_fault`: x86's encoding, as above.
    pub(crate) fn page_fault(
        kind: ExceptionKind,
        code: u64,
    ) -> Option<eggos_hal::paging::PageFault> {
        use eggos_hal::paging::{Access, PageFault};
        let access = match kind {
            _ if code & NOT_A_PAGE_FAULT != 0 => return None,
            ExceptionKind::InstructionFault => Access::Execute,
            ExceptionKind::DataFault if code & FAULT_WRITE != 0 => Access::Write,
            ExceptionKind::DataFault => Access::Read,
            _ => return None,
        };
        Some(PageFault {
            access,
            may_be_stale: code & FAULT_PERMISSION == 0,
        })
    }

    /// The ranges `drop_user_translations` was called with, in order.
    static DROPPED_TRANSLATIONS: std::sync::Mutex<Vec<(u64, u64)>> =
        std::sync::Mutex::new(Vec::new());

    /// The ranges the test platform dropped user translations of since the last call, which
    /// clears them.
    pub(crate) fn dropped_translations() -> Vec<(u64, u64)> {
        core::mem::take(&mut *DROPPED_TRANSLATIONS.lock().unwrap())
    }

    /// A call the scheduler made to the test platform's FP/SIMD methods (D54).
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum FpCall {
        SetUser(bool),
        Save(usize),
        Restore(usize),
    }

    /// The FP/SIMD calls, in order.
    static FP_CALLS: std::sync::Mutex<Vec<FpCall>> = std::sync::Mutex::new(Vec::new());

    /// The FP/SIMD calls the test platform received since the last call, which clears them.
    pub(crate) fn fp_calls() -> Vec<FpCall> {
        core::mem::take(&mut *FP_CALLS.lock().unwrap())
    }

    /// What the test platform's console received.
    static CONSOLE: std::sync::Mutex<Vec<u8>> = std::sync::Mutex::new(Vec::new());

    /// What the test platform's console UART holds for [`Platform::console_read`]: what the QEMU
    /// runner types, once [`Platform::enable_console_input`] says the kernel is ready. Tests that
    /// use it or the kernel's queue hold [`interrupt::lock_timer_interrupts_for_test`], as the
    /// boot test does.
    pub(crate) static CONSOLE_INPUT: std::sync::Mutex<std::collections::VecDeque<u8>> =
        std::sync::Mutex::new(std::collections::VecDeque::new());

    /// What the test platform's console received since the last call, which clears it.
    pub(crate) fn console_output() -> Vec<u8> {
        core::mem::take(&mut *CONSOLE.lock().unwrap())
    }

    /// The test platform's armed timer deadline; `u64::MAX` if none.
    pub(crate) fn timer_deadline() -> u64 {
        DEADLINE.load(Ordering::Relaxed)
    }

    /// Physical address of the test platform's RAM.
    const RAM_BASE: u64 = 0x10_0000;
    /// 16 MiB: enough for a Linux program's whole stack (8 MiB), which a self-test touches.
    const RAM_FRAMES: u64 = 4096;

    /// Real host memory standing in for the test platform's RAM: the address the platform's
    /// physical memory offset maps `RAM_BASE` to. The offset is aligned to the largest buddy
    /// block, as the real direct maps are, so physically aligned blocks are aligned virtually.
    fn ram() -> usize {
        static RAM: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
        *RAM.get_or_init(|| {
            let frame = eggos_hal::memory::FRAME_SIZE as usize;
            let block = frame << memory::MAX_ORDER;
            let memory = vec![0u8; RAM_FRAMES as usize * frame + block].leak();
            // The first address in `memory` congruent to `RAM_BASE` modulo `block`.
            let start = memory.as_mut_ptr() as usize;
            let base = RAM_BASE as usize % block;
            start + (base + block - start % block) % block
        })
    }

    impl Platform for TestPlatform {
        const ARCH: &'static str = "testarch";
        fn console_write(bytes: &[u8]) {
            CONSOLE.lock().unwrap().extend_from_slice(bytes);
        }
        fn console_read(buffer: &mut [u8]) -> usize {
            let mut input = CONSOLE_INPUT.lock().unwrap();
            let taken = buffer.len().min(input.len());
            for (slot, byte) in buffer.iter_mut().zip(input.drain(..taken)) {
                *slot = byte;
            }
            taken
        }
        fn enable_console_input() -> Result<(), &'static str> {
            CONSOLE_INPUT
                .lock()
                .unwrap()
                .extend(selftest::CONSOLE_INPUT_SCRIPT);
            Ok(())
        }
        fn exit(_status: ExitStatus) -> ! {
            unreachable!("not used in tests")
        }
        fn restart() -> ! {
            unreachable!("not used in tests")
        }
        fn halt() -> ! {
            unreachable!("not used in tests")
        }
        fn breakpoint() {
            let breakpoint = Exception {
                kind: ExceptionKind::Breakpoint,
                name: "test breakpoint",
                code: 0,
                pc: 0,
                address: None,
            };
            assert_eq!(exception::handle(&breakpoint), exception::Outcome::Resume);
        }
        fn execute_illegal_instruction() -> ! {
            unreachable!("not used in tests")
        }
        fn clock_ticks() -> u64 {
            CLOCK.fetch_add(1, Ordering::Relaxed)
        }
        fn clock_frequency() -> u64 {
            1_000_000
        }
        fn set_timer_deadline(deadline: u64) {
            DEADLINE.store(deadline, Ordering::Relaxed);
        }
        fn wait_for_interrupt() {
            if WAKEUP_PENDING.swap(false, Ordering::Relaxed) {
                interrupt::handle::<Self>(Interrupt::Wakeup);
                return;
            }
            // Sleeping until the deadline: jump the clock there and take the interrupt.
            let deadline = DEADLINE.swap(u64::MAX, Ordering::Relaxed);
            if deadline != u64::MAX {
                CLOCK.fetch_max(deadline, Ordering::Relaxed);
                interrupt::handle::<Self>(Interrupt::Timer);
            }
        }
        fn interrupt_window() {
            if WAKEUP_PENDING.swap(false, Ordering::Relaxed) {
                interrupt::handle::<Self>(Interrupt::Wakeup);
            }
            if !CONSOLE_INPUT.lock().unwrap().is_empty() {
                interrupt::handle::<Self>(Interrupt::ConsoleInput);
            }
            // Time passes; the timer interrupt is taken once its deadline has.
            let now = CLOCK.fetch_add(1, Ordering::Relaxed);
            let deadline = DEADLINE.load(Ordering::Relaxed);
            if deadline != u64::MAX && now >= deadline {
                DEADLINE.store(u64::MAX, Ordering::Relaxed);
                interrupt::handle::<Self>(Interrupt::Timer);
            }
        }
        fn memory_map() -> eggos_hal::MemoryMap {
            let mut map = eggos_hal::MemoryMap::new();
            map.add_ram(eggos_hal::PhysicalRange::from_start_length(
                RAM_BASE,
                RAM_FRAMES * eggos_hal::memory::FRAME_SIZE,
            ));
            map
        }
        fn physical_memory_offset() -> usize {
            ram().wrapping_sub(RAM_BASE as usize)
        }
        type PageTableFormat = eggos_hal::paging::x86_64::X86_64;
        fn direct_map_offset() -> usize {
            Self::physical_memory_offset()
        }
        fn kernel_image() -> eggos_hal::paging::KernelImage {
            // Below the free RAM, in RAM the direct map covers (see `physical_memory`), as on
            // real machines: the tables only describe it, nothing reaches it.
            eggos_hal::paging::KernelImage {
                virtual_offset: 0xffff_ffff_8000_0000,
                start: 0x8_0000,
                text_start: 0x8_1000,
                rodata_start: 0x8_4000,
                data_start: 0x8_6000,
                stack_guard_start: 0x8_8000,
                stack_start: 0x8_9000,
                end: 0x9_9000,
            }
        }
        fn physical_memory() -> eggos_hal::MemoryMap {
            let mut map = Self::memory_map();
            let image = Self::kernel_image();
            map.add_ram(eggos_hal::PhysicalRange {
                start: image.start,
                end: image.end,
            });
            map
        }
        fn early_device_regions(region: &mut dyn FnMut(eggos_hal::PhysicalRange)) {
            // Two register blocks in one page, as the paging code must allow.
            region(eggos_hal::PhysicalRange::from_start_length(0xa_0000, 0x100));
            region(eggos_hal::PhysicalRange::from_start_length(0xa_0800, 0x100));
        }
        unsafe fn activate_page_tables(roots: &[u64]) {
            assert_eq!(roots.len(), 1);
        }
        unsafe fn switch_address_space(roots: &[u64]) {
            ACTIVE_ROOT.store(roots[0], Ordering::Relaxed);
        }
        fn with_user_access<R>(access: impl FnOnce() -> R) -> R {
            access()
        }
        unsafe fn run_user(
            context: &mut eggos_hal::user::UserContext,
        ) -> eggos_hal::user::UserExit {
            // Plays the test program's part: its system calls, its faults and its spin.
            use eggos_hal::user::UserExit;
            let program = Self::user_test_program();
            let checks = crate::process::TEST_PROGRAM_ADDRESS + program.checks as u64;
            let null_syscalls = crate::process::TEST_PROGRAM_ADDRESS + program.null_syscalls as u64;
            let spin = crate::process::TEST_PROGRAM_ADDRESS + program.spin as u64;
            let floating_point =
                crate::process::TEST_PROGRAM_ADDRESS + program.floating_point as u64;
            let illegal_instruction =
                crate::process::TEST_PROGRAM_ADDRESS + program.illegal_instruction as u64;
            let fp_registers = crate::process::TEST_PROGRAM_ADDRESS + program.fp_registers as u64;
            let linux_hello = crate::process::TEST_PROGRAM_ADDRESS + program.linux_hello as u64;
            let linux_hello_line = crate::process::TEST_PROGRAM_ADDRESS + TEST_PROGRAM_LINE as u64;
            let linux_start = crate::process::TEST_PROGRAM_ADDRESS + program.linux_start as u64;
            let stack_growth = crate::process::TEST_PROGRAM_ADDRESS + program.stack_growth as u64;
            let linux_thread_pointer =
                crate::process::TEST_PROGRAM_ADDRESS + program.linux_thread_pointer as u64;
            let linux_poll = crate::process::TEST_PROGRAM_ADDRESS + program.linux_poll as u64;
            let linux_signals = crate::process::TEST_PROGRAM_ADDRESS + program.linux_signals as u64;
            let linux_signal_handler =
                crate::process::TEST_PROGRAM_ADDRESS + program.linux_signal_handler as u64;
            let linux_fault_handler =
                crate::process::TEST_PROGRAM_ADDRESS + program.linux_fault_handler as u64;
            let linux_signal_queue =
                crate::process::TEST_PROGRAM_ADDRESS + program.linux_signal_queue as u64;
            let linux_signal_suspend =
                crate::process::TEST_PROGRAM_ADDRESS + program.linux_signal_suspend as u64;
            let linux_clock = crate::process::TEST_PROGRAM_ADDRESS + program.linux_clock as u64;
            let linux_stat = crate::process::TEST_PROGRAM_ADDRESS + program.linux_stat as u64;
            let linux_credentials =
                crate::process::TEST_PROGRAM_ADDRESS + program.linux_credentials as u64;
            let linux_move_and_discard =
                crate::process::TEST_PROGRAM_ADDRESS + program.linux_move_and_discard as u64;
            match context.pc() {
                pc if pc == stack_growth => loop {
                    // Its stores, from the stack pointer's page down, each page's address in its
                    // first word, until one faults: the stack pointer stands in for the walk.
                    let page = context.stack() & !(eggos_hal::memory::FRAME_SIZE - 1);
                    let tables = crate::paging::PageTables::<
                        <TestPlatform as Platform>::PageTableFormat,
                    >::view_for_test(
                        active_root(), Self::physical_memory_offset()
                    );
                    match tables.translate(page) {
                        Some((phys, attributes)) if attributes.writable => {
                            let word = (phys as usize).wrapping_add(Self::physical_memory_offset());
                            // SAFETY: a writable page of the program's, a frame of the test RAM,
                            // through its offset.
                            unsafe { (word as *mut u64).write(page) };
                            *context = eggos_hal::user::UserContext::new(
                                pc,
                                page - eggos_hal::memory::FRAME_SIZE,
                            );
                        }
                        _ => {
                            return context.simulate_exception(Exception {
                                kind: ExceptionKind::DataFault,
                                name: "test page fault",
                                code: FAULT_WRITE,
                                pc,
                                address: Some(page),
                            });
                        }
                    }
                },
                pc if pc == checks => {
                    let arguments = [1, 2, 3, 4, 5, 6];
                    context.simulate_syscall(TestProgram::SYSCALL_FIRST, arguments, pc + 4);
                    UserExit::Syscall
                }
                pc if pc == checks + 4 => {
                    let arguments = [context.syscall_result(), 0, 0, 0, 0, 0];
                    context.simulate_syscall(TestProgram::SYSCALL_REPORT, arguments, pc + 4);
                    UserExit::Syscall
                }
                pc if pc == checks + 8 => context.simulate_exception(Exception {
                    kind: ExceptionKind::DataFault,
                    name: "test page fault",
                    code: 0,
                    pc,
                    address: Some(context.syscall_result()),
                }),
                pc if pc == floating_point || pc == illegal_instruction => context
                    .simulate_exception(Exception {
                        kind: ExceptionKind::IllegalInstruction,
                        name: "test illegal instruction",
                        code: 0,
                        pc,
                        address: None,
                    }),
                pc if (fp_registers..fp_registers + 3).contains(&pc) => {
                    let (number, next) = match pc - fp_registers {
                        0 => (TestProgram::SYSCALL_FIRST, pc + 1),
                        1 => (TestProgram::SYSCALL_NULL, pc + 1),
                        _ if context.syscall_result() == 0 => (TestProgram::SYSCALL_NULL, pc),
                        _ => (TestProgram::SYSCALL_REPORT, pc + 1),
                    };
                    context.simulate_syscall(number, [0; 6], next);
                    UserExit::Syscall
                }
                pc if pc == null_syscalls => {
                    context.simulate_syscall(TestProgram::SYSCALL_NULL, [0; 6], pc);
                    UserExit::Syscall
                }
                pc if pc == spin => {
                    Self::wait_for_interrupt();
                    UserExit::Interrupt
                }
                pc if (linux_start..linux_start + 64).contains(&pc) => {
                    // Its calls, from what its stack holds, one per step.
                    let (number, arguments) =
                        linux_start_calls(context.stack())[(pc - linux_start) as usize];
                    context.simulate_syscall(number, arguments, pc + 1);
                    UserExit::Syscall
                }
                pc if (linux_thread_pointer..linux_thread_pointer + 64).contains(&pc) => {
                    // getpid(), then its thread pointer set to the result << 12 and checked
                    // after each sched_yield, then exit_group(0) if it started at zero and
                    // stayed set.
                    use crate::linux::number;
                    static WRONG: AtomicU64 = AtomicU64::new(0);
                    let rounds = TestProgram::LINUX_THREAD_POINTER_ROUNDS;
                    let pointer = context.thread_pointer();
                    let (number, arguments) = match pc - linux_thread_pointer {
                        0 => {
                            WRONG.store(u64::from(pointer != 0), Ordering::Relaxed);
                            (number::GETPID, [0; 6])
                        }
                        step => {
                            if step == 1 {
                                context
                                    .set_thread_pointer(context.syscall_result() << 12)
                                    .expect("a process id shifted is a user address");
                            } else if pointer == 0 || !pointer.is_multiple_of(4096) {
                                WRONG.store(1, Ordering::Relaxed);
                            }
                            if step <= rounds {
                                (number::SCHED_YIELD, [0; 6])
                            } else {
                                let wrong = WRONG.load(Ordering::Relaxed);
                                (number::EXIT_GROUP, [wrong, 0, 0, 0, 0, 0])
                            }
                        }
                    };
                    context.simulate_syscall(number, arguments, pc + 1);
                    UserExit::Syscall
                }
                pc if (linux_poll..linux_poll + 4).contains(&pc) => {
                    // ppoll of descriptors 0 to 2 (POLLOUT on 1) with no wait, ioctl(1,
                    // TIOCGWINSZ), ppoll of nothing for LINUX_POLL_WAIT_NS, then exit_group(0)
                    // if each answered as the program checks; its structures below the stack
                    // pointer.
                    use crate::linux::number;
                    static WRONG: AtomicU64 = AtomicU64::new(0);
                    let stack = context.stack();
                    let (fds, timeout, size) = (stack - 32, stack - 48, stack - 64);
                    let result = context.syscall_result();
                    let (number, arguments) = match pc - linux_poll {
                        0 => {
                            let mut fds_bytes = [0u8; 24];
                            fds_bytes[8] = 1;
                            fds_bytes[12] = 4;
                            fds_bytes[16] = 2;
                            write_user_bytes(fds, &fds_bytes);
                            write_user_bytes(timeout, &[0; 16]);
                            (number::PPOLL, [fds, 3, timeout, 0, 0, 0])
                        }
                        1 => {
                            let revents = |entry: u64| {
                                let mut bytes = [0u8; 2];
                                read_user_bytes(fds + 8 * entry + 6, &mut bytes);
                                u16::from_le_bytes(bytes)
                            };
                            let answered =
                                result == 1 && [revents(0), revents(1), revents(2)] == [0, 4, 0];
                            WRONG.store(u64::from(!answered), Ordering::Relaxed);
                            (number::IOCTL, [1, 0x5413, size, 0, 0, 0])
                        }
                        2 => {
                            WRONG.fetch_or(u64::from(result != 0), Ordering::Relaxed);
                            let mut wait = [0u8; 16];
                            wait[8..]
                                .copy_from_slice(&TestProgram::LINUX_POLL_WAIT_NS.to_le_bytes());
                            write_user_bytes(timeout, &wait);
                            (number::PPOLL, [0, 0, timeout, 0, 0, 0])
                        }
                        _ => {
                            let mut remaining = [0xffu8; 16];
                            read_user_bytes(timeout, &mut remaining);
                            let wrong = WRONG.load(Ordering::Relaxed)
                                | u64::from(result != 0 || remaining != [0; 16]);
                            (number::EXIT_GROUP, [wrong, 0, 0, 0, 0, 0])
                        }
                    };
                    context.simulate_syscall(number, arguments, pc + 1);
                    UserExit::Syscall
                }
                pc if (linux_signals..linux_signals + 7).contains(&pc) => {
                    // kill(0, SIGCHLD), rt_sigprocmask(SIG_BLOCK, SIGUSR1), getpid,
                    // tgkill(pid, pid, SIGUSR1), rt_sigpending, rt_sigprocmask(SIG_UNBLOCK,
                    // SIGUSR1), which must end it, else exit_group(1); each call's result
                    // checked, as the program checks it.
                    use crate::linux::number;
                    let stack = context.stack();
                    let (set, pending) = (stack - 16, stack - 32);
                    let result = context.syscall_result();
                    let usr1 = 1u64 << 9;
                    let step = pc - linux_signals;
                    let failed = match step {
                        1 | 2 | 4 | 5 => result != 0,
                        _ => false,
                    } || step == 5 && {
                        let mut bytes = [0u8; 8];
                        read_user_bytes(pending, &mut bytes);
                        u64::from_ne_bytes(bytes) != usr1
                    };
                    let (number, arguments) = match step {
                        _ if failed || step == 6 => (number::EXIT_GROUP, [1, 0, 0, 0, 0, 0]),
                        0 => (number::KILL, [0, 17, 0, 0, 0, 0]),
                        1 => {
                            write_user_bytes(set, &usr1.to_ne_bytes());
                            (number::RT_SIGPROCMASK, [0, set, 0, 8, 0, 0])
                        }
                        2 => (number::GETPID, [0; 6]),
                        3 => (number::TGKILL, [result, result, 10, 0, 0, 0]),
                        4 => (number::RT_SIGPENDING, [pending, 8, 0, 0, 0, 0]),
                        _ => (number::RT_SIGPROCMASK, [1, set, 0, 8, 0, 0]),
                    };
                    context.simulate_syscall(number, arguments, pc + 1);
                    UserExit::Syscall
                }
                pc if (linux_signal_handler..linux_signal_handler + 6).contains(&pc)
                    || pc == linux_signal_handler + SIGNAL_HANDLER =>
                {
                    signal_handler_program(context, linux_signal_handler)
                }
                pc if (linux_fault_handler..linux_fault_handler + 5).contains(&pc)
                    || pc == linux_fault_handler + SIGNAL_HANDLER =>
                {
                    fault_handler_program(context, linux_fault_handler)
                }
                pc if (linux_signal_queue..linux_signal_queue + 10).contains(&pc) => {
                    signal_queue_program(context, linux_signal_queue)
                }
                pc if (linux_signal_suspend..linux_signal_suspend + 7).contains(&pc)
                    || pc == linux_signal_suspend + SIGNAL_HANDLER =>
                {
                    signal_suspend_program(context, linux_signal_suspend)
                }
                pc if (linux_clock..linux_clock + 5).contains(&pc) => {
                    clock_program(context, linux_clock)
                }
                pc if (linux_stat..linux_stat + 6).contains(&pc) => {
                    stat_program(context, linux_stat)
                }
                pc if (linux_credentials..linux_credentials + 5).contains(&pc) => {
                    credentials_program(context, linux_credentials)
                }
                pc if (linux_move_and_discard..linux_move_and_discard + 6).contains(&pc) => {
                    move_and_discard_program(context, linux_move_and_discard)
                }
                pc if (linux_hello..linux_hello + 4).contains(&pc) => {
                    // write(1, the line, its length), write(1, 0, 1), an unknown call, then
                    // exit_group(0) if each returned what Linux returns.
                    use crate::linux::{Errno, number};
                    static WRONG: AtomicU64 = AtomicU64::new(0);
                    let hello = TestProgram::LINUX_HELLO.len() as u64;
                    let result = context.syscall_result();
                    let (number, arguments) = match pc - linux_hello {
                        0 => (number::WRITE, [1, linux_hello_line, hello, 0, 0, 0]),
                        1 => {
                            WRONG.store(u64::from(result != hello), Ordering::Relaxed);
                            (number::WRITE, [1, 0, 1, 0, 0, 0])
                        }
                        2 => {
                            WRONG.fetch_or(
                                u64::from(result != Errno::EFAULT.result()),
                                Ordering::Relaxed,
                            );
                            (TestProgram::LINUX_UNKNOWN_SYSCALL, [0; 6])
                        }
                        _ => {
                            let wrong = WRONG.load(Ordering::Relaxed)
                                | u64::from(result != Errno::ENOSYS.result());
                            (number::EXIT_GROUP, [wrong, 0, 0, 0, 0, 0])
                        }
                    };
                    context.simulate_syscall(number, arguments, pc + 1);
                    UserExit::Syscall
                }
                _ => unreachable!("the test platform runs only the test program"),
            }
        }
        fn user_test_program() -> eggos_hal::user::TestProgram {
            eggos_hal::user::TestProgram {
                code: &TEST_PROGRAM_CODE,
                checks: 0,
                floating_point: 20,
                fp_registers: 28,
                illegal_instruction: 32,
                null_syscalls: 12,
                spin: 16,
                linux_hello: 21,
                linux_start: 100,
                // The whole code: the simulated programs run at their offsets, past it.
                linux_start_end: 64,
                linux_unknown_syscalls: 26,
                stack_growth: 27,
                linux_thread_pointer: 200,
                linux_poll: 264,
                linux_signals: 300,
                linux_signal_handler: 340,
                linux_fault_handler: 400,
                linux_signal_queue: 460,
                linux_signal_suspend: 520,
                linux_clock: 580,
                linux_stat: 640,
                linux_credentials: 700,
                linux_move_and_discard: 760,
            }
        }
        fn linux_hardware_capabilities() -> [u64; 2] {
            [0, 0]
        }
        fn make_instructions_visible(_start: usize, _length: usize) {}
        fn drop_user_translations(start: u64, length: u64) {
            DROPPED_TRANSLATIONS.lock().unwrap().push((start, length));
        }
        fn page_fault(
            kind: eggos_hal::ExceptionKind,
            code: u64,
        ) -> Option<eggos_hal::paging::PageFault> {
            crate::tests::page_fault(kind, code)
        }
        unsafe fn prepare_thread(
            _stack_top: usize,
            entry: extern "C" fn(usize) -> !,
            argument: usize,
        ) -> usize {
            thread::host::prepare_thread(entry, argument)
        }
        unsafe fn switch_thread(save: *mut usize, resume: usize) {
            // SAFETY: the caller's guarantees are the host switch's.
            unsafe { thread::host::switch_thread(save, resume) }
        }
        fn random_seeds(seed: &mut dyn FnMut(eggos_hal::random::SeedSource, &[u8])) {
            seed(eggos_hal::random::SeedSource::CpuInstruction, &[0x5e; 32]);
        }
        unsafe fn set_user_fp(on: bool) {
            FP_CALLS.lock().unwrap().push(FpCall::SetUser(on));
        }
        unsafe fn save_fp(state: *mut eggos_hal::fp::FpState) {
            FP_CALLS.lock().unwrap().push(FpCall::Save(state as usize));
        }
        unsafe fn restore_fp(state: *const eggos_hal::fp::FpState) {
            FP_CALLS
                .lock()
                .unwrap()
                .push(FpCall::Restore(state as usize));
        }
        fn cpus(cpu: &mut dyn FnMut(u64)) {
            cpu(0);
        }
        fn boot_cpu() -> u64 {
            0
        }
        unsafe fn start_cpu(
            _cpu: u64,
            _start: &eggos_hal::cpu::CpuStart,
        ) -> Result<(), eggos_hal::cpu::StartCpuError> {
            unreachable!("the test platform has one CPU")
        }
        unsafe fn set_cpu_local(block: usize) {
            CPU_LOCAL.store(block, Ordering::Relaxed);
        }
        fn cpu_local() -> usize {
            // The boot CPU's block from the first use on, as on a machine that booted.
            if CPU_LOCAL.load(Ordering::Relaxed) == 0 {
                cpu::init_boot_cpu::<Self>();
            }
            CPU_LOCAL.load(Ordering::Relaxed)
        }
        fn wake_cpu(cpu: u64) {
            assert_eq!(cpu, 0, "the test platform has one CPU");
            WAKEUP_PENDING.store(true, Ordering::Relaxed);
        }
    }

    /// The system calls the test program's `linux_start` makes, from a stack at `stack` in
    /// the address space the test platform last switched to: a `write` for each argument and
    /// environment string, then `exit_group(argc)`, or `exit_group(255)` if the stack pointer
    /// is not 16-byte aligned.
    fn linux_start_calls(stack: u64) -> Vec<(u64, [u64; 6])> {
        use crate::linux::number;
        let tables =
            crate::paging::PageTables::<<TestPlatform as Platform>::PageTableFormat>::view_for_test(
                active_root(),
                TestPlatform::physical_memory_offset(),
            );
        let byte = |virt: u64| {
            let (phys, _) = tables
                .translate(virt)
                .expect("the program's memory is mapped");
            // SAFETY: a frame of the test RAM, through its offset.
            unsafe {
                *((phys as usize).wrapping_add(TestPlatform::physical_memory_offset()) as *const u8)
            }
        };
        let word = |virt: u64| u64::from_le_bytes(core::array::from_fn(|i| byte(virt + i as u64)));
        if !stack.is_multiple_of(16) {
            return vec![(number::EXIT_GROUP, [255, 0, 0, 0, 0, 0])];
        }
        let mut calls = Vec::new();
        let mut pointer = stack + 8;
        for _list in 0..2 {
            loop {
                let string = word(pointer);
                pointer += 8;
                if string == 0 {
                    break;
                }
                let length = (0..).take_while(|&i| byte(string + i) != 0).count() as u64;
                calls.push((number::WRITE, [1, string, length, 0, 0, 0]));
            }
        }
        calls.push((number::EXIT_GROUP, [word(stack), 0, 0, 0, 0, 0]));
        calls
    }

    /// Where the kernel reaches the active address space's byte at user address `virt` on the
    /// test platform: its frame in the test RAM, through its offset. Panics if `virt` is not
    /// mapped.
    fn user_byte(virt: u64) -> *mut u8 {
        let tables =
            crate::paging::PageTables::<<TestPlatform as Platform>::PageTableFormat>::view_for_test(
                active_root(),
                TestPlatform::physical_memory_offset(),
            );
        let (phys, _) = tables
            .translate(virt)
            .expect("the program's memory is mapped");
        (phys as usize).wrapping_add(TestPlatform::physical_memory_offset()) as *mut u8
    }

    /// Writes `bytes` at user address `address` of the active address space, as the test
    /// program's own stores would (see [`user_byte`]).
    fn write_user_bytes(address: u64, bytes: &[u8]) {
        for (i, &byte) in bytes.iter().enumerate() {
            // SAFETY: a byte of a frame of the test RAM, mapped for the program.
            unsafe { user_byte(address + i as u64).write(byte) };
        }
    }

    /// Reads `bytes.len()` bytes at user address `address` of the active address space, as the
    /// test program's own loads would.
    fn read_user_bytes(address: u64, bytes: &mut [u8]) {
        for (i, byte) in bytes.iter_mut().enumerate() {
            // SAFETY: as in `write_user_bytes`.
            *byte = unsafe { user_byte(address + i as u64).read() };
        }
    }

    fn signal_queue_program(
        context: &mut eggos_hal::user::UserContext,
        start: u64,
    ) -> eggos_hal::user::UserExit {
        use crate::linux::{Errno, number, signals};
        use eggos_hal::user::{TestProgram, UserExit};
        static PID: AtomicU64 = AtomicU64::new(0);
        let pc = context.pc();
        let base = context.stack() - 288;
        let (set, sent, received, timeout) = (base + 272, base + 128, base, base + 256);
        let signal = u64::from(TestProgram::LINUX_QUEUED_SIGNAL);
        let [first_value, second_value] = TestProgram::LINUX_QUEUED_VALUES;
        let word = |address: u64| {
            let mut bytes = [0u8; 8];
            read_user_bytes(address, &mut bytes);
            u64::from_ne_bytes(bytes)
        };
        let queue_with = |value: u64| {
            let pid = PID.load(Ordering::Relaxed);
            let mut info = [0u8; 32];
            info[..4].copy_from_slice(&(signal as u32).to_ne_bytes());
            info[8..12].copy_from_slice(&signals::code::QUEUE.to_ne_bytes());
            info[16..20].copy_from_slice(&(pid as u32).to_ne_bytes());
            info[24..].copy_from_slice(&value.to_ne_bytes());
            write_user_bytes(sent, &info);
            (number::RT_SIGQUEUEINFO, [pid, signal, sent, 0, 0, 0])
        };
        let wait = (number::RT_SIGTIMEDWAIT, [set, received, timeout, 8, 0, 0]);
        let took = |result: u64, code: i32, value: u64| {
            result == signal
                && word(received) as u32 == signal as u32
                && word(received + 8) as u32 as i32 == code
                && word(received + 16) == PID.load(Ordering::Relaxed)
                && word(received + 24) == value
        };
        let result = context.syscall_result();
        let step = pc - start;
        let passed = match step {
            0 => true,
            1..=5 if step != 2 => result == 0,
            2 => {
                PID.store(result, Ordering::Relaxed);
                true
            }
            6 => took(result, signals::code::QUEUE, first_value),
            7 => took(result, signals::code::QUEUE, second_value),
            8 => took(result, signals::code::USER, 0),
            _ => result == Errno::EAGAIN.result(),
        };
        let (number, arguments) = match step {
            _ if !passed => (number::EXIT_GROUP, [1, 0, 0, 0, 0, 0]),
            0 => {
                write_user_bytes(set, &(1u64 << (signal - 1)).to_ne_bytes());
                write_user_bytes(timeout, &[0; 16]);
                (number::RT_SIGPROCMASK, [0, set, 0, 8, 0, 0])
            }
            1 => (number::GETPID, [0; 6]),
            2 => queue_with(first_value),
            3 => queue_with(second_value),
            4 => (
                number::KILL,
                [PID.load(Ordering::Relaxed), signal, 0, 0, 0, 0],
            ),
            5..=8 => wait,
            _ => (number::EXIT_GROUP, [0; 6]),
        };
        context.simulate_syscall(number, arguments, pc + 1);
        UserExit::Syscall
    }

    /// Plays `linux_stat` (at `start`) for the test platform: `fstat(1)`, `newfstatat(1, "",
    /// AT_EMPTY_PATH)` and `statx(1, "", AT_EMPTY_PATH, STATX_BASIC_STATS)` into a buffer
    /// below a stack pointer it moves down, then `lseek(1, 0, SEEK_CUR)` and `fstat(3)`, and
    /// `exit_group(0)` if each answered as the program checks.
    fn stat_program(
        context: &mut eggos_hal::user::UserContext,
        start: u64,
    ) -> eggos_hal::user::UserExit {
        use crate::linux::{Errno, number};
        use eggos_hal::user::UserExit;
        let pc = context.pc();
        if pc == start {
            *context = eggos_hal::user::UserContext::new(pc, context.stack() - 272);
            write_user_bytes(context.stack() + 256, &[0]);
        }
        let buffer = context.stack();
        let empty_path = buffer + 256;
        let (mode, rdev) = if cfg!(target_arch = "x86_64") {
            (24, 40)
        } else {
            (16, 32)
        };
        let field = |offset: u64, bytes: &mut [u8]| read_user_bytes(buffer + offset, bytes);
        let character_device = |offset: u64, size: usize| {
            let mut bytes = [0u8; 4];
            field(offset, &mut bytes[..size]);
            u32::from_ne_bytes(bytes) & 0xf000 == 0x2000
        };
        let console_device = || {
            let mut bytes = [0u8; 8];
            field(rdev, &mut bytes);
            u64::from_ne_bytes(bytes) == 0x501
        };
        let result = context.syscall_result();
        let step = pc - start;
        let passed = match step {
            0 => true,
            1 => result == 0 && character_device(mode, 4) && console_device(),
            2 => result == 0 && console_device(),
            3 => {
                let mut major = [0u8; 4];
                field(128, &mut major);
                result == 0 && character_device(28, 2) && u32::from_ne_bytes(major) == 5
            }
            4 => result == Errno::ESPIPE.result(),
            _ => result == Errno::EBADF.result(),
        };
        let (number, arguments) = match step {
            _ if !passed => (number::EXIT_GROUP, [1, 0, 0, 0, 0, 0]),
            0 => (number::FSTAT, [1, buffer, 0, 0, 0, 0]),
            1 => {
                write_user_bytes(buffer + rdev, &[0; 8]);
                (number::NEWFSTATAT, [1, empty_path, buffer, 0x1000, 0, 0])
            }
            2 => {
                write_user_bytes(buffer + 28, &[0; 2]);
                write_user_bytes(buffer + 128, &[0; 4]);
                (number::STATX, [1, empty_path, 0x1000, 0x7ff, buffer, 0])
            }
            3 => (number::LSEEK, [1, 0, 1, 0, 0, 0]),
            4 => (number::FSTAT, [3, buffer, 0, 0, 0, 0]),
            _ => (number::EXIT_GROUP, [0; 6]),
        };
        context.simulate_syscall(number, arguments, pc + 1);
        UserExit::Syscall
    }

    /// Plays `linux_credentials` (at `start`) for the test platform: `geteuid()`,
    /// `setuid(1000)`, `getuid()` and `setuid(0)`, and `exit_group(0)` if each answered as the
    /// program checks.
    fn credentials_program(
        context: &mut eggos_hal::user::UserContext,
        start: u64,
    ) -> eggos_hal::user::UserExit {
        use crate::linux::{Errno, number};
        use eggos_hal::user::UserExit;
        let pc = context.pc();
        let result = context.syscall_result();
        let step = pc - start;
        let passed = match step {
            0 => true,
            1 | 2 => result == 0,
            3 => result == 1000,
            _ => result == Errno::EPERM.result(),
        };
        let (number, arguments) = match step {
            _ if !passed => (number::EXIT_GROUP, [1, 0, 0, 0, 0, 0]),
            0 => (number::GETEUID, [0; 6]),
            1 => (number::SETUID, [1000, 0, 0, 0, 0, 0]),
            2 => (number::GETUID, [0; 6]),
            3 => (number::SETUID, [0; 6]),
            _ => (number::EXIT_GROUP, [0; 6]),
        };
        context.simulate_syscall(number, arguments, pc + 1);
        UserExit::Syscall
    }

    /// Plays `linux_move_and_discard` (at `start`) for the test platform: `mmap`s a page,
    /// populates it and writes a word there, moves it 1 MiB down with `mremap`, maps a new
    /// page where it was, discards the moved one with `madvise`, and `exit_group(0)` if each
    /// call answered and each page read as the program checks.
    fn move_and_discard_program(
        context: &mut eggos_hal::user::UserContext,
        start: u64,
    ) -> eggos_hal::user::UserExit {
        use crate::linux::memory::{advice, map, prot, remap};
        use crate::linux::number;
        use eggos_hal::user::UserExit;
        const WORD: u64 = 0x1234_5678;
        const MOVED_DOWN_BY: u64 = 0x10_0000;
        static FIRST: AtomicU64 = AtomicU64::new(0);
        let pc = context.pc();
        let result = context.syscall_result();
        let word = |address: u64| {
            let tables = crate::paging::PageTables::<
                <TestPlatform as Platform>::PageTableFormat,
            >::view_for_test(active_root(), TestPlatform::physical_memory_offset());
            if tables.translate(address).is_none() {
                return 0;
            }
            let mut bytes = [0; 8];
            read_user_bytes(address, &mut bytes);
            u64::from_ne_bytes(bytes)
        };
        let first = FIRST.load(Ordering::Relaxed);
        let moved = first.wrapping_sub(MOVED_DOWN_BY);
        let anonymous = map::PRIVATE | map::ANONYMOUS;
        let read_write = prot::READ | prot::WRITE;
        let step = pc - start;
        let passed = match step {
            0 => true,
            1 => result < 0u64.wrapping_sub(4096),
            2 => result == 0,
            3 => result == moved && word(moved) == WORD,
            4 => result == first && word(first) == 0,
            _ => result == 0 && word(moved) == 0,
        };
        let (number, arguments) = match step {
            _ if !passed => (number::EXIT_GROUP, [1, 0, 0, 0, 0, 0]),
            0 => (number::MMAP, [0, 4096, read_write, anonymous, u64::MAX, 0]),
            1 => {
                FIRST.store(result, Ordering::Relaxed);
                (
                    number::MADVISE,
                    [result, 4096, advice::POPULATE_WRITE, 0, 0, 0],
                )
            }
            2 => {
                write_user_bytes(first, &WORD.to_ne_bytes());
                let flags = remap::MAYMOVE | remap::FIXED;
                (number::MREMAP, [first, 4096, 4096, flags, moved, 0])
            }
            3 => (
                number::MMAP,
                [first, 4096, read_write, anonymous | map::FIXED, u64::MAX, 0],
            ),
            4 => (number::MADVISE, [moved, 4096, advice::DONTNEED, 0, 0, 0]),
            _ => (number::EXIT_GROUP, [0; 6]),
        };
        context.simulate_syscall(number, arguments, pc + 1);
        UserExit::Syscall
    }

    /// Plays `linux_clock` (at `start`) for the test platform: `clock_gettime(CLOCK_MONOTONIC)`
    /// below a stack pointer it moves down, `nanosleep` for
    /// [`TestProgram::LINUX_CLOCK_WAIT_NS`], `clock_gettime(CLOCK_MONOTONIC)` again, then
    /// `clock_gettime(CLOCK_PROCESS_CPUTIME_ID)`, and `exit_group(0)` if each answered as the
    /// program checks.
    fn clock_program(
        context: &mut eggos_hal::user::UserContext,
        start: u64,
    ) -> eggos_hal::user::UserExit {
        use crate::linux::{Errno, number};
        use eggos_hal::user::UserExit;
        let pc = context.pc();
        if pc == start {
            *context = eggos_hal::user::UserContext::new(pc, context.stack() - 64);
        }
        let stack = context.stack();
        let (first, second, request) = (stack, stack + 16, stack + 32);
        let nanoseconds = |address: u64| {
            let mut bytes = [0u8; 16];
            read_user_bytes(address, &mut bytes);
            let [seconds, nanoseconds] =
                [&bytes[..8], &bytes[8..]].map(|half| u64::from_ne_bytes(half.try_into().unwrap()));
            seconds * 1_000_000_000 + nanoseconds
        };
        let result = context.syscall_result();
        let step = pc - start;
        let passed = match step {
            0 => true,
            1 | 2 => result == 0,
            3 => {
                result == 0
                    && nanoseconds(second) - nanoseconds(first) >= TestProgram::LINUX_CLOCK_WAIT_NS
            }
            _ => result == Errno::EINVAL.result(),
        };
        let (number, arguments) = match step {
            _ if !passed => (number::EXIT_GROUP, [1, 0, 0, 0, 0, 0]),
            0 => (number::CLOCK_GETTIME, [1, first, 0, 0, 0, 0]),
            1 => {
                let mut bytes = [0u8; 16];
                bytes[8..].copy_from_slice(&TestProgram::LINUX_CLOCK_WAIT_NS.to_ne_bytes());
                write_user_bytes(request, &bytes);
                (number::NANOSLEEP, [request, 0, 0, 0, 0, 0])
            }
            2 => (number::CLOCK_GETTIME, [1, second, 0, 0, 0, 0]),
            3 => (number::CLOCK_GETTIME, [2, second, 0, 0, 0, 0]),
            _ => (number::EXIT_GROUP, [0; 6]),
        };
        context.simulate_syscall(number, arguments, pc + 1);
        UserExit::Syscall
    }

    /// Plays `linux_signal_suspend` (at `start`) for the test platform: `rt_sigaction` for
    /// `SIGUSR1` below a stack pointer it moves down, `rt_sigprocmask` blocking it, `getpid`,
    /// `tgkill`, then `rt_sigsuspend` with an empty set, whose handler (at [`SIGNAL_HANDLER`])
    /// checks the mask its frame saved, changes the frame and returns with `rt_sigreturn`;
    /// then `rt_sigprocmask` reads the mask back, and `exit_group(0)` if every check passed.
    /// Each call's result is checked as the program checks it.
    fn signal_suspend_program(
        context: &mut eggos_hal::user::UserContext,
        start: u64,
    ) -> eggos_hal::user::UserExit {
        use crate::linux::{Errno, number, signal_frame, signals};
        use eggos_hal::user::UserExit;
        let usr1 = 1u64 << 9;
        let pc = context.pc();
        let word = |address: u64| {
            let mut bytes = [0u8; 8];
            read_user_bytes(address, &mut bytes);
            u64::from_ne_bytes(bytes)
        };
        if pc == start + SIGNAL_HANDLER {
            let [signal, _, ucontext, ..] = context.syscall_arguments();
            let sigmask = ucontext - signal_frame::layout::UCONTEXT as u64
                + signal_frame::layout::SIGMASK as u64;
            if signal == 10 && word(sigmask) == usr1 {
                write_user_bytes(ucontext + CHANGED_IN_UCONTEXT, &2u64.to_ne_bytes());
            }
            #[cfg(target_arch = "x86_64")]
            {
                let mut registers = context.registers();
                registers.rsp += 8;
                context.set_registers(&registers);
            }
            context.simulate_syscall(number::RT_SIGRETURN, [0; 6], pc + 1);
            return UserExit::Syscall;
        }
        if pc == start {
            *context = eggos_hal::user::UserContext::new(pc, context.stack() - 64);
        }
        let stack = context.stack();
        let (action, set, empty, old) = (stack, stack + 32, stack + 40, stack + 48);
        let result = context.syscall_result();
        let step = pc - start;
        let mut registers = context.registers();
        let [_, changed] = kept_and_changed(&mut registers);
        let passed = match step {
            0 | 3 => true,
            1 | 2 | 4 => result == 0,
            5 => result == Errno::EINTR.result() && *changed == 2,
            _ => result == 0 && word(old) == usr1,
        };
        let (number, arguments) = match step {
            _ if !passed => (number::EXIT_GROUP, [1, 0, 0, 0, 0, 0]),
            0 => {
                let flags = signals::flag::SIGINFO
                    | if cfg!(target_arch = "x86_64") {
                        signals::flag::RESTORER
                    } else {
                        0
                    };
                let handler = start + SIGNAL_HANDLER;
                let words = if signals::HAS_RESTORER {
                    [handler, flags, start + SIGNAL_HANDLER + 10, 0]
                } else {
                    [handler, flags, 0, 0]
                };
                write_user_bytes(action, &words.map(u64::to_ne_bytes).concat());
                (number::RT_SIGACTION, [10, action, 0, 8, 0, 0])
            }
            1 => {
                write_user_bytes(set, &usr1.to_ne_bytes());
                write_user_bytes(empty, &0u64.to_ne_bytes());
                (number::RT_SIGPROCMASK, [0, set, 0, 8, 0, 0])
            }
            2 => (number::GETPID, [0; 6]),
            3 => (number::TGKILL, [result, result, 10, 0, 0, 0]),
            4 => {
                *changed = 1;
                context.set_registers(&registers);
                (number::RT_SIGSUSPEND, [empty, 8, 0, 0, 0, 0])
            }
            5 => (number::RT_SIGPROCMASK, [0, 0, old, 8, 0, 0]),
            _ => (number::EXIT_GROUP, [0; 6]),
        };
        context.simulate_syscall(number, arguments, pc + 1);
        UserExit::Syscall
    }

    /// Where the simulated `linux_signal_handler` program's handler is, from its start.
    const SIGNAL_HANDLER: u64 = 20;

    /// The two registers the simulated `linux_signal_handler` sets before its `tgkill`: one
    /// the handler clobbers, which `rt_sigreturn` must restore, and one it sets to 2 in its
    /// frame (`x19` and `x20`, `s1` and `s2`, `rbx` and `r12`), with the latter's offset in the
    /// `ucontext`.
    #[cfg(target_arch = "aarch64")]
    fn kept_and_changed(registers: &mut eggos_hal::user::UserRegisters) -> [&mut u64; 2] {
        let [.., kept, changed, _, _, _, _, _, _, _, _, _, _] = &mut registers.x;
        [kept, changed]
    }
    #[cfg(target_arch = "aarch64")]
    const CHANGED_IN_UCONTEXT: u64 = 176 + 8 + 20 * 8;
    #[cfg(target_arch = "riscv64")]
    fn kept_and_changed(registers: &mut eggos_hal::user::UserRegisters) -> [&mut u64; 2] {
        let (low, high) = registers.x.split_at_mut(18);
        [&mut low[9], &mut high[0]]
    }
    #[cfg(target_arch = "riscv64")]
    const CHANGED_IN_UCONTEXT: u64 = 176 + 18 * 8;
    #[cfg(target_arch = "x86_64")]
    fn kept_and_changed(registers: &mut eggos_hal::user::UserRegisters) -> [&mut u64; 2] {
        [&mut registers.rbx, &mut registers.r12]
    }
    #[cfg(target_arch = "x86_64")]
    const CHANGED_IN_UCONTEXT: u64 = 40 + 4 * 8;

    /// Where the simulated `linux_signal_handler`'s handler sets an FP/SIMD register in its
    /// frame whose `ucontext_t` is at `ucontext` (`v9`, `f9`, `xmm9`), as the program does: the
    /// thread has none until then, so `rt_sigreturn` must give it some.
    fn fp_changed_in_frame(ucontext: u64) -> u64 {
        #[cfg(target_arch = "aarch64")]
        {
            ucontext + 176 + 288 + 16 + 9 * 16
        }
        #[cfg(target_arch = "riscv64")]
        {
            ucontext + 176 + 256 + 9 * 8
        }
        #[cfg(target_arch = "x86_64")]
        {
            let mut image = [0u8; 8];
            read_user_bytes(ucontext + 40 + 184, &mut image);
            u64::from_ne_bytes(image) + 160 + 9 * 16
        }
    }

    /// Plays `linux_signal_handler` (at `start`) for the test platform: `sigaltstack`,
    /// `rt_sigaction` for `SIGUSR1`, `getpid`, then `tgkill`, whose handler (at
    /// [`SIGNAL_HANDLER`]) checks its arguments and stack, changes the frame and returns with
    /// `rt_sigreturn`; then `rt_sigprocmask` reads the mask back, and `exit_group(0)` if every
    /// check passed. Each call's result is checked as the program checks it.
    fn signal_handler_program(
        context: &mut eggos_hal::user::UserContext,
        start: u64,
    ) -> eggos_hal::user::UserExit {
        use crate::linux::{number, signals};
        use eggos_hal::user::UserExit;
        let pc = context.pc();
        let stack = context.stack();
        let word = |address: u64| {
            let mut bytes = [0u8; 8];
            read_user_bytes(address, &mut bytes);
            u64::from_ne_bytes(bytes)
        };
        if pc == start + SIGNAL_HANDLER {
            let [signal, info, ucontext, ..] = context.syscall_arguments();
            let (signo, code) = (word(info) as u32, word(info + 8) as u32 as i32);
            let (base, size) = (word(ucontext + 16), word(ucontext + 32));
            if signal == 10 && signo == 10 && code == -6 && (base..base + size).contains(&stack) {
                write_user_bytes(ucontext + CHANGED_IN_UCONTEXT, &2u64.to_ne_bytes());
                let changed = eggos_hal::user::TestProgram::LINUX_SIGNAL_FP_CHANGED;
                write_user_bytes(fp_changed_in_frame(ucontext), &changed.to_ne_bytes());
            }
            let mut registers = context.registers();
            *kept_and_changed(&mut registers)[0] = 0;
            // Its return: x86_64's `ret` pops the restorer.
            #[cfg(target_arch = "x86_64")]
            {
                registers.rsp += 8;
            }
            context.set_registers(&registers);
            context.simulate_syscall(number::RT_SIGRETURN, [0; 6], pc + 1);
            return UserExit::Syscall;
        }
        let result = context.syscall_result();
        let step = pc - start;
        let mut registers = context.registers();
        let [kept, changed] = kept_and_changed(&mut registers);
        let failed = match step {
            1 | 2 | 5 => result != 0,
            4 => result != 0 || *kept != 0x5a5 || *changed != 2,
            _ => false,
        } || step == 5 && word(stack - 72) != 0;
        let (number, arguments) = match step {
            _ if failed => (number::EXIT_GROUP, [1, 0, 0, 0, 0, 0]),
            0 => {
                let stack_t = [stack - 0x2800, 0, 0x2000];
                write_user_bytes(stack - 32, &stack_t.map(u64::to_ne_bytes).concat());
                (number::SIGALTSTACK, [stack - 32, 0, 0, 0, 0, 0])
            }
            1 => {
                let flags = signals::flag::SIGINFO
                    | signals::flag::ONSTACK
                    | if cfg!(target_arch = "x86_64") {
                        signals::flag::RESTORER
                    } else {
                        0
                    };
                let handler = start + SIGNAL_HANDLER;
                let action = if signals::HAS_RESTORER {
                    [handler, flags, start + SIGNAL_HANDLER + 10, 0]
                } else {
                    [handler, flags, 0, 0]
                };
                write_user_bytes(stack - 64, &action.map(u64::to_ne_bytes).concat());
                (number::RT_SIGACTION, [10, stack - 64, 0, 8, 0, 0])
            }
            2 => (number::GETPID, [0; 6]),
            3 => {
                (*kept, *changed) = (0x5a5, 1);
                context.set_registers(&registers);
                (number::TGKILL, [result, result, 10, 0, 0, 0])
            }
            4 => (number::RT_SIGPROCMASK, [0, 0, stack - 72, 8, 0, 0]),
            _ => (number::EXIT_GROUP, [0; 6]),
        };
        context.simulate_syscall(number, arguments, pc + 1);
        UserExit::Syscall
    }

    /// The four registers the simulated `linux_fault_handler` sets before each fault: the
    /// faults its handler counted, the `si_addr` and `si_code` the handler must see, and where
    /// it resumes the program (`x20` to `x23`, `s2` to `s5`, `r12` to `r15`), with their
    /// offsets in the `ucontext`, then the program counter's.
    #[cfg(target_arch = "aarch64")]
    fn fault_registers(registers: &mut eggos_hal::user::UserRegisters) -> [&mut u64; 4] {
        let [.., count, address, code, resume, _, _, _, _, _, _, _] = &mut registers.x;
        [count, address, code, resume]
    }
    #[cfg(target_arch = "aarch64")]
    const FAULT_REGISTERS_IN_UCONTEXT: [u64; 5] = [
        176 + 8 + 20 * 8,
        176 + 8 + 21 * 8,
        176 + 8 + 22 * 8,
        176 + 8 + 23 * 8,
        176 + 264,
    ];
    #[cfg(target_arch = "riscv64")]
    fn fault_registers(registers: &mut eggos_hal::user::UserRegisters) -> [&mut u64; 4] {
        let [count, address, code, resume] = &mut registers.x[18..22] else {
            unreachable!()
        };
        [count, address, code, resume]
    }
    #[cfg(target_arch = "riscv64")]
    const FAULT_REGISTERS_IN_UCONTEXT: [u64; 5] =
        [176 + 18 * 8, 176 + 19 * 8, 176 + 20 * 8, 176 + 21 * 8, 176];
    #[cfg(target_arch = "x86_64")]
    fn fault_registers(registers: &mut eggos_hal::user::UserRegisters) -> [&mut u64; 4] {
        [
            &mut registers.r12,
            &mut registers.r13,
            &mut registers.r14,
            &mut registers.r15,
        ]
    }
    #[cfg(target_arch = "x86_64")]
    const FAULT_REGISTERS_IN_UCONTEXT: [u64; 5] =
        [40 + 4 * 8, 40 + 5 * 8, 40 + 6 * 8, 40 + 7 * 8, 40 + 16 * 8];

    /// Whether the machine context at `ucontext` reports a fault at `address` as the
    /// simulated `linux_fault_handler`'s handler checks it: AArch64's `fault_address` and
    /// `esr_context` record, x86_64's `cr2` and `trapno` 14, nothing on RISC-V.
    fn reports_fault(ucontext: u64, address: u64) -> bool {
        let word = |at: u64| {
            let mut bytes = [0u8; 8];
            read_user_bytes(ucontext + at, &mut bytes);
            u64::from_ne_bytes(bytes)
        };
        match () {
            _ if cfg!(target_arch = "aarch64") => {
                word(176) == address && word(176 + 288 + 528) == 0x10_4553_5201
            }
            _ if cfg!(target_arch = "x86_64") => word(40 + 176) == address && word(40 + 160) == 14,
            _ => true,
        }
    }

    /// Plays `linux_fault_handler` (at `start`) for the test platform: `rt_sigaction` for
    /// `SIGSEGV`, `mmap` of a `PROT_NONE` page, a read of `LINUX_FAULT_ADDRESS`, then of that
    /// page, each a fault whose handler (at [`SIGNAL_HANDLER`]) checks its `siginfo_t` and
    /// machine context against the registers the frame holds, counts it, resumes the program
    /// past it and returns with `rt_sigreturn`; then `exit_group(0)` if both were counted.
    fn fault_handler_program(
        context: &mut eggos_hal::user::UserContext,
        start: u64,
    ) -> eggos_hal::user::UserExit {
        use crate::linux::{fault, number, signals};
        use eggos_hal::user::UserExit;
        static MAPPED: AtomicU64 = AtomicU64::new(0);
        let pc = context.pc();
        let stack = context.stack();
        let word = |address: u64| {
            let mut bytes = [0u8; 8];
            read_user_bytes(address, &mut bytes);
            u64::from_ne_bytes(bytes)
        };
        if pc == start + SIGNAL_HANDLER {
            let [signal, info, ucontext, ..] = context.syscall_arguments();
            let [count, address, code, resume, pc_at] =
                FAULT_REGISTERS_IN_UCONTEXT.map(|offset| ucontext + offset);
            let (signo, si_code, si_addr) =
                (word(info) as u32, word(info + 8) as u32, word(info + 16));
            if signal == 11
                && signo == 11
                && u64::from(si_code) == word(code)
                && si_addr == word(address)
                && reports_fault(ucontext, si_addr)
            {
                write_user_bytes(count, &(word(count) + 1).to_ne_bytes());
            }
            write_user_bytes(pc_at, &word(resume).to_ne_bytes());
            // Its return: x86_64's `ret` pops the restorer.
            #[cfg(target_arch = "x86_64")]
            {
                let mut registers = context.registers();
                registers.rsp += 8;
                context.set_registers(&registers);
            }
            context.simulate_syscall(number::RT_SIGRETURN, [0; 6], pc + 1);
            return UserExit::Syscall;
        }
        let result = context.syscall_result();
        let step = pc - start;
        let fault = |context: &mut eggos_hal::user::UserContext, address, code, resume| {
            let mut registers = context.registers();
            let [_, expected_address, expected_code, resume_at] = fault_registers(&mut registers);
            (*expected_address, *expected_code, *resume_at) = (address, code, resume);
            context.set_registers(&registers);
            // A read, as the test platform encodes it, with the architecture's class in it
            // (AArch64's data abort, RISC-V's load page fault).
            let code = match () {
                _ if cfg!(target_arch = "x86_64") => 0,
                _ if cfg!(target_arch = "aarch64") => 0x24 << 26,
                _ => 13,
            };
            context.simulate_exception(Exception {
                kind: ExceptionKind::DataFault,
                name: "test page fault",
                code,
                pc,
                address: Some(address),
            })
        };
        let (number, arguments) = match step {
            0 => {
                let flags = signals::flag::SIGINFO
                    | if cfg!(target_arch = "x86_64") {
                        signals::flag::RESTORER
                    } else {
                        0
                    };
                let handler = start + SIGNAL_HANDLER;
                let action = if signals::HAS_RESTORER {
                    [handler, flags, start + SIGNAL_HANDLER + 10, 0]
                } else {
                    [handler, flags, 0, 0]
                };
                write_user_bytes(stack - 64, &action.map(u64::to_ne_bytes).concat());
                (number::RT_SIGACTION, [11, stack - 64, 0, 8, 0, 0])
            }
            1 if result != 0 => (number::EXIT_GROUP, [1, 0, 0, 0, 0, 0]),
            1 => (number::MMAP, [0, 4096, 0, 0x22, u64::MAX, 0]),
            2 if result > u64::MAX - 4095 => (number::EXIT_GROUP, [1, 0, 0, 0, 0, 0]),
            2 => {
                MAPPED.store(result, Ordering::Relaxed);
                let mut registers = context.registers();
                *fault_registers(&mut registers)[0] = 0;
                context.set_registers(&registers);
                let code = fault::code::SEGV_MAPERR as u64;
                return fault(context, TestProgram::LINUX_FAULT_ADDRESS, code, pc + 1);
            }
            3 => {
                let code = fault::code::SEGV_ACCERR as u64;
                return fault(context, MAPPED.load(Ordering::Relaxed), code, pc + 1);
            }
            _ => {
                let mut registers = context.registers();
                let counted = *fault_registers(&mut registers)[0];
                (number::EXIT_GROUP, [u64::from(counted != 2), 0, 0, 0, 0, 0])
            }
        };
        context.simulate_syscall(number, arguments, pc + 1);
        UserExit::Syscall
    }

    /// Where the test platform's test program keeps the line its Linux program writes.
    const TEST_PROGRAM_LINE: usize = 28;
    /// The test platform's test program: no real code (`run_user` plays its part), and the
    /// line its Linux program writes.
    static TEST_PROGRAM_CODE: [u8; 64] = {
        let mut code = [0xa5; 64];
        let line = TestProgram::LINUX_HELLO.as_bytes();
        let mut i = 0;
        while i < line.len() {
            code[TEST_PROGRAM_LINE + i] = line[i];
            i += 1;
        }
        code
    };

    /// The test platform's per-CPU register. The test machine has one CPU, which the host
    /// threads standing in for kernel threads take turns on (see `thread::host`), so they all
    /// share it.
    static CPU_LOCAL: AtomicUsize = AtomicUsize::new(0);

    #[test]
    fn boot_prints_banner_with_version_and_arch() {
        let _lock = interrupt::lock_timer_interrupts_for_test();
        let _scheduler = thread::host::lock_kernel_state_for_test();
        let mut out = String::new();
        assert_eq!(
            boot::<TestPlatform, _>(&mut out),
            ExitStatus::Success,
            "{out}"
        );
        assert!(out.starts_with(&format!(
            "EggOS {VERSION} (testarch)\nclock: 1000000 Hz\n\
             memory: 16384 KiB free in 1 range\n  0x100000..0x1100000\n\
             random: seeded, 256 bits credited from 1 seed\n\
             paging: kernel page tables active, direct map at {:#x}\n",
            TestPlatform::direct_map_offset()
        )));
        assert!(out.ends_with(&format!("{}\n", selftest::PASS_MARKER)));
    }
}
