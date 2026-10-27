use core::ffi::c_void;
use core::fmt::Write;

const PREFIX: &str = "WINDOWS-CONSOLE";
const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
const STD_ERROR_HANDLE: u32 = -12i32 as u32;
const GENERIC_READ: u32 = 0x8000_0000;
const FILE_SHARE_READ: u32 = 1;
const OPEN_EXISTING: u32 = 3;
const INVALID_HANDLE_VALUE: isize = -1;
const FILE_BEGIN: u32 = 0;
const HEAP_ZERO_MEMORY: u32 = 8;
const MEM_COMMIT_RESERVE: u32 = 0x3000;
const MEM_RELEASE: u32 = 0x8000;
const PAGE_READWRITE: u32 = 4;
const ERROR_ENVVAR_NOT_FOUND: u32 = 203;
const ERROR_MOD_NOT_FOUND: u32 = 126;
const FILETIME_1970: u64 = 116_444_736_000_000_000;
const MAX_ARGUMENTS: usize = 16;

type Handle = isize;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, b: *const u8, n: u32, w: *mut u32, o: *mut c_void) -> i32;
    fn WriteConsoleW(h: Handle, b: *const u16, n: u32, w: *mut u32, r: *mut c_void) -> i32;
    fn ReadFile(h: Handle, b: *mut u8, n: u32, r: *mut u32, o: *mut c_void) -> i32;
    fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        security: *mut c_void,
        disposition: u32,
        flags: u32,
        template: Handle,
    ) -> Handle;
    fn CloseHandle(h: Handle) -> i32;
    fn GetFileSizeEx(h: Handle, size: *mut i64) -> i32;
    fn SetFilePointerEx(h: Handle, distance: i64, new: *mut i64, method: u32) -> i32;
    fn ExitProcess(code: u32) -> !;
    fn GetCommandLineW() -> *const u16;
    fn GetEnvironmentVariableW(name: *const u16, buffer: *mut u16, size: u32) -> u32;
    fn GetLastError() -> u32;
    fn GetProcessHeap() -> Handle;
    fn HeapAlloc(heap: Handle, flags: u32, size: usize) -> *mut u8;
    fn HeapReAlloc(heap: Handle, flags: u32, block: *mut u8, size: usize) -> *mut u8;
    fn HeapFree(heap: Handle, flags: u32, block: *mut u8) -> i32;
    fn HeapSize(heap: Handle, flags: u32, block: *const u8) -> usize;
    fn VirtualAlloc(address: *mut c_void, size: usize, kind: u32, protect: u32) -> *mut u8;
    fn VirtualFree(address: *mut c_void, size: usize, kind: u32) -> i32;
    fn QueryPerformanceCounter(counter: *mut i64) -> i32;
    fn QueryPerformanceFrequency(frequency: *mut i64) -> i32;
    fn GetTickCount64() -> u64;
    fn GetSystemTimeAsFileTime(time: *mut u64);
    fn Sleep(milliseconds: u32);
    fn GetModuleHandleW(name: *const u16) -> usize;
    fn LoadLibraryA(name: *const u8) -> usize;
    fn GetProcAddress(module: usize, name: *const u8) -> *const c_void;
    fn TlsAlloc() -> u32;
    fn TlsSetValue(index: u32, value: *mut c_void) -> i32;
    fn TlsGetValue(index: u32) -> *mut c_void;
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
    bytes: [u8; 256],
    length: usize,
}

impl Line {
    fn new() -> Self {
        let mut line = Self {
            bytes: [0; 256],
            length: 0,
        };
        let _ = write!(line, "{PREFIX} ");
        line
    }

    fn print(mut self) {
        let _ = self.write_str("\n");
        let mut written = 0;
        // SAFETY: `GetStdHandle` takes no pointers.
        let output = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
        // SAFETY: the bytes are this line's own, and `written` is a local.
        unsafe {
            WriteFile(
                output,
                self.bytes.as_ptr(),
                self.length as u32,
                &mut written,
                core::ptr::null_mut(),
            )
        };
    }
}

impl Write for Line {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        let taken = text.len().min(self.bytes.len() - self.length);
        self.bytes[self.length..self.length + taken].copy_from_slice(&text.as_bytes()[..taken]);
        self.length += taken;
        Ok(())
    }
}

macro_rules! say {
    ($($argument:tt)*) => {{
        let mut line = Line::new();
        let _ = write!(line, $($argument)*);
        line.print();
    }};
}

struct Wide<const N: usize> {
    units: [u16; N],
}

impl<const N: usize> Wide<N> {
    fn new(text: &str) -> Self {
        let mut units = [0; N];
        for (unit, value) in units.iter_mut().zip(text.encode_utf16()) {
            *unit = value;
        }
        Self { units }
    }
}

struct Arguments {
    bytes: [u8; 1024],
    ends: [usize; MAX_ARGUMENTS],
    count: usize,
}

impl Arguments {
    fn from_command_line() -> Self {
        let mut arguments = Self {
            bytes: [0; 1024],
            ends: [0; MAX_ARGUMENTS],
            count: 0,
        };
        // SAFETY: `GetCommandLineW` returns the process's NUL-terminated command line.
        let line = unsafe { GetCommandLineW() };
        let mut units = [0u16; 512];
        let mut length = 0;
        // SAFETY: the command line is NUL-terminated; the copy stops there or when full.
        while length < units.len() && unsafe { line.wrapping_add(length).read() } != 0 {
            // SAFETY: as above.
            units[length] = unsafe { line.wrapping_add(length).read() };
            length += 1;
        }
        let mut chars = char::decode_utf16(units[..length].iter().copied())
            .map(|c| c.unwrap_or('\u{fffd}'))
            .peekable();
        let mut at = 0;
        let mut started = false;
        let mut quoted = false;
        while let Some(c) = chars.next() {
            let mut push = |c: char, at: &mut usize| {
                let mut buffer = [0u8; 4];
                for byte in c.encode_utf8(&mut buffer).bytes() {
                    if *at < arguments.bytes.len() {
                        arguments.bytes[*at] = byte;
                        *at += 1;
                    }
                }
            };
            match c {
                '\\' => {
                    let mut count = 1;
                    while chars.peek() == Some(&'\\') {
                        chars.next();
                        count += 1;
                    }
                    let before_quote = chars.peek() == Some(&'"');
                    let backslashes = if before_quote { count / 2 } else { count };
                    (0..backslashes).for_each(|_| push('\\', &mut at));
                    if before_quote && count % 2 == 1 {
                        chars.next();
                        push('"', &mut at);
                    }
                    started = true;
                }
                '"' => {
                    quoted = !quoted;
                    started = true;
                }
                ' ' | '\t' if !quoted => {
                    if started && arguments.count < MAX_ARGUMENTS {
                        arguments.ends[arguments.count] = at;
                        arguments.count += 1;
                    }
                    started = false;
                }
                c => {
                    push(c, &mut at);
                    started = true;
                }
            }
        }
        if started && arguments.count < MAX_ARGUMENTS {
            arguments.ends[arguments.count] = at;
            arguments.count += 1;
        }
        arguments
    }

    fn get(&self, index: usize) -> Option<&str> {
        if index >= self.count {
            return None;
        }
        let start = if index == 0 { 0 } else { self.ends[index - 1] };
        core::str::from_utf8(&self.bytes[start..self.ends[index]]).ok()
    }
}

#[unsafe(no_mangle)]
extern "system" fn mainCRTStartup() -> u32 {
    let arguments = Arguments::from_command_line();
    if arguments.get(1) == Some("exit") {
        let code = arguments
            .get(2)
            .and_then(|code| code.parse().ok())
            .unwrap_or(1);
        // SAFETY: `ExitProcess` ends the process with the code.
        unsafe { ExitProcess(code) }
    }
    let program = arguments.get(0).unwrap_or_default();
    let name = program.rsplit(['/', '\\']).next().unwrap_or_default();
    let mut line = Line::new();
    let _ = write!(line, "{name} args ");
    for index in 1..arguments.count {
        let separator = if index > 1 { "|" } else { "" };
        let _ = write!(
            line,
            "{separator}{}",
            arguments.get(index).unwrap_or_default()
        );
    }
    line.print();
    environment();
    say!("teb {}", verdict(environment_blocks()));
    file();
    say!("heap {}", verdict(heap()));
    say!("time {}", verdict(time()));
    say!("stack {}", verdict(stack()));
    say!("library {}", verdict(library()));
    say!("tls {}", verdict(thread_local()));
    let message = Wide::<32>::new("WINDOWS-CONSOLE stderr\n");
    let mut written = 0;
    // SAFETY: `GetStdHandle` takes no pointers.
    let error = unsafe { GetStdHandle(STD_ERROR_HANDLE) };
    // SAFETY: the message is a local buffer of 23 UTF-16 units.
    unsafe {
        WriteConsoleW(
            error,
            message.units.as_ptr(),
            23,
            &mut written,
            core::ptr::null_mut(),
        )
    };
    0
}

fn verdict(result: Result<(), &'static str>) -> &'static str {
    match result {
        Ok(()) => "ok",
        Err(step) => step,
    }
}

fn environment() {
    let name = Wide::<16>::new("GREETING\0");
    let mut value = [0u16; 64];
    // SAFETY: the name is NUL-terminated and the buffer holds 64 units.
    let length = unsafe { GetEnvironmentVariableW(name.units.as_ptr(), value.as_mut_ptr(), 64) };
    let mut line = Line::new();
    let _ = write!(line, "env GREETING=");
    for c in char::decode_utf16(value[..length as usize].iter().copied()) {
        let _ = line.write_char(c.unwrap_or('?'));
    }
    line.print();
    let missing = Wide::<16>::new("NO_SUCH_NAME\0");
    // SAFETY: as above.
    let length = unsafe { GetEnvironmentVariableW(missing.units.as_ptr(), value.as_mut_ptr(), 64) };
    // SAFETY: reads the thread's last error.
    let error = unsafe { GetLastError() };
    if length != 0 || error != ERROR_ENVVAR_NOT_FOUND {
        say!("env missing variable gave {length} and error {error}");
    }
}

fn environment_blocks() -> Result<(), &'static str> {
    let (teb, peb): (usize, usize);
    // SAFETY: Windows keeps the TEB's own address at gs:[0x30] and the PEB's at gs:[0x60].
    unsafe {
        core::arch::asm!(
            "mov {teb}, gs:[0x30]",
            "mov {peb}, gs:[0x60]",
            teb = out(reg) teb,
            peb = out(reg) peb,
            options(nostack, readonly, preserves_flags),
        )
    };
    if teb == 0 || peb == 0 {
        return Err("no TEB or PEB");
    }
    // SAFETY: the TEB's `Self` field, in the TEB GS points at.
    let own = unsafe { (teb as *const usize).wrapping_add(6).read() };
    // SAFETY: the PEB's `ImageBaseAddress` field.
    let image_base = unsafe { ((peb + 0x10) as *const usize).read() };
    // SAFETY: a null name asks for the program's own module.
    let module = unsafe { GetModuleHandleW(core::ptr::null()) };
    if own != teb {
        return Err("TEB Self is not the TEB");
    }
    if image_base != module {
        return Err("PEB image base is not the module handle");
    }
    let main = mainCRTStartup as *const () as usize;
    if !(image_base..image_base + (16 << 20)).contains(&main) {
        return Err("the entry point is not in the image");
    }
    Ok(())
}

#[allow(clippy::multiple_unsafe_ops_per_block)]
fn file() {
    let name = Wide::<32>::new("C:\\etc\\hostname\0");
    // SAFETY: the name is NUL-terminated; no security attributes or template.
    let handle = unsafe {
        CreateFileW(
            name.units.as_ptr(),
            GENERIC_READ,
            FILE_SHARE_READ,
            core::ptr::null_mut(),
            OPEN_EXISTING,
            0,
            0,
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        // SAFETY: reads the thread's last error.
        say!("file open error {}", unsafe { GetLastError() });
        return;
    }
    let mut bytes = [0u8; 64];
    let mut read = 0;
    let mut size = 0;
    let mut position = -1;
    let mut first = [0u8; 1];
    let mut again = 0;
    // SAFETY: the buffers are locals of the sizes given; the handle is open.
    let ok = unsafe {
        ReadFile(
            handle,
            bytes.as_mut_ptr(),
            64,
            &mut read,
            core::ptr::null_mut(),
        ) != 0
            && GetFileSizeEx(handle, &mut size) != 0
            && SetFilePointerEx(handle, 0, &mut position, FILE_BEGIN) != 0
            && ReadFile(
                handle,
                first.as_mut_ptr(),
                1,
                &mut again,
                core::ptr::null_mut(),
            ) != 0
            && CloseHandle(handle) != 0
    };
    let text = core::str::from_utf8(&bytes[..read as usize]).unwrap_or("?");
    if !ok || size != i64::from(read) || position != 0 || again != 1 || first[0] != bytes[0] {
        say!("file failed: read {read} size {size} position {position}");
    } else {
        say!("file {}", text.trim_end());
    }
    let missing = Wide::<32>::new("C:\\no\\such\\file\0");
    // SAFETY: as above.
    let handle = unsafe {
        CreateFileW(
            missing.units.as_ptr(),
            GENERIC_READ,
            0,
            core::ptr::null_mut(),
            OPEN_EXISTING,
            0,
            0,
        )
    };
    // SAFETY: reads the thread's last error.
    let error = unsafe { GetLastError() };
    say!(
        "missing file {} error {error}",
        if handle == INVALID_HANDLE_VALUE {
            "refused"
        } else {
            "opened"
        }
    );
}

#[allow(clippy::multiple_unsafe_ops_per_block)]
fn heap() -> Result<(), &'static str> {
    // SAFETY: the heap calls get the process heap and blocks it returned.
    unsafe {
        let heap = GetProcessHeap();
        let mut blocks = [core::ptr::null_mut::<u8>(); 12];
        for (index, block) in blocks.iter_mut().enumerate() {
            let size = 1usize << (index * 2);
            *block = HeapAlloc(heap, HEAP_ZERO_MEMORY, size);
            if block.is_null() || !(*block as usize).is_multiple_of(16) {
                return Err("HeapAlloc failed or misaligned");
            }
            if (0..size).any(|at| block.add(at).read() != 0) {
                return Err("HEAP_ZERO_MEMORY left bytes");
            }
            memset(block.cast(), index as i32 + 1, size);
            if HeapSize(heap, 0, *block) != size {
                return Err("HeapSize is not the size asked for");
            }
        }
        for (index, block) in blocks.iter().enumerate() {
            let size = 1usize << (index * 2);
            if (0..size).any(|at| block.add(at).read() != index as u8 + 1) {
                return Err("a block was overwritten");
            }
        }
        let grown = HeapReAlloc(heap, HEAP_ZERO_MEMORY, blocks[3], 100_000);
        if grown.is_null()
            || grown.read() != 4
            || grown.add(63).read() != 4
            || grown.add(64).read() != 0
        {
            return Err("HeapReAlloc lost the contents");
        }
        blocks[3] = grown;
        for block in blocks {
            if HeapFree(heap, 0, block) == 0 {
                return Err("HeapFree failed");
            }
        }
        if HeapFree(heap, 0, blocks[2]) != 0 {
            return Err("HeapFree took a freed block");
        }
        let pages = VirtualAlloc(
            core::ptr::null_mut(),
            3 * 4096 + 1,
            MEM_COMMIT_RESERVE,
            PAGE_READWRITE,
        );
        if pages.is_null() {
            return Err("VirtualAlloc failed");
        }
        memset(pages.cast(), 0x5a, 4 * 4096);
        let mut copy = [0u8; 16];
        memcpy(
            copy.as_mut_ptr().cast(),
            pages.add(4 * 4096 - 16).cast(),
            16,
        );
        memmove(pages.add(1).cast(), pages.cast(), 64);
        if memcmp(copy.as_ptr().cast(), pages.cast(), 16) != 0 || copy[15] != 0x5a {
            return Err("virtual memory lost its contents");
        }
        if VirtualFree(pages.cast(), 0, MEM_RELEASE) == 0 {
            return Err("VirtualFree failed");
        }
    }
    Ok(())
}

#[allow(clippy::multiple_unsafe_ops_per_block)]
fn time() -> Result<(), &'static str> {
    let (mut first, mut second, mut frequency, mut system) = (0, 0, 0, 0);
    // SAFETY: each call writes one local.
    unsafe {
        QueryPerformanceFrequency(&mut frequency);
        QueryPerformanceCounter(&mut first);
        let ticks = GetTickCount64();
        Sleep(20);
        QueryPerformanceCounter(&mut second);
        if GetTickCount64() < ticks + 20 {
            return Err("Sleep(20) took less than 20 ms of GetTickCount64");
        }
        GetSystemTimeAsFileTime(&mut system);
    }
    if frequency != 10_000_000 {
        return Err("QueryPerformanceFrequency is not 10 MHz");
    }
    if second - first < frequency / 50 {
        return Err("QueryPerformanceCounter did not advance by the sleep");
    }
    if system < FILETIME_1970 {
        return Err("GetSystemTimeAsFileTime is before 1970");
    }
    Ok(())
}

#[inline(never)]
fn stack() -> Result<(), &'static str> {
    let mut frame = [0u8; 64 << 10];
    let frame = core::hint::black_box(&mut frame);
    for (index, byte) in frame.iter_mut().enumerate().step_by(4096) {
        *byte = index as u8 | 1;
    }
    let sum: u32 = core::hint::black_box(&*frame)
        .iter()
        .map(|byte| u32::from(*byte))
        .sum();
    let _ = __chkstk as *const ();
    match sum {
        0 => Err("the large frame was not written"),
        _ => Ok(()),
    }
}

#[allow(clippy::multiple_unsafe_ops_per_block)]
fn library() -> Result<(), &'static str> {
    // SAFETY: the names are NUL-terminated; the function found is called with its signature.
    unsafe {
        let kernel32 = LoadLibraryA(c"KERNEL32.DLL".as_ptr().cast());
        if kernel32 == 0 {
            return Err("LoadLibraryA(kernel32) failed");
        }
        let function = GetProcAddress(kernel32, c"GetTickCount64".as_ptr().cast());
        if function.is_null() {
            return Err("GetProcAddress(GetTickCount64) failed");
        }
        let tick_count: extern "system" fn() -> u64 = core::mem::transmute(function);
        if tick_count() == 0 && GetTickCount64() == 0 {
            return Err("GetTickCount64 through GetProcAddress gave 0");
        }
        if LoadLibraryA(c"gdi32.dll".as_ptr().cast()) != 0 || GetLastError() != ERROR_MOD_NOT_FOUND
        {
            return Err("LoadLibraryA(gdi32) did not fail with ERROR_MOD_NOT_FOUND");
        }
    }
    Ok(())
}

#[allow(clippy::multiple_unsafe_ops_per_block)]
fn thread_local() -> Result<(), &'static str> {
    // SAFETY: the slot is the one `TlsAlloc` returned.
    unsafe {
        let index = TlsAlloc();
        if index == u32::MAX {
            return Err("TlsAlloc failed");
        }
        if TlsSetValue(index, 0x5eed as *mut c_void) == 0 || TlsGetValue(index) as usize != 0x5eed {
            return Err("TLS slot lost its value");
        }
    }
    Ok(())
}
