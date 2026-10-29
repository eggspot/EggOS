# Running Windows programs

EggOS runs unmodified Windows console and GUI programs natively: x64 programs on x86_64 and
ARM64 programs on aarch64. The program's own machine code runs directly on the CPU, with no emulator
and no virtual machine (D17). The Windows layer
(D118), an optional component at `/usr/lib/eggos/windows`, loads the program's PE32+ file,
binds the functions it imports from `kernel32.dll`, `ntdll.dll`, `user32.dll` and `gdi32.dll`
to its own implementations, written in Rust on EggOS's system calls and window server, and calls
the program's entry point. A Win32 call is a
plain function call; no system call happens unless the function needs the kernel (a write to
the console, a file read), and the heap allocates without one.

Part 1 (D118) brought console programs; part 2a (D121) brings what small real tools use:
threads and waits, locks, error codes as Windows reports them, UTF-16 strings and directory
listing; part 2b (D124) brings all of it to ARM64 programs on aarch64; GUI programs, part 1
(D129), bring windows: a GUI program's windows open on the desktop, it paints them with GDI,
gets the mouse and keyboard as Win32 messages and shows message boxes. Programs on MSVC's C
runtime (D145) run too: C programs built with `/MD` import `vcruntime140.dll` and the
Universal C Runtime (`ucrtbase.dll`, through the `api-ms-win-crt-*` API sets), which the layer
provides as well. Programs that need C++ exceptions, MinGW's `msvcrt.dll` or Rust's standard
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

## Install a program as an app

On an image with the layer, `app install /media/host/hello.exe` installs a program from the
host share (or anywhere) into `/Applications/hello/`, with a launcher row and a desktop icon;
a double-click opens it (a GUI program's window appears, a console program runs in the
terminal) and `app uninstall hello` removes it. `--window` shows the installer's dialogs.
See [Apps](apps.md) (D134).

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

A GUI program links with `/SUBSYSTEM:WINDOWS` and its entry `WinMainCRTStartup` (the layer
calls the entry the PE names, with no arguments, as Windows does).

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
functions. `win-paint.exe` (`tests/windows-gui`) is a GUI program: it registers a class,
creates a window, paints a scene of GDI shapes and text, recolours its ellipse on a click
through `GetDC`, adds a typed letter, opens a message box on Space, has a second thread open one on F2 and ends when its
window is closed; the Windows GUI boot test drives it through QEMU's input and compares every frame with
the host's render of the same scene (D129).

## Programs on MSVC's C runtime

A C program built by Microsoft's compiler with the C runtime's DLLs (`/MD`, the default) runs
as it is, console or GUI, x64 on x86_64 and ARM64 on aarch64:

```
cl /O2 /MD hello.c                                         :: a console program
cl /O2 /MD pad.c /link /SUBSYSTEM:WINDOWS user32.lib gdi32.lib
```

`tests/windows-crt/eggpad.c` is our own example: a small notepad with `wWinMain`, a window
painted with `TextOutW`, typing through `WM_CHAR`, and opening and saving its file with
`_wfopen`, `fread`, `fwrite` and `ftell`; `cargo xtask windows-programs` builds it on a Windows
host, and the boot tests run it (docs/build.md). MSVC links the runtime's startup into the
program; the layer gives it its arguments (`__argc`, `__argv`, `__wargv`, the line `WinMain`
gets), its environment, `atexit`, `exit` and the rest. `printf`'s family is the same engine as
the macOS layer's (`eggos-c-format`), with Windows' rules: `%ld` is 32-bit, `%I64d`, `%zu`,
`%ls` and `%S` for wide strings, and in the wide functions `%s` is wide. Streams are in text
mode unless opened with `b`: a written `\n` becomes `\r\n` in the file, and reading turns it
back.

## GUI programs

Run one from `eggsh` like any program; the shell waits for it to end. Its windows open on the
desktop with EggOS's chrome (title bar, controls, the user's moving, resizing, maximising,
minimising and closing, D128) around the content the program paints. A GUI program has no
console: its standard handles are null where they would be the terminal, as on Windows, and
the files or pipes the shell redirected them to otherwise (`win-paint.exe > /tmp/log`).

- **Sizes.** `CreateWindowExW` takes the outside size; the content is that less EggOS's 1 px
  border and 28 px title bar (2 by 30). `GetSystemMetrics` reports that chrome
  (`SM_CYCAPTION` 28, borders and frames 1), so `AdjustWindowRect(Ex)` gives the same answer.
  `CW_USEDEFAULT` places the window by the desktop's cascade and makes it three quarters of
  the work area. Without `WS_THICKFRAME` the window cannot be resized or maximised.
- **Painting.** A window appears once its first `WM_PAINT` has ended, painted; until then and
  after a resize the desktop shows `window.bg`. `BeginPaint` sends `WM_ERASEBKGND`, which
  `DefWindowProcW` answers with the class brush; `EndPaint` shows the region it painted,
  `ReleaseDC` what a `GetDC` context drew. GDI follows Win32's pixel rules (a shape's right and
  bottom edges are outside it, a null pen makes rectangles and ellipses one pixel smaller,
  `LineTo` leaves out its end point); a wide pen's outline is inside the shape. Every font is
  Egg Mono 8x16 (`GetTextExtentPoint32W` gives 8 by 16 a character). The system colours are
  Windows' light defaults (`docs/ux/windows-gui-v0.3.md` section 2).
- **Messages.** Each thread reads its own windows' messages, those posted to it and its own
  `WM_QUIT`: posted messages, then the mouse and keyboard, then `WM_QUIT`, then `WM_PAINT`. The close control, Alt+F4 and the window menu's `Close` send
  `WM_SYSCOMMAND` with `SC_CLOSE`, a resize `WM_SIZE`, focus `WM_SETFOCUS` and `WM_KILLFOCUS`;
  keys have Windows' virtual-key codes, and `TranslateMessage` posts `WM_CHAR` with the
  character of the US layout.
- **Message boxes.** `MessageBoxW` shows the box of `docs/ux/windows-gui-v0.3.md` section 1, a
  dialog of its owner's window, with every `MB_` button set, the four icons, the default
  button, Enter, Esc, Tab, the arrows and the underlined mnemonics; it returns the button's
  `ID` value.

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
| Modules | `GetModuleHandleW`/`A`, `GetModuleFileNameW` (the program's full path in Windows' form), `LoadLibraryW`/`A`, `LoadLibraryExW`/`A`, `FreeLibrary`, `GetProcAddress` (kernel32, ntdll, shell32, user32, gdi32; `api-ms-win-core-*` names resolve to kernel32) |
| Threads | `CreateThread` (with `CREATE_SUSPENDED`), `ExitThread`, `ResumeThread`, `GetExitCodeThread`, `GetThreadId`, `GetCurrentThreadId`, `SwitchToThread`, `SetThreadPriority`, `GetThreadPriority`, `SetThreadDescription`; each thread has its own TEB, last error, TLS slots and copy of the program's static TLS (callbacks get `DLL_THREAD_ATTACH` and `DETACH`), and the process ends when its last thread does (returning from the entry point ends only the first thread) |
| Waits and objects | `WaitForSingleObject`(`Ex`), `WaitForMultipleObjects`(`Ex`) (any or all, timeouts) on threads, events, mutexes and semaphores; `CreateEventW`/`A`/`ExW`, `SetEvent`, `ResetEvent`, `CreateMutexW`/`A`/`ExW`, `ReleaseMutex`, `CreateSemaphoreW`/`A`/`ExW`, `ReleaseSemaphore`, `DuplicateHandle`, `CloseHandle` |
| Locks | `InitializeCriticalSection`(`AndSpinCount`, `Ex`), `EnterCriticalSection`, `TryEnterCriticalSection`, `LeaveCriticalSection`, `DeleteCriticalSection`; SRW locks (`AcquireSRWLockExclusive`/`Shared`, `TryAcquire...`, `Release...`); condition variables (`SleepConditionVariableCS`/`SRW`, `WakeConditionVariable`, `WakeAllConditionVariable`); `InitOnceExecuteOnce`, `InitOnceBeginInitialize`, `InitOnceComplete`; `WaitOnAddress`, `WakeByAddressSingle`/`All`. Interlocked operations are the compiler's own atomic instructions (lock-prefixed on x64, exclusive or LSE on ARM64) and need nothing from the layer |
| Fiber-local storage | `FlsAlloc` (callbacks run when a thread ends), `FlsFree`, `FlsGetValue`, `FlsSetValue` |
| Directories | `FindFirstFileW`, `FindFirstFileExW`, `FindNextFileW`, `FindClose` (`*` and `?`, names compared ignoring ASCII case, `*.*` matches everything, `.` and `..` listed), `GetFileAttributesW`, `GetFileAttributesExW`, `GetFileInformationByHandle`, `CreateDirectoryW`, `DeleteFileW`, `RemoveDirectoryW`, `GetCurrentDirectoryW`, `SetCurrentDirectoryW`, `GetFullPathNameW` |
| Windows (user32) | `RegisterClassExW`, `RegisterClassW`, `UnregisterClassW`, `CreateWindowExW`, `DestroyWindow`, `IsWindow`, `ShowWindow`, `UpdateWindow`, `GetClientRect`, `AdjustWindowRect`(`Ex`), `GetWindowLongW`, `SetWindowLongW`, `GetWindowLongPtrW`, `SetWindowLongPtrW` (the window procedure, instance, parent, id, styles, user data and the extra bytes), `DefWindowProcW`, `GetSystemMetrics`, `LoadCursorW`, `LoadIconW`, `MessageBoxW` |
| Messages (user32) | `GetMessageW`, `PeekMessageW` (`PM_REMOVE`), `TranslateMessage`, `DispatchMessageW`, `PostMessageW`, `PostQuitMessage`, `SendMessageW`; `WM_NCCREATE`, `WM_CREATE`, `WM_SHOWWINDOW`, `WM_SIZE`, `WM_PAINT`, `WM_ERASEBKGND`, `WM_SETFOCUS`, `WM_KILLFOCUS`, `WM_SYSCOMMAND`, `WM_CLOSE`, `WM_DESTROY`, `WM_NCDESTROY`, `WM_QUIT`, `WM_MOUSEMOVE`, `WM_LBUTTONDOWN`/`UP`, `WM_RBUTTONDOWN`/`UP`, `WM_MBUTTONDOWN`/`UP`, `WM_MOUSEWHEEL`, `WM_KEYDOWN`/`UP`, `WM_SYSKEYDOWN`/`UP`, `WM_CHAR`, `WM_SYSCHAR`, `WM_HELP` |
| Painting (user32) | `BeginPaint`, `EndPaint`, `GetDC`, `ReleaseDC`, `InvalidateRect`, `FillRect`, `DrawTextW` (left, centre, right; top, centre, bottom on one line; `DT_WORDBREAK`, `DT_NOPREFIX`, `DT_EXPANDTABS`, `DT_NOCLIP`, `DT_CALCRECT`, `DT_END_ELLIPSIS`), `GetSysColor`, `GetSysColorBrush` |
| GDI (gdi32) | `CreateSolidBrush`, `CreatePen` (solid or `PS_NULL`), `GetStockObject`, `SelectObject`, `DeleteObject`, `SetTextColor`, `SetBkColor`, `SetBkMode`, `MoveToEx`, `LineTo`, `Rectangle`, `Ellipse`, `SetPixel`, `TextOutW`, `GetTextExtentPoint32W` |
| C runtime startup (kernel32) | `IsDebuggerPresent`, `IsProcessorFeaturePresent` (the baseline features), `InitializeSListHead`, `GetStartupInfoW`/`A`, `SetUnhandledExceptionFilter`, `UnhandledExceptionFilter`, `RtlCaptureContext`, `RtlLookupFunctionEntry`, `RtlVirtualUnwind` (no unwinding yet: no function entry is found) |
| C runtime (vcruntime140) | `memcpy`, `memmove`, `memset`, `memcmp`, `memchr`, `strchr`, `strrchr`, `strstr`, `wcschr`, `wcsrchr`, `wcsstr`, `__C_specific_handler`, `__current_exception`, `__current_exception_context`, `__std_type_info_destroy_list`, `_purecall` |
| C runtime (ucrtbase, `api-ms-win-crt-*`): startup and exit | `_initterm`, `_initterm_e`, `_configure_narrow_argv`, `_configure_wide_argv`, `__p___argc`, `__p___argv`, `__p___wargv`, `_initialize_narrow_environment`, `_initialize_wide_environment`, `_get_initial_narrow_environment`, `_get_initial_wide_environment`, `__p__environ`, `__p__wenviron`, `_get_narrow_winmain_command_line`, `_get_wide_winmain_command_line`, `_set_app_type`, `__setusermatherr`, `_configthreadlocale`, `_set_new_mode`, `_query_new_mode`, `__p__commode`, `__p__fmode`, `_set_fmode`, `_get_fmode`, `_seh_filter_exe`, `_register_thread_local_exe_atexit_callback`, `_crt_atexit`, `_crt_at_quick_exit`, `_initialize_onexit_table`, `_register_onexit_function`, `_execute_onexit_table`, `exit`, `quick_exit`, `_exit`, `_Exit`, `_cexit`, `_c_exit`, `abort`, `terminate`, `_invalid_parameter_noinfo`(`_noreturn`), `_errno`, `_get_errno`, `_set_errno`, `__doserrno` |
| C runtime: heap | `malloc`, `calloc`, `realloc`, `_recalloc`, `free`, `_msize` (the process heap) |
| C runtime: strings and characters | `strlen`, `strnlen`, `strcmp`, `strncmp`, `_stricmp`, `_strnicmp`, `strcpy`, `strncpy`, `strcat`, `strncat`, `_strdup`, `strcpy_s`, `strcat_s`, `wcslen`, `wcsnlen`, `wcscmp`, `wcsncmp`, `_wcsicmp`, `wcscpy`, `wcsncpy`, `wcscat`, `_wcsdup`, `wcscpy_s`, `wcscat_s`, `isalpha`, `isdigit`, `isalnum`, `isspace`, `isupper`, `islower`, `isxdigit`, `ispunct`, `iscntrl`, `isprint`, `tolower`, `toupper`, `towlower`, `towupper`, `iswspace`, `iswalpha`, `iswdigit`, `strtol`, `strtoul`, `strtoll`, `strtoull`, `atoi`, `atol`, `atoll`, `wcstol`, `wcstoul`, `_wtoi`, `_wtol` (the "C" locale) |
| C runtime: streams | `__acrt_iob_func` (`stdin`, `stdout`, `stderr`), `fopen`, `_wfopen`, `fopen_s`, `_wfopen_s`, `fclose`, `fread`, `fwrite`, `fgetc`, `getc`, `fputc`, `putc`, `ungetc`, `fgets`, `fputs`, `puts`, `fflush`, `fseek`, `_fseeki64`, `ftell`, `_ftelli64`, `rewind`, `fgetpos`, `fsetpos`, `feof`, `ferror`, `clearerr`, `_fileno`, `remove`, `_wremove`; `printf`'s family through `__stdio_common_vfprintf`(`_s`), `__stdio_common_vfwprintf`(`_s`), `__stdio_common_vsprintf`, `__stdio_common_vsprintf_s`, `__stdio_common_vsnprintf_s`, `__stdio_common_vswprintf`, `__stdio_common_vswprintf_s`, `__stdio_common_vsnwprintf_s` (`%e`, `%f` and `%g` too) |
| C runtime: utility and time | `rand`, `srand` (our own generator), `abs`, `labs`, `llabs`, `_abs64`, `qsort`, `bsearch`, `_time64` |
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

- **Programs on a C++ runtime, on MinGW's `msvcrt.dll` or on Rust's std**: they import more
  (C++ exception handling, `ProcessPrng`, more of the C library), which later parts add. The
  layer names the first function it lacks. Of MSVC's C runtime, `scanf`'s family, `getenv`,
  `setlocale`, `signal`, `setjmp`, the math library, wide streams, low-level I/O (`_open`),
  `rename`, `tmpfile` and `_set_invalid_parameter_handler` are missing (D145's "Not yet"); a
  function given an invalid parameter returns its documented error instead of ending the
  program, and one lock serialises all of stdio.
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
- **DLLs**: only the layer's own kernel32, ntdll, shell32 (`CommandLineToArgvW`), user32,
  gdi32, vcruntime140 and the Universal C Runtime (what the tables above list); a program that imports another DLL (comctl32, ws2_32, its
  own) or a function they lack is refused. Imports by ordinal are refused.
- **GUI programs beyond part 1** (D129): child windows and the system's controls (buttons,
  edits, list boxes), menus, dialogs from resources, timers, `GetTextMetricsW`, fonts other
  than Egg Mono and text
  beyond ASCII (drawn as `?`), double-click messages, screen coordinates in `MSG.pt` and
  `WM_MOUSEWHEEL` (client coordinates today), `SetCursor`, `GetWindowRect` and moving windows
  from the program, `SW_HIDE`, bitmaps and `BitBlt`, dashed pens, raster operations and
  clipping beyond one rectangle. Follow-ups: `SendMessageW` to another thread's window calls
  the procedure on the caller's thread; destroying a window that owns an open message box leaves
  the box waiting; `WM_CREATE` answered with -1 removes the window without `WM_DESTROY`;
  `DestroyWindow` from another thread runs the procedure on the caller's thread too; all
  threads share the 256 posted and 256 input entries; a thread waiting while another reads the
  server's records spins instead of sleeping; handing the `ppoll` wait between sleeping threads
  wakes the others once for nothing; a pen is never wider than the window's width and height
  together. 32-bit
  programs (x86 and 32-bit Arm).
- **Writing files**: the file system is read-only, so `CreateFileW` for writing fails with
  `ERROR_ACCESS_DENIED`.
- **Exit codes** above 255 reach the shell as their low 8 bits.
- `VirtualProtect` reports the old protection as `PAGE_READWRITE`.
