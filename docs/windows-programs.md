# Running Windows programs

EggOS runs unmodified Windows console programs natively: x64 programs on x86_64 and ARM64
programs on aarch64. The program's own machine code runs directly on the CPU, with no emulator
and no virtual machine (D17). The Windows layer
(D118), an optional component at `/usr/lib/eggos/windows`, loads the program's PE32+ file,
binds the functions it imports from `kernel32.dll` and `ntdll.dll` to its own implementations,
written in Rust on EggOS's system calls, and calls the program's entry point. A Win32 call is a
plain function call; no system call happens unless the function needs the kernel (a write to
the console, a file read), and the heap allocates without one.

Part 1 (D118) brought console programs; part 2a (D121) brings what small real tools use:
threads and waits, locks, error codes as Windows reports them, UTF-16 strings and directory
listing; part 2b (D124) brings all of it to ARM64 programs on aarch64. Programs built with a C
or C++ runtime (MSVC's or MinGW's) or with Rust's standard
library import more than this and are refused with the name of the first missing function.

## Add a program to the image

```
cargo xtask image --arch x86_64 --add path/to/hello.exe
cargo xtask run --arch x86_64 --add path/to/hello.exe     # build that image and boot it
cargo xtask run --arch aarch64 --add path/to/hello-arm64.exe
```

`--add` works as for [Linux programs](linux-programs.md): the file goes to `/bin/hello.exe`.
Adding a Windows program (a file that starts with `MZ`) also installs the Windows layer in an
x86_64 or aarch64 image; an image without one does not carry the layer at all (D18). At the prompt, type
`hello.exe` (or its path) and its arguments; the shell reports its exit code's low 8 bits in
`$?`, as for any program (`ExitProcess(300)` shows `44`).

| Program | x86_64 | aarch64 | riscv64 |
|---------|--------|---------|---------|
| x64 (machine `0x8664`) | runs | refused by the layer, status 126 | `Exec format error`, 126 |
| ARM64 (machine `0xAA64`) | refused by the layer, status 126 | runs | `Exec format error`, 126 |

The layer names the reason for the other CPU's program (`windows: /bin/x.exe: an x64 program,
and this computer runs ARM64 programs (EggOS does not translate x64 programs for Arm yet)`).
Without the layer, and on riscv64, a Windows program is refused with `Exec format error` and
status 126.

## Build a program that runs

Rust, from our own source, with the official target and the toolchain's own linker (no Windows
SDK, no C runtime):

```
rustup target add x86_64-pc-windows-msvc aarch64-pc-windows-msvc
RUSTFLAGS="-C linker=rust-lld -C link-arg=/NODEFAULTLIB -C link-arg=/SUBSYSTEM:CONSOLE -C link-arg=/ENTRY:mainCRTStartup" \
    cargo build --release --target x86_64-pc-windows-msvc    # or aarch64-pc-windows-msvc
```

(`cargo xtask` passes the same flags when it builds EggOS's test programs; they are not in
`.cargo/config.toml`, where they would also apply to programs built for a Windows host.)

The program is `#![no_std]` and `#![no_main]`, defines `mainCRTStartup`, and declares its
imports with `#[link(name = "kernel32", kind = "raw-dylib")]` (and `ntdll` for `memcpy`,
`memset`, `memmove`, `memcmp`, `strlen` and `__chkstk`, which the compiler calls), so the
toolchain needs no import libraries. `tests/windows-console` is such a program; CI builds it for
both targets and runs each on its own CPU (x64 on x86_64, ARM64 on aarch64), checking its arguments, environment, TEB and PEB, a file read from the
image, a missing file's error code, the heap and virtual memory, the clocks and `Sleep`, a
64 KiB stack frame, `LoadLibrary` and `GetProcAddress`, TLS slots, standard error through
`WriteConsoleW` and its exit codes; `other-cpu.exe`, the same program built for the other CPU,
checks that it is refused with the reason and status 126; `windows-unsupported.exe` checks that a program importing a
function the layer lacks is refused with a clear message and status 126. `windows-tools.exe`
is a small tool: `dir` lists wildcard patterns (`C:\bin\windows-*.exe`, a non-ASCII file
name), `cat` prints files (UTF-8 converted with `MultiByteToWideChar`, written with
`WriteConsoleW`, missing ones reported with `FormatMessageW`'s text and status 1), and `checks`
runs four worker threads joined with `WaitForMultipleObjects` over a shared counter, a critical
section, a mutex and an SRW lock, plus events, a semaphore, one-time initialisation, TLS and
FLS slots and the last error per thread, a suspended thread and `ExitThread`, the error codes,
UTF-8 round trips with non-ASCII text, the system, version, clock, path and environment
functions.

## What the layer provides

| Area | Functions (kernel32 unless noted) |
|------|-----------------------------------|
| Console | `GetStdHandle`, `WriteFile`, `WriteConsoleW`/`A`, `ReadFile`, `ReadConsoleW`/`A`, `GetConsoleMode`, `SetConsoleMode`, `GetConsoleCP`, `GetConsoleOutputCP`, `SetConsoleCP`, `SetConsoleOutputCP`, `GetACP`, `GetOEMCP` (all UTF-8, 65001) |
| Files | `CreateFileW`/`A`, `ReadFile`, `WriteFile`, `CloseHandle`, `GetFileSize`, `GetFileSizeEx`, `SetFilePointer`, `SetFilePointerEx`, `GetFileType`, `FlushFileBuffers` |
| Memory | `GetProcessHeap`, `HeapAlloc`, `HeapFree`, `HeapReAlloc`, `HeapSize`, `HeapCreate`, `HeapDestroy`, `VirtualAlloc`, `VirtualFree`, `VirtualProtect`; ntdll's `memcpy`, `memmove`, `memset`, `memcmp`, `strlen`, `__chkstk` |
| Process | `GetCommandLineW`/`A`, `ExitProcess`, `TerminateProcess` (itself), `GetCurrentProcess`, `GetCurrentProcessId`, `GetCurrentThread`, `GetCurrentThreadId` |
| Environment | `GetEnvironmentVariableW`/`A`, `GetEnvironmentStrings`, `GetEnvironmentStringsW`/`A`, `FreeEnvironmentStringsW`/`A` |
| Time | `GetTickCount`, `GetTickCount64`, `QueryPerformanceCounter`, `QueryPerformanceFrequency` (10 MHz), `GetSystemTimeAsFileTime`, `GetSystemTimePreciseAsFileTime`, `Sleep`, `SleepEx` |
| Errors and TLS | `GetLastError`, `SetLastError`, `TlsAlloc`, `TlsFree`, `TlsGetValue`, `TlsSetValue`, and a program's static TLS (its TLS directory and callbacks) |
| Modules | `GetModuleHandleW`/`A`, `GetModuleFileNameW` (the program's full path in Windows' form), `LoadLibraryW`/`A`, `LoadLibraryExW`/`A`, `FreeLibrary`, `GetProcAddress` (kernel32, ntdll, shell32, user32; `api-ms-win-core-*` names resolve to kernel32) |
| Threads | `CreateThread` (with `CREATE_SUSPENDED`), `ExitThread`, `ResumeThread`, `GetExitCodeThread`, `GetThreadId`, `GetCurrentThreadId`, `SwitchToThread`, `SetThreadPriority`, `GetThreadPriority`, `SetThreadDescription`; each thread has its own TEB, last error, TLS slots and copy of the program's static TLS (callbacks get `DLL_THREAD_ATTACH` and `DETACH`), and the process ends when its last thread does (returning from the entry point ends only the first thread) |
| Waits and objects | `WaitForSingleObject`(`Ex`), `WaitForMultipleObjects`(`Ex`) (any or all, timeouts) on threads, events, mutexes and semaphores; `CreateEventW`/`A`/`ExW`, `SetEvent`, `ResetEvent`, `CreateMutexW`/`A`/`ExW`, `ReleaseMutex`, `CreateSemaphoreW`/`A`/`ExW`, `ReleaseSemaphore`, `DuplicateHandle`, `CloseHandle` |
| Locks | `InitializeCriticalSection`(`AndSpinCount`, `Ex`), `EnterCriticalSection`, `TryEnterCriticalSection`, `LeaveCriticalSection`, `DeleteCriticalSection`; SRW locks (`AcquireSRWLockExclusive`/`Shared`, `TryAcquire...`, `Release...`); condition variables (`SleepConditionVariableCS`/`SRW`, `WakeConditionVariable`, `WakeAllConditionVariable`); `InitOnceExecuteOnce`, `InitOnceBeginInitialize`, `InitOnceComplete`; `WaitOnAddress`, `WakeByAddressSingle`/`All`. Interlocked operations are the compiler's own atomic instructions (lock-prefixed on x64, exclusive or LSE on ARM64) and need nothing from the layer |
| Fiber-local storage | `FlsAlloc` (callbacks run when a thread ends), `FlsFree`, `FlsGetValue`, `FlsSetValue` |
| Directories | `FindFirstFileW`, `FindFirstFileExW`, `FindNextFileW`, `FindClose` (`*` and `?`, names compared ignoring ASCII case, `*.*` matches everything, `.` and `..` listed), `GetFileAttributesW`, `GetFileAttributesExW`, `GetFileInformationByHandle`, `CreateDirectoryW`, `DeleteFileW`, `RemoveDirectoryW`, `GetCurrentDirectoryW`, `SetCurrentDirectoryW`, `GetFullPathNameW` |
| Strings | `MultiByteToWideChar`, `WideCharToMultiByte` (`CP_UTF8`, and `CP_ACP`/`CP_OEMCP`, which are UTF-8 too; `MB_ERR_INVALID_CHARS`, `WC_ERR_INVALID_CHARS`), `lstrlenW`/`A`, `lstrcmpW`, `lstrcmpiW`, `CompareStringOrdinal`, `CompareStringW`/`Ex` (ordinal, `NORM_IGNORECASE`), `FormatMessageW` (system messages, `FORMAT_MESSAGE_ALLOCATE_BUFFER`), user32's `CharUpperW` and `CharLowerW`, shell32's `CommandLineToArgvW` |
| Environment, more | `SetEnvironmentVariableW`/`A`; `GetEnvironmentStrings*` return a copy that reflects changes |
| System and time, more | `GetSystemInfo`, `GetNativeSystemInfo` (`PROCESSOR_ARCHITECTURE_AMD64`, 9, on x86_64; `PROCESSOR_ARCHITECTURE_ARM64`, 12, on aarch64), `GetVersionExW`, `GetVersion`, ntdll's `RtlGetVersion` (Windows 11: 10.0, build 26100), `GetSystemTime`, `GetLocalTime` (UTC), `FileTimeToSystemTime`, `SystemTimeToFileTime`, `FileTimeToLocalFileTime`, `LocalFileTimeToFileTime`, `LocalAlloc`/`LocalFree`, `GlobalAlloc`/`GlobalFree` (fixed blocks) |

Paths may use `\`, a drive letter (`C:\etc\hostname` is `/etc/hostname`; every drive letter is
the root) or `\\?\`; paths the layer gives back (`GetCurrentDirectoryW`, `GetFullPathNameW`,
`GetModuleFileNameW`) are in Windows' form, with `C:\` for `/`. Names are case-sensitive, unlike
on Windows: `C:\ETC\HOSTNAME` is not `/etc/hostname` (wildcard matching ignores ASCII case).

On aarch64 the layer follows the ARM64 Windows ABI: the program's TEB is in `x18` (the layer's
own code is built for Linux, where `x18` is an ordinary register, so each Win32 function saves
and restores the program's `x18`, and the layer loads it from its own copy in `tpidr_el0`
before every call into the program), the
arguments follow the Arm64 procedure call standard, `__chkstk` takes the frame size in 16-byte
units in `x15`, and the TEB and PEB have the same layout as on x64.

Error codes are Windows': a missing file is `ERROR_FILE_NOT_FOUND` (2), and
`ERROR_PATH_NOT_FOUND` (3) when its directory is missing too; an existing directory given to
`CreateDirectoryW` is `ERROR_ALREADY_EXISTS` (183); the end of a listing is
`ERROR_NO_MORE_FILES` (18); a too-small buffer is `ERROR_INSUFFICIENT_BUFFER` (122) or the
size needed, as each function documents.

## What does not work yet

- **Programs on a C or C++ runtime or on Rust's std**: they import more (the CRT's own
  start-up, exception handling, threads, synchronisation, `ProcessPrng`), which later parts
  add. The layer names the first function it lacks.
- **Other processes** (`CreateProcessW`, pipes) and **UTF-16 console polish** (line endings as
  `\r\n`, `ReadConsoleW` across multi-byte input): a later part.
- **x64 programs on aarch64** (and ARM64 programs on x86_64): an x64 program on Arm needs its
  code translated ahead of time (D17), which EggOS does not do yet; ARM64EC and ARM64X hybrid
  images run only as plain ARM64 code (their x64 parts are not supported).
- **DOS wildcard special cases** (`a?` matching `a`, `*.` matching names without an
  extension); `lstrcmpW` and `lstrcmpiW` compare ordinally, not by locale;
  `DuplicateHandle(GetCurrentThread())` is refused.
- **Abandoned mutexes** (`WAIT_ABANDONED`), `SuspendThread`, `TerminateThread`, named objects,
  alertable waits and APCs, `FlsFree` and `TlsFree` clearing other threads' slots (only the
  caller's), the TLS expansion slots (64 TLS and 64 FLS slots per thread).
- **Creating and removing** files and directories: the file system is read-only, so
  `CreateDirectoryW`, `DeleteFileW` and `RemoveDirectoryW` on a path that exists (or could)
  fail with `ERROR_ACCESS_DENIED`.
- **Time zones**: local time is UTC.
- **Structured exception handling**: a fault ends the program as a signal would end a Linux
  one; no `__try`/`__except`, no vectored handlers, no unwinding.
- **DLLs**: only the layer's own kernel32, ntdll, shell32 (`CommandLineToArgvW`) and user32 (its
  character functions); a program that imports another DLL (gdi32, ws2_32, its own) or a
  function they lack is refused. Imports by ordinal are refused.
- **GUI programs** (subsystem `WINDOWS`), 32-bit programs (x86 and 32-bit Arm).
- **Writing files**: the file system is read-only, so `CreateFileW` for writing fails with
  `ERROR_ACCESS_DENIED`.
- **Exit codes** above 255 reach the shell as their low 8 bits.
- `VirtualProtect` reports the old protection as `PAGE_READWRITE`.
