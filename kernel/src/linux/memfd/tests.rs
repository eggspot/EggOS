use super::*;
use crate::linux::descriptors::open_flag;
use crate::linux::memory::{map, prot};
use crate::linux::{handle, number};
use crate::paging::PageTables;
use crate::tests::TestPlatform as P;
use crate::thread::host::frames_for_test;
use alloc::vec;
use core::ops::ControlFlow;
use eggos_hal::user::{UserContext, UserExit};

const DATA: u64 = 0x50_0000;

fn kernel_tables() -> PageTables<<P as Platform>::PageTableFormat> {
    let mut frames = FRAMES.lock();
    let mut tables = PageTables::new(frames.as_mut().unwrap()).unwrap();
    tables.mark_active();
    tables
}

fn process(kernel: &PageTables<<P as Platform>::PageTableFormat>) -> Arc<Process<P>> {
    // SAFETY: test tables, never active; the test platform's switch only records roots.
    let mut process = unsafe { Process::<P>::new(kernel) }.unwrap();
    for page in 0..2 {
        process
            .map_new_page(DATA + page * FRAME_SIZE, Attributes::USER_READ_WRITE, &[])
            .unwrap();
    }
    Arc::new(process)
}

fn call(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    number: u64,
    arguments: [u64; 6],
) -> u64 {
    let mut context = UserContext::new(0x40_0000, 0);
    context.simulate_syscall(number, arguments, 0x40_0004);
    assert_eq!(
        handle(process, thread, &mut context, UserExit::Syscall),
        ControlFlow::Continue(())
    );
    context.syscall_result()
}

fn free_frames() -> u64 {
    FRAMES.lock().as_ref().unwrap().free_frames()
}

fn memfd(process: &Arc<Process<P>>, thread: &mut LinuxThread, flags: u32) -> u64 {
    process.write_user_memory(DATA, b"wl_shm\0").unwrap();
    call(
        process,
        thread,
        number::MEMFD_CREATE,
        [DATA, u64::from(flags), 0, 0, 0, 0],
    )
}

fn mmap(
    process: &Arc<Process<P>>,
    thread: &mut LinuxThread,
    length: u64,
    protection: u64,
    flags: u64,
    fd: u64,
    offset: u64,
) -> u64 {
    call(
        process,
        thread,
        number::MMAP,
        [0, length, protection, flags, fd, offset],
    )
}

fn byte(process: &Process<P>, address: u64) -> u8 {
    let mut byte = [0u8];
    process.read_user_memory(address, &mut byte).unwrap();
    byte[0]
}

const PAGE_BYTES: u64 = FRAME_SIZE;
const SHARED: u64 = map::SHARED;
const READ_WRITE: u64 = prot::READ | prot::WRITE;

#[test]
fn bytes_come_back_holes_read_zero_and_writes_grow_the_file() {
    let _frames = frames_for_test();
    let file = MemoryFile::new(0);
    assert_eq!(file.write_at(10, b"hello"), Ok(5));
    assert_eq!(file.size(), 15);
    let mut out = [0xffu8; 20];
    assert_eq!(file.read_at(0, &mut out), 15);
    assert_eq!(&out[..10], &[0u8; 10]);
    assert_eq!(&out[10..15], b"hello");
    assert_eq!(file.read_at(15, &mut out), 0);
    assert_eq!(file.write_at(3 * PAGE_BYTES - 2, b"abcd"), Ok(4));
    assert_eq!(file.size(), 3 * PAGE_BYTES + 2);
    assert_eq!(file.frames_held(), 3);
    let mut across = [0u8; 4];
    assert_eq!(file.read_at(3 * PAGE_BYTES - 2, &mut across), 4);
    assert_eq!(&across, b"abcd");
    let mut hole = [0xffu8; 8];
    assert_eq!(file.read_at(PAGE_BYTES + 5, &mut hole), 8);
    assert_eq!(hole, [0; 8]);
    assert_eq!(file.write_at(MAX_SIZE, b"x"), Err(Errno::EFBIG));
}

#[test]
fn truncate_cuts_and_zeroes_and_seals_refuse_what_they_seal() {
    let _frames = frames_for_test();
    let file = MemoryFile::new(0);
    file.write_at(0, &[7u8; 3 * PAGE]).unwrap();
    file.truncate(PAGE_BYTES + 1).unwrap();
    assert_eq!(file.frames_held(), 2);
    file.truncate(3 * PAGE_BYTES).unwrap();
    let mut out = [0xffu8; 2];
    assert_eq!(file.read_at(PAGE_BYTES, &mut out), 2);
    assert_eq!(out, [7, 0]);
    file.add_seals(seal::SHRINK).unwrap();
    assert_eq!(file.truncate(1), Err(Errno::EPERM));
    file.truncate(4 * PAGE_BYTES).unwrap();
    file.add_seals(seal::GROW).unwrap();
    assert_eq!(file.truncate(5 * PAGE_BYTES), Err(Errno::EPERM));
    assert_eq!(file.write_at(4 * PAGE_BYTES, b"x"), Err(Errno::EPERM));
    assert_eq!(file.write_at(0, b"in"), Ok(2));
    assert_eq!(file.allocate(0, 5 * PAGE_BYTES), Err(Errno::EPERM));
    assert_eq!(file.add_seals(seal::WRITE), Err(Errno::EINVAL));
    assert_eq!(file.add_seals(seal::FUTURE_WRITE), Err(Errno::EINVAL));
    assert_eq!(file.add_seals(0x40), Err(Errno::EINVAL));
    file.add_seals(seal::SEAL).unwrap();
    assert_eq!(file.add_seals(seal::EXEC), Err(Errno::EPERM));
    assert_eq!(file.seals(), seal::SHRINK | seal::GROW | seal::SEAL);
    assert_eq!(
        MemoryFile::new(seal::SEAL).add_seals(seal::GROW),
        Err(Errno::EPERM)
    );
}

#[test]
fn a_sparse_file_takes_no_frames_and_allocate_fills_and_grows_it() {
    let _frames = frames_for_test();
    let file = MemoryFile::new(0);
    file.truncate(1 << 30).unwrap();
    assert_eq!(file.frames_held(), 0);
    assert!(file.pages.lock().frames.is_empty());
    let small = MemoryFile::new(0);
    small.allocate(PAGE_BYTES, 2 * PAGE_BYTES).unwrap();
    assert_eq!(small.size(), 3 * PAGE_BYTES);
    assert_eq!(small.frames_held(), 2);
    small.allocate(0, 10).unwrap();
    assert_eq!(small.size(), 3 * PAGE_BYTES);
    assert_eq!(small.frames_held(), 3);
    assert_eq!(small.allocate(MAX_SIZE, 1), Err(Errno::EFBIG));
    assert_eq!(small.allocate(0, 1 << 40), Err(Errno::ENOSPC));
    assert_eq!(small.frames_held(), 3);
    assert!(matches!(
        small.mapping(0, 1 << 40, true),
        Err(Errno::ENOMEM)
    ));
    assert!(matches!(
        small.mapping(MAX_SIZE & !(FRAME_SIZE - 1), 2 * FRAME_SIZE, true),
        Err(Errno::EOVERFLOW)
    ));
    assert_eq!(small.frames_held(), 3);
}

#[test]
fn mappings_and_writes_stop_at_the_reserve_programs_leave_the_kernel() {
    let _frames = frames_for_test();
    let free = free_frames();
    FRAMES.lock().as_mut().unwrap().set_user_reserve(free - 3);
    let file = MemoryFile::new(0);
    file.truncate(16 * PAGE_BYTES).unwrap();
    assert!(matches!(
        file.mapping(0, 4 * PAGE_BYTES, true),
        Err(Errno::ENOMEM)
    ));
    assert_eq!(file.frames_held(), 0);
    let mapping = file.mapping(0, 3 * PAGE_BYTES, true).unwrap();
    assert_eq!(file.frames_held(), 3);
    assert_eq!(free_frames(), free - 3);
    assert_eq!(file.write_at(5 * PAGE_BYTES, b"x"), Err(Errno::ENOSPC));
    assert_eq!(file.allocate(8 * PAGE_BYTES, 1), Err(Errno::ENOSPC));
    drop(mapping);
    drop(file);
    FRAMES.lock().as_mut().unwrap().set_user_reserve(0);
    assert_eq!(free_frames(), free);
}

#[test]
fn a_file_s_frames_go_back_when_it_is_dropped() {
    let _frames = frames_for_test();
    let warm = MemoryFile::new(0);
    warm.write_at(0, &[1u8; 3 * PAGE]).unwrap();
    drop(warm);
    let before = free_frames();
    let file = MemoryFile::new(0);
    file.write_at(0, &[1u8; 3 * PAGE]).unwrap();
    assert_eq!(free_frames(), before - 3);
    drop(file);
    assert_eq!(free_frames(), before);
}

#[test]
fn two_processes_share_a_memfd_s_frames_through_shrinks_and_growth() {
    let _frames = frames_for_test();
    let kernel = kernel_tables();
    let first = process(&kernel);
    let second = process(&kernel);
    let mut first_thread = LinuxThread::first(&first);
    let mut second_thread = LinuxThread::first(&second);
    let fd = memfd(&first, &mut first_thread, flag::ALLOW_SEALING);
    assert_eq!(fd, 3);
    let file = first_thread.files().file(fd).unwrap();
    let FileKind::MemoryFile(memory) = file.kind() else {
        panic!("not a memory file")
    };
    let memory = Arc::clone(memory);
    let limit = 64;
    let second_fd = second_thread
        .files()
        .lock()
        .open(Arc::clone(&file), false, limit)
        .unwrap();
    assert_eq!(
        call(
            &first,
            &mut first_thread,
            number::FALLOCATE,
            [fd, 0, 0, 2 * PAGE_BYTES, 0, 0]
        ),
        0
    );
    let one = mmap(
        &first,
        &mut first_thread,
        2 * PAGE_BYTES,
        READ_WRITE,
        SHARED,
        fd,
        0,
    );
    let two = mmap(
        &second,
        &mut second_thread,
        2 * PAGE_BYTES,
        READ_WRITE,
        SHARED,
        u64::from(second_fd),
        0,
    );
    assert!(one < 1 << 47 && two < 1 << 47, "{one:#x} {two:#x}");
    first
        .write_user_memory(one + PAGE_BYTES + 3, b"from one")
        .unwrap();
    let mut seen = [0u8; 8];
    second
        .read_user_memory(two + PAGE_BYTES + 3, &mut seen)
        .unwrap();
    assert_eq!(&seen, b"from one");
    second.write_user_memory(two, b"two").unwrap();
    let mut back = [0u8; 3];
    assert_eq!(memory.read_at(0, &mut back), 3);
    assert_eq!(&back, b"two");

    assert_eq!(
        call(
            &first,
            &mut first_thread,
            number::FTRUNCATE,
            [fd, 4, 0, 0, 0, 0]
        ),
        0
    );
    assert_eq!(byte(&second, two + PAGE_BYTES + 3), 0);
    assert_eq!(memory.frames_held(), 2);
    assert_eq!(
        call(
            &first,
            &mut first_thread,
            number::FTRUNCATE,
            [fd, 4 * PAGE_BYTES, 0, 0, 0, 0]
        ),
        0
    );
    first.write_user_memory(one + PAGE_BYTES, b"again").unwrap();
    second
        .read_user_memory(two + PAGE_BYTES, &mut seen[..5])
        .unwrap();
    assert_eq!(&seen[..5], b"again");
    memory.write_at(3 * PAGE_BYTES, b"past").unwrap();
    assert_eq!(memory.frames_held(), 3);

    assert_eq!(
        call(
            &first,
            &mut first_thread,
            number::MUNMAP,
            [one, 2 * PAGE_BYTES, 0, 0, 0, 0]
        ),
        0
    );
    assert_eq!(
        call(
            &first,
            &mut first_thread,
            number::FTRUNCATE,
            [fd, PAGE_BYTES, 0, 0, 0, 0]
        ),
        0
    );
    assert_eq!(memory.frames_held(), 3);
    assert_eq!(byte(&second, two + PAGE_BYTES), 0);
    assert_eq!(
        call(
            &second,
            &mut second_thread,
            number::MUNMAP,
            [two, 2 * PAGE_BYTES, 0, 0, 0, 0]
        ),
        0
    );
    assert_eq!(memory.frames_held(), 1);
    let weak = Arc::downgrade(&memory);
    drop(memory);
    drop(file);
    for (process, thread, fd) in [
        (&first, &mut first_thread, fd),
        (&second, &mut second_thread, u64::from(second_fd)),
    ] {
        assert_eq!(call(process, thread, number::CLOSE, [fd, 0, 0, 0, 0, 0]), 0);
    }
    assert_eq!(weak.strong_count(), 0);
}

#[test]
fn a_mapping_keeps_the_file_after_its_last_descriptor_closes() {
    let _frames = frames_for_test();
    let kernel = kernel_tables();
    let process = process(&kernel);
    let mut thread = LinuxThread::first(&process);
    let fd = memfd(&process, &mut thread, 0);
    assert_eq!(
        call(
            &process,
            &mut thread,
            number::FTRUNCATE,
            [fd, PAGE_BYTES, 0, 0, 0, 0]
        ),
        0
    );
    let mapped = mmap(&process, &mut thread, PAGE_BYTES, READ_WRITE, SHARED, fd, 0);
    let weak = match thread.files().file(fd).unwrap().kind() {
        FileKind::MemoryFile(memory) => Arc::downgrade(memory),
        _ => panic!("not a memory file"),
    };
    assert_eq!(
        call(&process, &mut thread, number::CLOSE, [fd, 0, 0, 0, 0, 0]),
        0
    );
    assert_eq!(weak.strong_count(), 1);
    process.write_user_memory(mapped, b"kept").unwrap();
    assert_eq!(
        call(
            &process,
            &mut thread,
            number::MUNMAP,
            [mapped, PAGE_BYTES, 0, 0, 0, 0]
        ),
        0
    );
    assert_eq!(weak.strong_count(), 0);
}

#[test]
fn private_mappings_are_read_only_and_other_files_cannot_be_mapped() {
    let _frames = frames_for_test();
    let kernel = kernel_tables();
    let process = process(&kernel);
    let mut thread = LinuxThread::first(&process);
    let fd = memfd(&process, &mut thread, 0);
    process.write_user_memory(DATA + 64, b"keymap").unwrap();
    assert_eq!(
        call(
            &process,
            &mut thread,
            number::WRITE,
            [fd, DATA + 64, 6, 0, 0, 0]
        ),
        6
    );
    let private = map::PRIVATE;
    let keymap = mmap(
        &process,
        &mut thread,
        PAGE_BYTES,
        prot::READ,
        private,
        fd,
        0,
    );
    assert!(keymap < 1 << 47);
    let mut read = [0u8; 6];
    process.read_user_memory(keymap, &mut read).unwrap();
    assert_eq!(&read, b"keymap");
    assert!(process.write_user_memory(keymap, b"x").is_err());
    assert_eq!(
        call(
            &process,
            &mut thread,
            number::MPROTECT,
            [keymap, PAGE_BYTES, READ_WRITE, 0, 0, 0]
        ),
        Errno::EACCES.result()
    );
    assert_eq!(
        mmap(
            &process,
            &mut thread,
            PAGE_BYTES,
            READ_WRITE,
            private,
            fd,
            0
        ),
        Errno::ENODEV.result()
    );
    assert_eq!(
        mmap(
            &process,
            &mut thread,
            PAGE_BYTES,
            prot::READ | prot::EXEC,
            SHARED,
            fd,
            0
        ),
        Errno::EPERM.result()
    );
    assert_eq!(
        mmap(&process, &mut thread, PAGE_BYTES, 0, SHARED, fd, 0),
        Errno::EPERM.result()
    );
    let shared = mmap(&process, &mut thread, PAGE_BYTES, prot::READ, SHARED, fd, 0);
    assert_eq!(
        call(
            &process,
            &mut thread,
            number::MPROTECT,
            [shared, PAGE_BYTES, READ_WRITE, 0, 0, 0]
        ),
        0
    );
    process.write_user_memory(shared, b"K").unwrap();
    process.read_user_memory(keymap, &mut read[..1]).unwrap();
    assert_eq!(&read[..1], b"K");
    let regular = Arc::new(OpenFile::new(
        FileKind::Regular(crate::vfs::root()),
        open_flag::RDWR,
    ));
    let regular_fd = thread
        .files()
        .lock()
        .open(Arc::clone(&regular), false, 64)
        .unwrap();
    assert_eq!(
        mmap(
            &process,
            &mut thread,
            PAGE_BYTES,
            prot::READ | prot::WRITE,
            SHARED,
            u64::from(regular_fd),
            0
        ),
        Errno::ENODEV.result()
    );
    assert_eq!(
        mmap(&process, &mut thread, PAGE_BYTES, prot::READ, SHARED, 1, 0),
        Errno::EBADF.result()
    );
}

#[test]
fn memfd_create_fallocate_and_the_seal_commands_check_as_linux_does() {
    let _frames = frames_for_test();
    let kernel = kernel_tables();
    let process = process(&kernel);
    let mut thread = LinuxThread::first(&process);
    assert_eq!(
        memfd(&process, &mut thread, flag::HUGETLB),
        Errno::EINVAL.result()
    );
    assert_eq!(memfd(&process, &mut thread, 0x100), Errno::EINVAL.result());
    assert_eq!(
        memfd(&process, &mut thread, flag::NOEXEC_SEAL | flag::EXEC),
        Errno::EINVAL.result()
    );
    let long = vec![b'n'; NAME_MAX + 1];
    process.write_user_memory(DATA + 512, &long).unwrap();
    process
        .write_user_memory(DATA + 512 + long.len() as u64, &[0])
        .unwrap();
    assert_eq!(
        call(
            &process,
            &mut thread,
            number::MEMFD_CREATE,
            [DATA + 512, 0, 0, 0, 0, 0]
        ),
        Errno::EINVAL.result()
    );
    let sealed = memfd(&process, &mut thread, flag::CLOEXEC);
    assert!(
        thread
            .files()
            .lock()
            .get(sealed as u32)
            .unwrap()
            .close_on_exec
    );
    let get = u64::from(command::GET_SEALS);
    let add = u64::from(command::ADD_SEALS);
    assert_eq!(
        call(
            &process,
            &mut thread,
            number::FCNTL,
            [sealed, get, 0, 0, 0, 0]
        ),
        u64::from(seal::SEAL)
    );
    assert_eq!(
        call(
            &process,
            &mut thread,
            number::FCNTL,
            [sealed, add, u64::from(seal::GROW), 0, 0, 0]
        ),
        Errno::EPERM.result()
    );
    let open = memfd(&process, &mut thread, flag::ALLOW_SEALING);
    assert_eq!(
        call(
            &process,
            &mut thread,
            number::FCNTL,
            [open, get, 0, 0, 0, 0]
        ),
        0
    );
    assert_eq!(
        call(
            &process,
            &mut thread,
            number::FCNTL,
            [open, add, u64::from(seal::SHRINK), 0, 0, 0]
        ),
        0
    );
    assert_eq!(
        call(&process, &mut thread, number::FCNTL, [1, get, 0, 0, 0, 0]),
        Errno::EINVAL.result()
    );
    let noexec = memfd(&process, &mut thread, flag::NOEXEC_SEAL);
    assert_eq!(
        call(
            &process,
            &mut thread,
            number::FCNTL,
            [noexec, get, 0, 0, 0, 0]
        ),
        u64::from(seal::EXEC)
    );
    let fallocate = |thread: &mut LinuxThread, fd: u64, mode: u64, offset: u64, length: u64| {
        call(
            &process,
            thread,
            number::FALLOCATE,
            [fd, mode, offset, length, 0, 0],
        )
    };
    assert_eq!(fallocate(&mut thread, open, 0, 0, 3 * PAGE_BYTES), 0);
    let file = thread.files().file(open).unwrap();
    let FileKind::MemoryFile(memory) = file.kind() else {
        panic!("not a memory file")
    };
    assert_eq!(memory.size(), 3 * PAGE_BYTES);
    assert_eq!(
        fallocate(&mut thread, open, 1, 0, 1),
        Errno::EOPNOTSUPP.result()
    );
    assert_eq!(
        fallocate(&mut thread, open, 0x100, 0, 1),
        Errno::EOPNOTSUPP.result()
    );
    assert_eq!(
        fallocate(&mut thread, open, 0, u64::MAX, 1),
        Errno::EINVAL.result()
    );
    assert_eq!(
        fallocate(&mut thread, open, 0, 0, 0),
        Errno::EINVAL.result()
    );
    assert_eq!(fallocate(&mut thread, 40, 0, 0, 1), Errno::EBADF.result());
    assert_eq!(fallocate(&mut thread, 1, 0, 0, 1), Errno::ENODEV.result());
    assert_eq!(
        fallocate(&mut thread, open, 0, 1 << 62, 1 << 62),
        Errno::EFBIG.result()
    );
    let regular = Arc::new(OpenFile::new(
        FileKind::Regular(crate::vfs::root()),
        open_flag::RDWR,
    ));
    let regular_fd = thread
        .files()
        .lock()
        .open(Arc::clone(&regular), false, 64)
        .unwrap();
    assert_eq!(
        fallocate(&mut thread, u64::from(regular_fd), 0, 0, 1),
        Errno::EOPNOTSUPP.result()
    );
    assert_eq!(
        call(
            &process,
            &mut thread,
            number::FTRUNCATE,
            [open, 1, 0, 0, 0, 0]
        ),
        Errno::EPERM.result()
    );
    assert_eq!(
        call(&process, &mut thread, number::LSEEK, [open, 0, 2, 0, 0, 0]),
        3 * PAGE_BYTES
    );
    assert_eq!(
        call(&process, &mut thread, number::FSYNC, [open, 0, 0, 0, 0, 0]),
        0
    );
}
