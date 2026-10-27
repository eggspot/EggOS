//! A small Windows console tool, built from our own source for `x86_64-pc-windows-msvc` and
//! `aarch64-pc-windows-msvc` like `windows-console`, that the Windows programs boot test runs
//! from `eggsh` (D121, D124). It reads its
//! arguments with `CommandLineToArgvW` and writes UTF-16 through `WriteConsoleW`, one
//! `WINDOWS-TOOLS` line per result: `dir <patterns>` lists what each wildcard pattern matches
//! (`FindFirstFileW`), `cat <files or patterns>` prints files read as UTF-8 and converted with
//! `MultiByteToWideChar`, and `checks` runs threads, waits and locks, error codes, UTF-16
//! conversions and the system, path and environment functions.

#![cfg_attr(all(windows, not(test)), no_std, no_main)]

#[cfg(all(windows, not(test)))]
#[allow(clippy::multiple_unsafe_ops_per_block)]
mod tool {
    use core::cell::UnsafeCell;
    use core::ffi::c_void;
    use core::sync::atomic::{AtomicPtr, AtomicU32, Ordering};

    const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
    const GENERIC_READ: u32 = 0x8000_0000;
    const OPEN_EXISTING: u32 = 3;
    const INVALID_HANDLE_VALUE: isize = -1;
    const INFINITE: u32 = u32::MAX;
    const WAIT_OBJECT_0: u32 = 0;
    const WAIT_TIMEOUT: u32 = 0x102;
    const WAIT_FAILED: u32 = u32::MAX;
    const CP_UTF8: u32 = 65001;
    const MB_ERR_INVALID_CHARS: u32 = 0x8;
    const WC_ERR_INVALID_CHARS: u32 = 0x80;
    const CSTR_EQUAL: i32 = 2;
    const CSTR_LESS_THAN: i32 = 1;
    const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
    const FORMAT_MESSAGE_FROM_SYSTEM: u32 = 0x1000;
    const FORMAT_MESSAGE_IGNORE_INSERTS: u32 = 0x200;
    const CREATE_SUSPENDED: u32 = 4;
    const WORKERS: usize = 4;
    const ROUNDS: u32 = 1000;
    const MAX_NAMES: usize = 16;
    const NAME_LENGTH: usize = 64;

    type Handle = isize;

    #[repr(C)]
    struct FindData {
        attributes: u32,
        times: [u32; 6],
        size_high: u32,
        size_low: u32,
        reserved: [u32; 2],
        name: [u16; 260],
        alternate: [u16; 14],
    }

    #[repr(C)]
    struct FileInformation {
        attributes: u32,
        times: [u32; 6],
        volume: u32,
        size_high: u32,
        size_low: u32,
        links: u32,
        index: [u32; 2],
    }

    #[link(name = "kernel32", kind = "raw-dylib")]
    unsafe extern "system" {
        fn GetStdHandle(which: u32) -> Handle;
        fn WriteConsoleW(h: Handle, b: *const u16, n: u32, w: *mut u32, r: *mut c_void) -> i32;
        fn ExitProcess(code: u32) -> !;
        fn GetCommandLineW() -> *const u16;
        fn LocalFree(block: *mut c_void) -> *mut c_void;
        fn GetLastError() -> u32;
        fn SetLastError(code: u32);
        fn FindFirstFileW(name: *const u16, data: *mut FindData) -> Handle;
        fn FindNextFileW(find: Handle, data: *mut FindData) -> i32;
        fn FindClose(find: Handle) -> i32;
        fn CreateFileW(
            name: *const u16,
            access: u32,
            share: u32,
            security: *mut c_void,
            disposition: u32,
            flags: u32,
            template: Handle,
        ) -> Handle;
        fn ReadFile(h: Handle, b: *mut u8, n: u32, r: *mut u32, o: *mut c_void) -> i32;
        fn CloseHandle(h: Handle) -> i32;
        fn GetFileSizeEx(h: Handle, size: *mut i64) -> i32;
        fn GetFileInformationByHandle(h: Handle, info: *mut FileInformation) -> i32;
        fn GetFileAttributesW(name: *const u16) -> u32;
        fn CreateDirectoryW(name: *const u16, security: *mut c_void) -> i32;
        fn DeleteFileW(name: *const u16) -> i32;
        fn RemoveDirectoryW(name: *const u16) -> i32;
        fn GetCurrentDirectoryW(size: u32, buffer: *mut u16) -> u32;
        fn SetCurrentDirectoryW(name: *const u16) -> i32;
        fn GetFullPathNameW(
            name: *const u16,
            size: u32,
            buffer: *mut u16,
            part: *mut *mut u16,
        ) -> u32;
        fn GetModuleFileNameW(module: usize, buffer: *mut u16, size: u32) -> u32;
        fn GetEnvironmentVariableW(name: *const u16, buffer: *mut u16, size: u32) -> u32;
        fn SetEnvironmentVariableW(name: *const u16, value: *const u16) -> i32;
        fn GetEnvironmentStringsW() -> *mut u16;
        fn FreeEnvironmentStringsW(block: *mut u16) -> i32;
        fn MultiByteToWideChar(
            page: u32,
            flags: u32,
            bytes: *const u8,
            count: i32,
            out: *mut u16,
            out_count: i32,
        ) -> i32;
        fn WideCharToMultiByte(
            page: u32,
            flags: u32,
            units: *const u16,
            count: i32,
            out: *mut u8,
            out_count: i32,
            default: *const u8,
            used: *mut i32,
        ) -> i32;
        fn lstrlenW(text: *const u16) -> i32;
        fn CompareStringOrdinal(a: *const u16, na: i32, b: *const u16, nb: i32, fold: i32) -> i32;
        fn FormatMessageW(
            flags: u32,
            source: *const c_void,
            id: u32,
            language: u32,
            buffer: *mut u16,
            size: u32,
            arguments: *mut c_void,
        ) -> u32;
        fn CreateThread(
            security: *mut c_void,
            stack: usize,
            start: extern "system" fn(usize) -> u32,
            parameter: usize,
            flags: u32,
            id: *mut u32,
        ) -> Handle;
        fn ResumeThread(thread: Handle) -> u32;
        fn ExitThread(code: u32) -> !;
        fn GetExitCodeThread(thread: Handle, code: *mut u32) -> i32;
        fn GetCurrentThreadId() -> u32;
        fn WaitForSingleObject(h: Handle, milliseconds: u32) -> u32;
        fn WaitForMultipleObjects(n: u32, h: *const Handle, all: i32, milliseconds: u32) -> u32;
        fn CreateEventW(
            security: *mut c_void,
            manual: i32,
            initial: i32,
            name: *const u16,
        ) -> Handle;
        fn SetEvent(event: Handle) -> i32;
        fn CreateMutexW(security: *mut c_void, owner: i32, name: *const u16) -> Handle;
        fn ReleaseMutex(mutex: Handle) -> i32;
        fn CreateSemaphoreW(
            security: *mut c_void,
            initial: i32,
            maximum: i32,
            name: *const u16,
        ) -> Handle;
        fn ReleaseSemaphore(semaphore: Handle, count: i32, previous: *mut i32) -> i32;
        fn InitializeCriticalSection(section: *mut [u64; 5]);
        fn EnterCriticalSection(section: *mut [u64; 5]);
        fn LeaveCriticalSection(section: *mut [u64; 5]);
        fn DeleteCriticalSection(section: *mut [u64; 5]);
        fn AcquireSRWLockExclusive(lock: *mut usize);
        fn ReleaseSRWLockExclusive(lock: *mut usize);
        fn InitOnceExecuteOnce(
            once: *mut usize,
            function: extern "system" fn(*mut usize, usize, *mut usize) -> i32,
            parameter: usize,
            context: *mut usize,
        ) -> i32;
        fn TlsAlloc() -> u32;
        fn TlsSetValue(index: u32, value: usize) -> i32;
        fn TlsGetValue(index: u32) -> usize;
        fn FlsAlloc(callback: Option<extern "system" fn(usize)>) -> u32;
        fn FlsSetValue(index: u32, value: usize) -> i32;
        fn FlsGetValue(index: u32) -> usize;
        fn Sleep(milliseconds: u32);
        fn SwitchToThread() -> i32;
        fn GetSystemInfo(info: *mut [u32; 12]);
        fn GetVersionExW(info: *mut [u32; 71]) -> i32;
        fn FileTimeToSystemTime(file_time: *const u64, time: *mut [u16; 8]) -> i32;
        fn GetLocalTime(time: *mut [u16; 8]);
        fn GetCurrentThread() -> Handle;
        fn InitializeConditionVariable(variable: *mut usize);
        fn SleepConditionVariableCS(
            variable: *mut usize,
            section: *mut [u64; 5],
            milliseconds: u32,
        ) -> i32;
        fn WakeConditionVariable(variable: *mut usize);
        fn AcquireSRWLockShared(lock: *mut usize);
        fn ReleaseSRWLockShared(lock: *mut usize);
        fn TryAcquireSRWLockShared(lock: *mut usize) -> u8;
        fn TryAcquireSRWLockExclusive(lock: *mut usize) -> u8;
        fn InitOnceBeginInitialize(
            once: *mut usize,
            flags: u32,
            pending: *mut i32,
            context: *mut usize,
        ) -> i32;
    }

    #[link(name = "api-ms-win-core-synch-l1-2-0", kind = "raw-dylib")]
    unsafe extern "system" {
        fn WaitOnAddress(
            address: *const c_void,
            compare: *const c_void,
            size: usize,
            milliseconds: u32,
        ) -> i32;
        fn WakeByAddressSingle(address: *const c_void);
    }

    #[link(name = "shell32", kind = "raw-dylib")]
    unsafe extern "system" {
        fn CommandLineToArgvW(line: *const u16, count: *mut i32) -> *mut *mut u16;
    }

    #[link(name = "ntdll", kind = "raw-dylib")]
    unsafe extern "C" {
        fn memcpy(destination: *mut c_void, source: *const c_void, length: usize) -> *mut c_void;
        fn memmove(destination: *mut c_void, source: *const c_void, length: usize) -> *mut c_void;
        fn memset(destination: *mut c_void, value: i32, length: usize) -> *mut c_void;
        fn memcmp(a: *const c_void, b: *const c_void, length: usize) -> i32;
        fn __chkstk();
    }

    #[panic_handler]
    fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
        // SAFETY: `ExitProcess` ends the process.
        unsafe { ExitProcess(101) }
    }

    struct Line {
        units: [u16; 1024],
        length: usize,
    }

    impl Line {
        fn new() -> Self {
            let mut line = Self {
                units: [0; 1024],
                length: 0,
            };
            line.text("WINDOWS-TOOLS ");
            line
        }

        fn text(&mut self, text: &str) -> &mut Self {
            for unit in text.encode_utf16() {
                self.wide(&[unit]);
            }
            self
        }

        fn wide(&mut self, units: &[u16]) -> &mut Self {
            let taken = units.len().min(self.units.len() - self.length);
            self.units[self.length..self.length + taken].copy_from_slice(&units[..taken]);
            self.length += taken;
            self
        }

        fn number(&mut self, mut value: u64) -> &mut Self {
            let mut digits = [0u16; 20];
            let mut first = digits.len();
            loop {
                first -= 1;
                digits[first] = u16::from(b'0') + (value % 10) as u16;
                value /= 10;
                if value == 0 {
                    break;
                }
            }
            self.wide(&digits[first..])
        }

        fn print(&mut self) {
            self.text("\n");
            let mut written = 0;
            // SAFETY: the units are this line's own; `written` is a local.
            unsafe {
                WriteConsoleW(
                    GetStdHandle(STD_OUTPUT_HANDLE),
                    self.units.as_ptr(),
                    self.length as u32,
                    &mut written,
                    core::ptr::null_mut(),
                )
            };
        }
    }

    struct Wide<const N: usize>([u16; N]);

    impl<const N: usize> Wide<N> {
        fn new(text: &str) -> Self {
            let mut units = [0; N];
            for (unit, value) in units.iter_mut().zip(text.encode_utf16()) {
                *unit = value;
            }
            Self(units)
        }

        fn as_ptr(&self) -> *const u16 {
            self.0.as_ptr()
        }
    }

    fn terminated(text: *const u16) -> &'static [u16] {
        // SAFETY: the callers pass NUL-terminated strings the layer keeps alive.
        let length = unsafe { lstrlenW(text) } as usize;
        // SAFETY: as above.
        unsafe { core::slice::from_raw_parts(text, length) }
    }

    fn last_error() -> u32 {
        // SAFETY: reads the calling thread's last error.
        unsafe { GetLastError() }
    }

    #[unsafe(no_mangle)]
    extern "system" fn mainCRTStartup() -> u32 {
        core::hint::black_box([
            memcpy as *const () as usize,
            memmove as *const () as usize,
            memset as *const () as usize,
            memcmp as *const () as usize,
            __chkstk as *const () as usize,
        ]);
        let mut count = 0;
        // SAFETY: the command line is the process's; the array is freed with `LocalFree`.
        let arguments = unsafe { CommandLineToArgvW(GetCommandLineW(), &mut count) };
        if arguments.is_null() || count < 2 {
            Line::new()
                .text("usage: windows-tools dir|cat|checks")
                .print();
            return 2;
        }
        // SAFETY: `CommandLineToArgvW` returns `count` pointers to NUL-terminated arguments.
        let arguments = unsafe { core::slice::from_raw_parts(arguments, count as usize) };
        let command = terminated(arguments[1]);
        let rest = &arguments[2..];
        let status = if wide_eq(command, "dir") {
            dir(rest)
        } else if wide_eq(command, "cat") {
            cat(rest)
        } else if wide_eq(command, "detach") {
            detach()
        } else {
            checks()
        };
        // SAFETY: the array `CommandLineToArgvW` returned.
        unsafe { LocalFree(arguments.as_ptr() as *mut c_void) };
        status
    }

    struct Names {
        names: [[u16; NAME_LENGTH]; MAX_NAMES],
        lengths: [usize; MAX_NAMES],
        count: usize,
    }

    impl Names {
        fn push(&mut self, name: &[u16], directory: bool) {
            if self.count == MAX_NAMES {
                return;
            }
            let length = name.len().min(NAME_LENGTH - 1);
            self.names[self.count][..length].copy_from_slice(&name[..length]);
            let mut total = length;
            if directory {
                self.names[self.count][length] = u16::from(b'\\');
                total += 1;
            }
            self.lengths[self.count] = total;
            self.count += 1;
        }

        fn sort(&mut self) {
            for end in (1..self.count).rev() {
                for at in 0..end {
                    let a = &self.names[at];
                    let b = &self.names[at + 1];
                    // SAFETY: both names hold their lengths of units.
                    let order = unsafe {
                        CompareStringOrdinal(
                            a.as_ptr(),
                            self.lengths[at] as i32,
                            b.as_ptr(),
                            self.lengths[at + 1] as i32,
                            1,
                        )
                    };
                    if order > CSTR_EQUAL {
                        self.names.swap(at, at + 1);
                        self.lengths.swap(at, at + 1);
                    }
                }
            }
        }
    }

    fn name_of(data: &FindData) -> &[u16] {
        let length = data.name.iter().position(|unit| *unit == 0).unwrap_or(0);
        &data.name[..length]
    }

    fn find(pattern: *const u16, names: &mut Names) -> Result<(), u32> {
        // SAFETY: zeros are a valid `WIN32_FIND_DATAW`.
        let mut data: FindData = unsafe { core::mem::zeroed() };
        // SAFETY: the pattern is NUL-terminated; `data` is a local.
        let search = unsafe { FindFirstFileW(pattern, &mut data) };
        if search == INVALID_HANDLE_VALUE {
            return Err(last_error());
        }
        loop {
            names.push(
                name_of(&data),
                data.attributes & FILE_ATTRIBUTE_DIRECTORY != 0,
            );
            // SAFETY: `search` is open; `data` is a local.
            if unsafe { FindNextFileW(search, &mut data) } == 0 {
                break;
            }
        }
        let end = last_error();
        // SAFETY: `search` is open.
        unsafe { FindClose(search) };
        match end {
            18 => Ok(()),
            code => Err(code),
        }
    }

    fn dir(patterns: &[*mut u16]) -> u32 {
        let mut status = 0;
        for &pattern in patterns {
            let mut line = Line::new();
            line.text("dir ").wide(terminated(pattern)).text(":");
            let mut names = Names {
                names: [[0; NAME_LENGTH]; MAX_NAMES],
                lengths: [0; MAX_NAMES],
                count: 0,
            };
            match find(pattern, &mut names) {
                Ok(()) => {
                    names.sort();
                    for index in 0..names.count {
                        line.text(" ")
                            .wide(&names.names[index][..names.lengths[index]]);
                    }
                }
                Err(code) => {
                    line.text(" error ").number(u64::from(code));
                    status = 1;
                }
            }
            line.print();
        }
        status
    }

    fn message(code: u32, line: &mut Line) {
        let mut text = [0u16; 256];
        // SAFETY: the buffer holds 256 units.
        let length = unsafe {
            FormatMessageW(
                FORMAT_MESSAGE_FROM_SYSTEM | FORMAT_MESSAGE_IGNORE_INSERTS,
                core::ptr::null(),
                code,
                0,
                text.as_mut_ptr(),
                256,
                core::ptr::null_mut(),
            )
        } as usize;
        let message = &text[..length];
        let trimmed = message
            .strip_suffix(&[u16::from(b'\r'), u16::from(b'\n')])
            .unwrap_or(message);
        line.wide(trimmed);
    }

    fn print_file(path: &[u16], label: &[u16]) -> Result<(), u32> {
        let mut name = [0u16; 512];
        let length = path.len().min(511);
        name[..length].copy_from_slice(&path[..length]);
        // SAFETY: the name is NUL-terminated.
        let file = unsafe {
            CreateFileW(
                name.as_ptr(),
                GENERIC_READ,
                1,
                core::ptr::null_mut(),
                OPEN_EXISTING,
                0,
                0,
            )
        };
        if file == INVALID_HANDLE_VALUE {
            return Err(last_error());
        }
        let mut bytes = [0u8; 2048];
        let mut read = 0;
        let mut size = 0;
        // SAFETY: the buffer and counts are locals; the file is open.
        let ok = unsafe {
            ReadFile(
                file,
                bytes.as_mut_ptr(),
                2048,
                &mut read,
                core::ptr::null_mut(),
            )
        } != 0;
        // SAFETY: as above.
        let sized = unsafe { GetFileSizeEx(file, &mut size) } != 0;
        // SAFETY: the file is open.
        unsafe { CloseHandle(file) };
        if !ok || !sized || size != i64::from(read) {
            return Err(13);
        }
        let mut units = [0u16; 2048];
        // SAFETY: `bytes` holds `read` bytes and `units` 2048 units.
        let converted = unsafe {
            MultiByteToWideChar(
                CP_UTF8,
                MB_ERR_INVALID_CHARS,
                bytes.as_ptr(),
                read as i32,
                units.as_mut_ptr(),
                2048,
            )
        };
        if converted <= 0 && read > 0 {
            return Err(last_error());
        }
        for text_line in units[..converted.max(0) as usize].split(|unit| *unit == u16::from(b'\n'))
        {
            if !text_line.is_empty() {
                Line::new().wide(label).text(": ").wide(text_line).print();
            }
        }
        Ok(())
    }

    fn has_wildcard(path: &[u16]) -> bool {
        path.iter()
            .any(|unit| *unit == u16::from(b'*') || *unit == u16::from(b'?'))
    }

    fn cat(paths: &[*mut u16]) -> u32 {
        let mut status = 0;
        for &path in paths {
            let path = terminated(path);
            let result = if has_wildcard(path) {
                cat_matches(path)
            } else {
                print_file(path, path)
            };
            if let Err(code) = result {
                let mut line = Line::new();
                line.text("cat ")
                    .wide(path)
                    .text(": error ")
                    .number(u64::from(code))
                    .text(" ");
                message(code, &mut line);
                line.print();
                status = 1;
            }
        }
        status
    }

    fn cat_matches(pattern: &[u16]) -> Result<(), u32> {
        let mut terminated_pattern = [0u16; 512];
        let length = pattern.len().min(511);
        terminated_pattern[..length].copy_from_slice(&pattern[..length]);
        let mut names = Names {
            names: [[0; NAME_LENGTH]; MAX_NAMES],
            lengths: [0; MAX_NAMES],
            count: 0,
        };
        find(terminated_pattern.as_ptr(), &mut names)?;
        names.sort();
        let directory = pattern
            .iter()
            .rposition(|unit| *unit == u16::from(b'\\'))
            .map_or(0, |at| at + 1);
        for index in 0..names.count {
            let name = &names.names[index][..names.lengths[index]];
            let mut path = [0u16; 512];
            path[..directory].copy_from_slice(&pattern[..directory]);
            path[directory..directory + name.len()].copy_from_slice(name);
            print_file(&path[..directory + name.len()], name)?;
        }
        Ok(())
    }

    struct Shared {
        start: Handle,
        mutex: Handle,
        semaphore: Handle,
        section: UnsafeCell<[u64; 5]>,
        srw: UnsafeCell<usize>,
        once: UnsafeCell<usize>,
        tls: u32,
        fls: u32,
        interlocked: AtomicU32,
        in_section: UnsafeCell<u32>,
        in_mutex: UnsafeCell<u32>,
        in_srw: UnsafeCell<u32>,
        once_runs: AtomicU32,
        fls_cleanups: AtomicU32,
        main_thread: u32,
        failures: AtomicU32,
    }

    // SAFETY: the cells are touched only under the lock each stands for.
    unsafe impl Sync for Shared {}

    static SHARED: AtomicPtr<Shared> = AtomicPtr::new(core::ptr::null_mut());

    fn shared() -> &'static Shared {
        // SAFETY: `threads` publishes its `Shared` before any worker starts and keeps it alive
        // until every worker has ended.
        unsafe { SHARED.load(Ordering::Acquire).as_ref() }.unwrap_or_else(|| panic!())
    }

    extern "system" fn once_function(_: *mut usize, parameter: usize, context: *mut usize) -> i32 {
        shared().once_runs.fetch_add(1, Ordering::Relaxed);
        // SAFETY: `InitOnceExecuteOnce` passes a writable context.
        unsafe { context.write(parameter << 2) };
        1
    }

    extern "system" fn fls_cleanup(value: usize) {
        if value != 0 {
            shared().fls_cleanups.fetch_add(1, Ordering::Relaxed);
        }
    }

    extern "system" fn worker(index: usize) -> u32 {
        let shared = shared();
        let fail = || {
            shared.failures.fetch_add(1, Ordering::Relaxed);
        };
        // SAFETY: the handles, the locks and the slots are the shared ones, set up by main.
        unsafe {
            if WaitForSingleObject(shared.start, INFINITE) != WAIT_OBJECT_0 {
                fail();
            }
            TlsSetValue(shared.tls, index + 1);
            FlsSetValue(shared.fls, index + 100);
            SetLastError(1000 + index as u32);
            let mut context = 0;
            InitOnceExecuteOnce(shared.once.get(), once_function, 7, &mut context);
            if context != 7 << 2 {
                fail();
            }
            if GetCurrentThreadId() == shared.main_thread {
                fail();
            }
        }
        for round in 0..ROUNDS {
            shared.interlocked.fetch_add(1, Ordering::SeqCst);
            // SAFETY: the critical section guards `in_section`.
            unsafe {
                EnterCriticalSection(shared.section.get());
                *shared.in_section.get() += 1;
                LeaveCriticalSection(shared.section.get());
            }
            // SAFETY: the mutex guards `in_mutex`.
            unsafe {
                if WaitForSingleObject(shared.mutex, INFINITE) != WAIT_OBJECT_0 {
                    fail();
                }
                *shared.in_mutex.get() += 1;
                ReleaseMutex(shared.mutex);
            }
            // SAFETY: the SRW lock guards `in_srw`.
            unsafe {
                AcquireSRWLockExclusive(shared.srw.get());
                *shared.in_srw.get() += 1;
                ReleaseSRWLockExclusive(shared.srw.get());
            }
            if round % 250 == 0 {
                // SAFETY: yields the CPU.
                unsafe { SwitchToThread() };
            }
        }
        // SAFETY: the slots are the shared ones.
        unsafe {
            if GetLastError() != 1000 + index as u32 {
                fail();
            }
            if TlsGetValue(shared.tls) != index + 1 || FlsGetValue(shared.fls) != index + 100 {
                fail();
            }
            ReleaseSemaphore(shared.semaphore, 1, core::ptr::null_mut());
        }
        10 + index as u32
    }

    extern "system" fn exiting(_: usize) -> u32 {
        // SAFETY: ends this thread with code 77.
        unsafe { ExitThread(77) }
    }

    fn threads() {
        let null = core::ptr::null_mut();
        // SAFETY: the objects are created with no names or security attributes.
        let shared = unsafe {
            Shared {
                start: CreateEventW(null, 1, 0, core::ptr::null()),
                mutex: CreateMutexW(null, 0, core::ptr::null()),
                semaphore: CreateSemaphoreW(null, 0, WORKERS as i32, core::ptr::null()),
                section: UnsafeCell::new([0; 5]),
                srw: UnsafeCell::new(0),
                once: UnsafeCell::new(0),
                tls: TlsAlloc(),
                fls: FlsAlloc(Some(fls_cleanup)),
                interlocked: AtomicU32::new(0),
                in_section: UnsafeCell::new(0),
                in_mutex: UnsafeCell::new(0),
                in_srw: UnsafeCell::new(0),
                once_runs: AtomicU32::new(0),
                fls_cleanups: AtomicU32::new(0),
                main_thread: GetCurrentThreadId(),
                failures: AtomicU32::new(0),
            }
        };
        // SAFETY: the critical section is `shared`'s own.
        unsafe { InitializeCriticalSection(shared.section.get()) };
        SHARED.store(core::ptr::from_ref(&shared).cast_mut(), Ordering::Release);
        let shared = &shared;
        let mut handles = [0isize; WORKERS];
        for (index, handle) in handles.iter_mut().enumerate() {
            let mut id = 0;
            // SAFETY: `worker` is a thread start routine; `id` is a local.
            *handle = unsafe { CreateThread(null, 0, worker, index, 0, &mut id) };
            if *handle == 0 || id == 0 {
                Line::new().text("threads CreateThread failed").print();
                return;
            }
        }
        // SAFETY: the handles are the workers' and the event's.
        let (early, joined) = unsafe {
            let early = WaitForMultipleObjects(WORKERS as u32, handles.as_ptr(), 1, 20);
            SetEvent(shared.start);
            let joined = WaitForMultipleObjects(WORKERS as u32, handles.as_ptr(), 1, 30_000);
            (early, joined)
        };
        let mut line = Line::new();
        line.text("threads ").number(WORKERS as u64);
        if early != WAIT_TIMEOUT || joined != WAIT_OBJECT_0 {
            line.text(" wait failed ")
                .number(u64::from(early))
                .text(" ");
            line.number(u64::from(joined)).print();
            return;
        }
        line.text(" joined exits");
        for handle in handles {
            let mut code = 0;
            // SAFETY: the worker's handle.
            unsafe { GetExitCodeThread(handle, &mut code) };
            line.text(" ").number(u64::from(code));
            // SAFETY: as above.
            unsafe { CloseHandle(handle) };
        }
        // SAFETY: the shared cells, read after every worker ended.
        let (section, mutex, srw) = unsafe {
            (
                *shared.in_section.get(),
                *shared.in_mutex.get(),
                *shared.in_srw.get(),
            )
        };
        line.text(" interlocked ")
            .number(u64::from(shared.interlocked.load(Ordering::SeqCst)));
        line.text(" critical ").number(u64::from(section));
        line.text(" mutex ").number(u64::from(mutex));
        line.text(" srw ").number(u64::from(srw));
        line.text(" once ")
            .number(u64::from(shared.once_runs.load(Ordering::SeqCst)));
        line.print();
        let mut posts = 0;
        // SAFETY: the semaphore's handle.
        while unsafe { WaitForSingleObject(shared.semaphore, 0) } == WAIT_OBJECT_0 {
            posts += 1;
        }
        let mut id = 0;
        // SAFETY: `exiting` is a thread start routine; `id` is a local.
        let suspended = unsafe { CreateThread(null, 0, exiting, 0, CREATE_SUSPENDED, &mut id) };
        let mut code = 0;
        // SAFETY: the suspended thread's handle.
        let (before, resumed, ended) = unsafe {
            let before = WaitForSingleObject(suspended, 20);
            let resumed = ResumeThread(suspended);
            let ended = WaitForSingleObject(suspended, 10_000);
            GetExitCodeThread(suspended, &mut code);
            CloseHandle(suspended);
            (before, resumed, ended)
        };
        let mut line = Line::new();
        line.text("threads semaphore ").number(posts);
        line.text(" suspended ").text(
            if before == WAIT_TIMEOUT && resumed == 1 && ended == WAIT_OBJECT_0 {
                "ok"
            } else {
                "failed"
            },
        );
        line.text(" exit-thread ").number(u64::from(code));
        line.text(" fls-cleanups ")
            .number(u64::from(shared.fls_cleanups.load(Ordering::SeqCst)));
        line.text(" per-thread tls and last error ").text(
            if shared.failures.load(Ordering::SeqCst) == 0 {
                "ok"
            } else {
                "failed"
            },
        );
        line.print();
        // SAFETY: the critical section no worker uses any more.
        unsafe { DeleteCriticalSection(shared.section.get()) };
    }

    fn errors() {
        let mut line = Line::new();
        line.text("errors");
        let mut push = |code: u32| {
            line.text(" ").number(u64::from(code));
        };
        let open = |path: &str| {
            let name = Wide::<64>::new(path);
            // SAFETY: the name is NUL-terminated.
            let handle = unsafe {
                CreateFileW(
                    name.as_ptr(),
                    GENERIC_READ,
                    0,
                    core::ptr::null_mut(),
                    OPEN_EXISTING,
                    0,
                    0,
                )
            };
            if handle == INVALID_HANDLE_VALUE {
                last_error()
            } else {
                0
            }
        };
        push(open("C:\\etc\\no-such-file\0"));
        push(open("C:\\no\\such\\file\0"));
        // SAFETY: every name is NUL-terminated and every buffer a local of the size given.
        unsafe {
            let etc = Wide::<16>::new("C:\\etc\0");
            push(GetFileAttributesW(etc.as_ptr()) & FILE_ATTRIBUTE_DIRECTORY);
            CreateDirectoryW(etc.as_ptr(), core::ptr::null_mut());
            push(last_error());
            DeleteFileW(Wide::<32>::new("C:\\etc\\no-such-file\0").as_ptr());
            push(last_error());
            RemoveDirectoryW(Wide::<32>::new("C:\\etc\\hostname\0").as_ptr());
            push(last_error());
            CloseHandle(0x1234);
            push(last_error());
            let mut value = [0u16; 4];
            GetEnvironmentVariableW(
                Wide::<16>::new("NO_SUCH_NAME\0").as_ptr(),
                value.as_mut_ptr(),
                4,
            );
            push(last_error());
            SetEnvironmentVariableW(
                Wide::<16>::new("LONG\0").as_ptr(),
                Wide::<16>::new("123456789\0").as_ptr(),
            );
            push(GetEnvironmentVariableW(
                Wide::<16>::new("LONG\0").as_ptr(),
                value.as_mut_ptr(),
                4,
            ));
            let mut small = [0u16; 2];
            MultiByteToWideChar(CP_UTF8, 0, b"abc".as_ptr(), 3, small.as_mut_ptr(), 2);
            push(last_error());
            push(u32::from(WaitForSingleObject(0x1234, 0) == WAIT_FAILED));
            push(last_error());
            let mutex = CreateMutexW(core::ptr::null_mut(), 0, core::ptr::null());
            ReleaseMutex(mutex);
            push(last_error());
            CloseHandle(mutex);
            let semaphore = CreateSemaphoreW(core::ptr::null_mut(), 1, 1, core::ptr::null());
            ReleaseSemaphore(semaphore, 1, core::ptr::null_mut());
            push(last_error());
            CloseHandle(semaphore);
            SetLastError(0);
            push(last_error());
        }
        line.print();
        let mut line = Line::new();
        line.text("message 2 ");
        message(2, &mut line);
        line.print();
    }

    fn text() {
        let original = "grüße 🥚 Ελληνικά";
        let bytes = original.as_bytes();
        let mut units = [0u16; 64];
        let mut back = [0u8; 64];
        let mut line = Line::new();
        // SAFETY: the buffers are locals of the sizes given.
        unsafe {
            let needed = MultiByteToWideChar(
                CP_UTF8,
                0,
                bytes.as_ptr(),
                bytes.len() as i32,
                core::ptr::null_mut(),
                0,
            );
            let converted = MultiByteToWideChar(
                CP_UTF8,
                0,
                bytes.as_ptr(),
                bytes.len() as i32,
                units.as_mut_ptr(),
                64,
            );
            let back_needed = WideCharToMultiByte(
                CP_UTF8,
                0,
                units.as_ptr(),
                converted,
                core::ptr::null_mut(),
                0,
                core::ptr::null(),
                core::ptr::null_mut(),
            );
            let back_length = WideCharToMultiByte(
                CP_UTF8,
                0,
                units.as_ptr(),
                converted,
                back.as_mut_ptr(),
                64,
                core::ptr::null(),
                core::ptr::null_mut(),
            );
            let expected = original.encode_utf16().count() as i32;
            let same = needed == expected
                && converted == expected
                && back_needed == bytes.len() as i32
                && &back[..back_length as usize] == bytes;
            line.text("text ").wide(&units[..converted.max(0) as usize]);
            line.text(" units ").number(converted as u64);
            line.text(" bytes ").number(back_length as u64);
            line.text(if same {
                " round trip ok"
            } else {
                " round trip failed"
            });
            let invalid = MultiByteToWideChar(
                CP_UTF8,
                MB_ERR_INVALID_CHARS,
                b"a\xffb".as_ptr(),
                3,
                units.as_mut_ptr(),
                64,
            );
            line.text(" invalid ")
                .number(u64::from(invalid == 0))
                .text(" ");
            line.number(u64::from(last_error()));
            let unpaired = [0x61u16, 0xd800];
            let refused = WideCharToMultiByte(
                CP_UTF8,
                WC_ERR_INVALID_CHARS,
                unpaired.as_ptr(),
                2,
                back.as_mut_ptr(),
                64,
                core::ptr::null(),
                core::ptr::null_mut(),
            );
            line.text(" unpaired ")
                .number(u64::from(refused == 0))
                .text(" ");
            line.number(u64::from(last_error()));
            let upper = Wide::<8>::new("ÉCOLE\0");
            let lower = Wide::<8>::new("école\0");
            let folded = CompareStringOrdinal(upper.as_ptr(), -1, lower.as_ptr(), -1, 1);
            let exact = CompareStringOrdinal(upper.as_ptr(), -1, lower.as_ptr(), -1, 0);
            line.text(" ordinal ")
                .text(if folded == CSTR_EQUAL && exact == CSTR_LESS_THAN {
                    "ok"
                } else {
                    "failed"
                });
        }
        line.print();
    }

    fn wide_eq(units: &[u16], text: &str) -> bool {
        units.iter().copied().eq(text.encode_utf16())
    }

    fn system() {
        let mut info = [0u32; 12];
        let mut version = [0u32; 71];
        version[0] = 276;
        let mut module = [0u16; 128];
        let mut line = Line::new();
        // SAFETY: the buffers are locals of the sizes given.
        unsafe {
            GetSystemInfo(&mut info);
            GetVersionExW(&mut version);
            let length = GetModuleFileNameW(0, module.as_mut_ptr(), 128) as usize;
            line.text("system arch ")
                .number(u64::from(info[0] & 0xffff));
            line.text(" page ").number(u64::from(info[1]));
            line.text(" granularity ").number(u64::from(info[10]));
            line.text(" cpus ")
                .text(if info[8] >= 1 { "some" } else { "none" });
            line.text(" version ")
                .number(u64::from(version[1]))
                .text(".");
            line.number(u64::from(version[2]))
                .text(".")
                .number(u64::from(version[3]));
            line.text(" module ").wide(&module[..length]);
        }
        line.print();
        let mut line = Line::new();
        // SAFETY: as above; the names are NUL-terminated.
        unsafe {
            let epoch = 116_444_736_000_000_000u64;
            let mut time = [0u16; 8];
            FileTimeToSystemTime(&epoch, &mut time);
            line.text("time epoch ")
                .number(u64::from(time[0]))
                .text("-");
            line.number(u64::from(time[1]))
                .text("-")
                .number(u64::from(time[3]));
            line.text(" weekday ").number(u64::from(time[2]));
            GetLocalTime(&mut time);
            let valid =
                time[0] >= 1970 && (1..=12).contains(&time[1]) && (1..=31).contains(&time[3]);
            line.text(" now ").text(if valid { "ok" } else { "failed" });
            Sleep(1);
        }
        line.print();
        let mut line = Line::new();
        // SAFETY: as above.
        unsafe {
            SetCurrentDirectoryW(Wide::<16>::new("C:\\etc\0").as_ptr());
            let mut directory = [0u16; 64];
            let length = GetCurrentDirectoryW(64, directory.as_mut_ptr()) as usize;
            line.text("cwd ").wide(&directory[..length.min(64)]);
            let mut full = [0u16; 64];
            let mut part = core::ptr::null_mut();
            let length = GetFullPathNameW(
                Wide::<32>::new("..\\etc\\.\\hostname\0").as_ptr(),
                64,
                full.as_mut_ptr(),
                &mut part,
            ) as usize;
            line.text(" full ").wide(&full[..length.min(64)]);
            line.text(" part ").wide(terminated(part));
            let file = CreateFileW(
                Wide::<16>::new("hostname\0").as_ptr(),
                GENERIC_READ,
                0,
                core::ptr::null_mut(),
                OPEN_EXISTING,
                0,
                0,
            );
            let mut information: FileInformation = core::mem::zeroed();
            let mut size = 0;
            let ok = file != INVALID_HANDLE_VALUE
                && GetFileInformationByHandle(file, &mut information) != 0
                && GetFileSizeEx(file, &mut size) != 0
                && i64::from(information.size_low) == size
                && information.links >= 1;
            CloseHandle(file);
            line.text(" relative open ")
                .text(if ok { "ok" } else { "failed" });
            let needed = GetCurrentDirectoryW(2, directory.as_mut_ptr());
            line.text(" small buffer ").number(u64::from(needed));
            SetCurrentDirectoryW(Wide::<4>::new("C:\\\0").as_ptr());
        }
        line.print();
        let mut line = Line::new();
        // SAFETY: as above; the environment block is freed with `FreeEnvironmentStringsW`.
        unsafe {
            let name = Wide::<8>::new("EGG\0");
            SetEnvironmentVariableW(name.as_ptr(), Wide::<16>::new("yolk ü\0").as_ptr());
            let mut value = [0u16; 32];
            let length = GetEnvironmentVariableW(name.as_ptr(), value.as_mut_ptr(), 32) as usize;
            line.text("env EGG=").wide(&value[..length.min(32)]);
            let block = GetEnvironmentStringsW();
            let mut at = block;
            let mut found = false;
            while *at != 0 {
                let entry = terminated(at);
                found |= wide_eq(entry, "EGG=yolk ü");
                at = at.add(entry.len() + 1);
            }
            FreeEnvironmentStringsW(block);
            line.text(" block ")
                .text(if found { "ok" } else { "failed" });
            SetEnvironmentVariableW(name.as_ptr(), core::ptr::null());
            let gone = GetEnvironmentVariableW(name.as_ptr(), value.as_mut_ptr(), 32);
            line.text(" removed ").number(u64::from(gone)).text(" ");
            line.number(u64::from(last_error()));
        }
        line.print();
    }

    struct SyncState {
        flag: AtomicU32,
        ready: AtomicU32,
        condition: UnsafeCell<usize>,
        section: UnsafeCell<[u64; 5]>,
        lock: UnsafeCell<usize>,
        shared_taken: AtomicU32,
        exclusive_refused: AtomicU32,
    }

    // SAFETY: the cells are the Win32 objects' own memory, which only their functions touch.
    unsafe impl Sync for SyncState {}

    static SYNC: SyncState = SyncState {
        flag: AtomicU32::new(0),
        ready: AtomicU32::new(0),
        condition: UnsafeCell::new(0),
        section: UnsafeCell::new([0; 5]),
        lock: UnsafeCell::new(0),
        shared_taken: AtomicU32::new(0),
        exclusive_refused: AtomicU32::new(0),
    };

    const SIGNAL_ADDRESS: usize = 0;
    const SIGNAL_CONDITION: usize = 1;
    const TRY_SHARED_LOCK: usize = 2;

    extern "system" fn helper(mode: usize) -> u32 {
        match mode {
            SIGNAL_ADDRESS => {
                // SAFETY: sleeps, then wakes a thread waiting on the flag's address.
                unsafe { Sleep(20) };
                SYNC.flag.store(1, Ordering::Release);
                // SAFETY: as above.
                unsafe { WakeByAddressSingle((&raw const SYNC.flag).cast()) };
            }
            SIGNAL_CONDITION => {
                // SAFETY: the critical section and condition variable `sync` initialized.
                unsafe {
                    Sleep(20);
                    EnterCriticalSection(SYNC.section.get());
                    SYNC.ready.store(1, Ordering::Release);
                    LeaveCriticalSection(SYNC.section.get());
                    WakeConditionVariable(SYNC.condition.get());
                }
            }
            _ => {
                // SAFETY: the SRW lock, which the main thread holds shared.
                unsafe {
                    if TryAcquireSRWLockShared(SYNC.lock.get()) != 0 {
                        SYNC.shared_taken.store(1, Ordering::Release);
                        ReleaseSRWLockShared(SYNC.lock.get());
                    }
                    if TryAcquireSRWLockExclusive(SYNC.lock.get()) == 0 {
                        SYNC.exclusive_refused.store(1, Ordering::Release);
                    }
                }
            }
        }
        0
    }

    fn run_helper(mode: usize) -> bool {
        let mut id = 0;
        // SAFETY: `helper` is a thread start routine; `id` is a local.
        let thread = unsafe { CreateThread(core::ptr::null_mut(), 0, helper, mode, 0, &mut id) };
        if thread == 0 {
            return false;
        }
        // SAFETY: the helper's handle.
        let joined = unsafe { WaitForSingleObject(thread, 10_000) } == WAIT_OBJECT_0;
        // SAFETY: as above.
        unsafe { CloseHandle(thread) };
        joined
    }

    fn start_helper(mode: usize) -> Handle {
        let mut id = 0;
        // SAFETY: `helper` is a thread start routine; `id` is a local.
        unsafe { CreateThread(core::ptr::null_mut(), 0, helper, mode, 0, &mut id) }
    }

    fn join(thread: Handle) -> bool {
        // SAFETY: the helper's handle.
        let joined = thread != 0 && unsafe { WaitForSingleObject(thread, 10_000) } == WAIT_OBJECT_0;
        // SAFETY: as above.
        unsafe { CloseHandle(thread) };
        joined
    }

    fn sync() {
        let zero = 0u32;
        let flag = (&raw const SYNC.flag).cast::<c_void>();
        let compare = (&raw const zero).cast::<c_void>();
        // SAFETY: the flag and the comparand are 4-byte values that live for the call.
        let timed_out = unsafe { WaitOnAddress(flag, compare, 4, 10) } == 0;
        let timeout_error = last_error();
        let waiter = start_helper(SIGNAL_ADDRESS);
        while SYNC.flag.load(Ordering::Acquire) == 0 {
            // SAFETY: as above.
            unsafe { WaitOnAddress(flag, compare, 4, INFINITE) };
        }
        let address_ok = timed_out && join(waiter);
        // SAFETY: the condition variable and critical section are `SYNC`'s; the main thread
        // holds the section while it waits, as `SleepConditionVariableCS` requires.
        let condition_ok = unsafe {
            InitializeConditionVariable(SYNC.condition.get());
            InitializeCriticalSection(SYNC.section.get());
            EnterCriticalSection(SYNC.section.get());
            let signaler = start_helper(SIGNAL_CONDITION);
            while SYNC.ready.load(Ordering::Acquire) == 0 {
                SleepConditionVariableCS(SYNC.condition.get(), SYNC.section.get(), INFINITE);
            }
            LeaveCriticalSection(SYNC.section.get());
            join(signaler)
        };
        // SAFETY: the SRW lock is `SYNC`'s.
        unsafe { AcquireSRWLockShared(SYNC.lock.get()) };
        let tried = run_helper(TRY_SHARED_LOCK);
        // SAFETY: as above; the main thread holds it shared.
        unsafe { ReleaseSRWLockShared(SYNC.lock.get()) };
        // SAFETY: as above; nobody holds it now.
        let exclusive = unsafe { TryAcquireSRWLockExclusive(SYNC.lock.get()) } != 0;
        if exclusive {
            // SAFETY: as above; the main thread holds it exclusively.
            unsafe { ReleaseSRWLockExclusive(SYNC.lock.get()) };
        }
        let shared_ok = tried
            && exclusive
            && SYNC.shared_taken.load(Ordering::Acquire) == 1
            && SYNC.exclusive_refused.load(Ordering::Acquire) == 1;
        let mut once = 0usize;
        let mut pending = 0;
        let mut context = 0;
        // SAFETY: the one-time initialisation, flag and context are locals.
        let check_only =
            unsafe { InitOnceBeginInitialize(&mut once, 1, &mut pending, &mut context) };
        let mut code = 0;
        // SAFETY: the pseudo-handle of the calling thread; `code` is a local.
        unsafe { GetExitCodeThread(GetCurrentThread(), &mut code) };
        let verdict = |ok: bool| if ok { "ok" } else { "failed" };
        let mut line = Line::new();
        line.text("sync wait-on-address ").text(verdict(address_ok));
        line.text(" timeout ").number(u64::from(timeout_error));
        line.text(" condition ").text(verdict(condition_ok));
        line.text(" shared-srw ").text(verdict(shared_ok));
        line.text(" check-only ").number(check_only as u64);
        line.text(" current-thread ").number(u64::from(code));
        line.print();
    }

    extern "system" fn last_one(_: usize) -> u32 {
        // SAFETY: sleeps past the main thread's return.
        unsafe { Sleep(100) };
        Line::new()
            .text("detach the last thread ran after the first one returned")
            .print();
        5
    }

    fn detach() -> u32 {
        let mut id = 0;
        // SAFETY: `last_one` is a thread start routine; `id` is a local.
        let thread = unsafe { CreateThread(core::ptr::null_mut(), 0, last_one, 0, 0, &mut id) };
        // SAFETY: the thread's handle, which this thread no longer needs.
        unsafe { CloseHandle(thread) };
        Line::new()
            .text("detach the first thread returns 0")
            .print();
        0
    }

    fn checks() -> u32 {
        threads();
        sync();
        errors();
        text();
        system();
        0
    }
}

#[cfg(any(not(windows), test))]
fn main() {}
