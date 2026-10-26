# Running Windows programs

EggOS runs unmodified Windows console programs on x86_64, natively: the program's own machine
code runs directly on the CPU, with no emulator and no virtual machine (D17). The Windows layer
(D118), an optional component at `/usr/lib/eggos/windows`, loads the program's PE32+ file,
binds the functions it imports from `kernel32.dll` and `ntdll.dll` to its own implementations,
written in Rust on EggOS's system calls, and calls the program's entry point. A Win32 call is a
plain function call; no system call happens unless the function needs the kernel (a write to
the console, a file read), and the heap allocates without one.

This is part 1: console programs that use the functions listed below. Programs built with a C
or C++ runtime (MSVC's or MinGW's) or with Rust's standard library import more than this and
are refused with the name of the first missing function.

## Add a program to the image

```
cargo xtask image --arch x86_64 --add path/to/hello.exe
cargo xtask run --arch x86_64 --add path/to/hello.exe     # build that image and boot it
```

`--add` works as for [Linux programs](linux-programs.md): the file goes to `/bin/hello.exe`.
Adding a Windows program (a file that starts with `MZ`) also installs the Windows layer in an
x86_64 image; an image without one does not carry the layer at all (D18). At the prompt, type
`hello.exe` (or its path) and its arguments; the shell reports its exit code's low 8 bits in
`$?`, as for any program (`ExitProcess(300)` shows `44`).

On aarch64 and riscv64 images, and on x86_64 images without the layer, a Windows program is
refused with `Exec format error` and status 126.

## Build a program that runs

Rust, from our own source, with the official target and the toolchain's own linker (no Windows
SDK, no C runtime):

```
rustup target add x86_64-pc-windows-msvc
cargo build --release --target x86_64-pc-windows-msvc
```

with, in `.cargo/config.toml` (EggOS's own has it):

```
[target.x86_64-pc-windows-msvc]
linker = "rust-lld"
rustflags = ["-C", "link-arg=/NODEFAULTLIB", "-C", "link-arg=/SUBSYSTEM:CONSOLE", "-C", "link-arg=/ENTRY:mainCRTStartup"]
```

The program is `#![no_std]` and `#![no_main]`, defines `mainCRTStartup`, and declares its
imports with `#[link(name = "kernel32", kind = "raw-dylib")]` (and `ntdll` for `memcpy`,
`memset`, `memmove`, `memcmp`, `strlen` and `__chkstk`, which the compiler calls), so the
toolchain needs no import libraries. `tests/windows-console` is such a program; CI builds it and
runs it on x86_64, checking its arguments, environment, TEB and PEB, a file read from the
image, a missing file's error code, the heap and virtual memory, the clocks and `Sleep`, a
64 KiB stack frame, `LoadLibrary` and `GetProcAddress`, TLS slots, standard error through
`WriteConsoleW` and its exit codes; `windows-unsupported.exe` checks that a program importing a
function the layer lacks is refused with a clear message and status 126.

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
| Modules | `GetModuleHandleW`/`A`, `GetModuleFileNameW`, `LoadLibraryW`/`A`, `LoadLibraryExW`/`A`, `FreeLibrary`, `GetProcAddress` (kernel32 and ntdll; `api-ms-win-core-*` names resolve to kernel32) |

Paths may use `\`, a drive letter (`C:\etc\hostname` is `/etc/hostname`) or `\\?\`.

## What does not work yet

- **Programs on a C or C++ runtime or on Rust's std**: they import more (the CRT's own
  start-up, exception handling, threads, synchronisation, `ProcessPrng`), which later parts
  add. The layer names the first function it lacks.
- **Threads and waits** (`CreateThread`, `WaitForSingleObject`, critical sections, SRW locks),
  **directory listing** (`FindFirstFileW`), **other processes** (`CreateProcessW`) and
  **UTF-16 console polish** (line endings as `\r\n`, `ReadConsoleW` across multi-byte input):
  the next part (roadmap item 19).
- **Structured exception handling**: a fault ends the program as a signal would end a Linux
  one; no `__try`/`__except`, no vectored handlers, no unwinding.
- **DLLs**: only the layer's own kernel32 and ntdll; a program that imports another DLL
  (user32, ws2_32, its own) is refused. Imports by ordinal are refused.
- **GUI programs** (subsystem `WINDOWS`), 32-bit programs, ARM64 programs.
- **Writing files**: the file system is read-only, so `CreateFileW` for writing fails with
  `ERROR_ACCESS_DENIED`.
- **Exit codes** above 255 reach the shell as their low 8 bits.
- `VirtualProtect` reports the old protection as `PAGE_READWRITE`.
