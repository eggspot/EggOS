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
runtime (D146) run too: C programs built with `/MD` import `vcruntime140.dll` and the
Universal C Runtime (`ucrtbase.dll`, through the `api-ms-win-crt-*` API sets), which the layer
provides as well, with structured exception handling and C++ exceptions (D177). Programs on
the C++ standard library's DLL (`msvcp140.dll`) import more than this and are refused with
the name of the first missing function; programs on MinGW's `msvcrt.dll` and on Rust's
standard library run (D189, below).

## Add a program to the image

```
cargo xtask image --arch x86_64 --add path/to/hello.exe
cargo xtask run --arch x86_64 --add path/to/hello.exe     # build that image and boot it
cargo xtask run --arch aarch64 --add path/to/hello-arm64.exe
```

`--add` works as for [Linux programs](linux-programs.md): the file goes to `/bin/hello.exe`.
The x86_64 and aarch64 release images carry the Windows layer at `/usr/lib/eggos/windows`
(D152), with or without added files, so a program in the host folder (`--apps`) runs too
(`/media/host/hello.exe`). At the prompt, type
`hello.exe` (or its path) and its arguments; the shell reports its exit code's low 8 bits in
`$?`, as for any program (`ExitProcess(300)` shows `44`).

| Program | x86_64 | aarch64 | riscv64 |
|---------|--------|---------|---------|
| x64 (machine `0x8664`) | runs | refused by the layer, status 126 | `Exec format error`, 126 |
| ARM64 (machine `0xAA64`) | refused by the layer, status 126 | runs | `Exec format error`, 126 |

The layer names the reason for the other CPU's program (`windows: /bin/x.exe: an x64 program,
and this computer runs ARM64 programs (EggOS does not translate x64 programs for Arm yet)`).
On riscv64, whose image carries no Windows layer, a Windows program is refused with `Exec
format error` and status 126.

## Install a program as an app

On x86_64 and aarch64, `app install /media/host/hello.exe` installs a program from the
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

## Exceptions: `__try`, C++ and faults

Programs that handle exceptions run as on Windows (D177), x64 on x86_64 and ARM64 on aarch64:

```
cl /O2 /MD /EHsc app.cpp                                   :: C++ with try, throw and catch
cl /O2 /MD seh.c                                           :: C with __try and __except
```

- **C++ exceptions.** `throw` and `catch` work as the compiler arranged them, with
  `__CxxFrameHandler4` (x64's default) or `__CxxFrameHandler3` (`/d2FH4-`, and older
  compilers): catching by type, through a base class, by value (the copy constructor runs) or
  by reference, `catch (...)`, `throw;`, a throw from inside a catch block, the destructors of
  every frame left, `noexcept` functions ending the program, and `std::exception` and its
  standard subclasses from the headers. An exception nothing catches ends the program
  through `terminate`, with status 3.
- **Structured exceptions.** `__try`/`__except` with filters (`GetExceptionCode()`,
  `GetExceptionInformation()`, `EXCEPTION_CONTINUE_EXECUTION`), `__try`/`__finally`
  (`AbnormalTermination()`), `__leave`, `RaiseException`, vectored exception and continue
  handlers, `SetUnhandledExceptionFilter`, and `RtlCaptureContext`, `RtlLookupFunctionEntry`,
  `RtlVirtualUnwind` and `RtlUnwindEx` for programs that walk their own stack.
- **Faults are exceptions.** A fault in the program's code reaches its handlers as the
  exception Windows raises: an access violation (`0xC0000005`, with read, write or execute and
  the address), an integer division by zero (`0xC0000094`; on ARM64 MSVC's own check raises
  it), other arithmetic errors, an illegal instruction (`0xC000001D`), a breakpoint
  (`__debugbreak()`) and a misalignment. `__fastfail` ends the program at once with
  `0xC0000409`. A fault nothing handles ends the program as before, after a line on standard
  error naming it and its address as the program's module and offset (D151); an exception the
  program raised and nothing handled ends it with the exception's code as its status, after a
  line naming the code.
- **C signals.** `signal` and `raise`, and MSVC's startup giving an unhandled access
  violation, illegal instruction or arithmetic error to the `SIGSEGV`, `SIGILL` or `SIGFPE`
  handler a program set.

`tests/windows-crt/eggcpp.cpp` is our own example, which the Windows C runtime boot test runs
in CI (`cargo xtask windows-crt`).

## Programs built on Rust's standard library

A Windows program written in Rust and built for `x86_64-pc-windows-msvc` or
`aarch64-pc-windows-msvc` runs as it is (D189): printing, `std::process::Command` (`output`,
`spawn` with piped standard streams, `wait`, exit codes), files (`fs::write`, `read_to_string`,
`set_len`, `metadata`, `remove_file`), threads with `Mutex`, `Condvar` and thread-locals,
`thread::sleep`, `Instant` and `SystemTime`, `HashMap` (seeded by `ProcessPrng`), and panics,
printed on standard error with exit code 101. The layer implements what std imports for
them: ntdll's `NtReadFile`, `NtWriteFile`, `NtOpenFile` and `NtCreateNamedPipeFile` (std's
anonymous pipes), overlapped reads on pipes, `ReadFileEx`, `WriteFileEx` and alertable waits,
`GetFileInformationByHandleEx`, `SetFileInformationByHandle`, `ProcessPrng` and
`GetUserProfileDirectoryW`; `NUL` is `/dev/null`. Not yet: `fs::copy`, `fs::canonicalize`,
links, `File::lock`, `fs::remove_dir_all`, networking and the process attribute-list API
(such a program does not start, and the message names the missing function).

## Programs built with MinGW

A C program built by MinGW-w64 (GCC or llvm-mingw) against its default C runtime, the old
`msvcrt.dll`, runs as it is on x86_64 and aarch64 (D189): MinGW's startup (`__getmainargs`,
`atexit`, `_fmode`, the TLS callbacks and pseudo-relocations, which use `VirtualQuery`),
MinGW's own `printf` (which locks the streams through `_lock` and writes them with `fputc`)
and the old runtime's `printf` family (`printf`, `fprintf`, `sprintf`, `_snprintf`,
`_scprintf`, `wprintf` and their `v` forms, with `%I64d`), `stdout` and `stderr` through
`__iob_func`, `setvbuf`, the heap, strings, `qsort`, `getenv`, `signal` and `raise`,
`strerror`, `localeconv`, `_setmode` and `exit`. The layer gives `msvcrt.dll` the same
functions as the Universal C Runtime where the two agree; a program built with `_dowildcard`
gets its wildcard arguments expanded (see below). Not yet: MinGW DLLs (`__dllonexit`),
`_putenv`, `setlocale`, the old runtime's `swprintf` and line-ending translation in text mode.

`tests/windows-mingw/eggmingw.c` is our own example: CI builds it with llvm-mingw for x64 and
ARM64, and the Windows C runtime boot test runs it (`cargo xtask windows-crt`).

## APCs, abandoned mutexes, ordinals and wildcards

`QueueUserAPC` queues a call for a thread, which runs it in its next alertable wait:
`SleepEx`, `WaitForSingleObjectEx`, `WaitForMultipleObjectsEx` and
`MsgWaitForMultipleObjectsEx` with `MWMO_ALERTABLE` run every queued call and return
`WAIT_IO_COMPLETION`, and a call queued while the thread already waits wakes it. A thread that
ends while it owns a mutex abandons it: the next wait takes it with `WAIT_ABANDONED` (D189).
`MsgWaitForMultipleObjects` also wakes for a message in the thread's queue. Programs linked
with the SDK's `ws2_32.lib` import Winsock by ordinal from `ws2_32.dll`, whose numbers differ
from `wsock32.dll`'s in three places; both work, and `GetProcAddress` with an ordinal agrees.
`FindFirstFileW` matches names with DOS's rules: `*.*` is everything, `*.` names without an
extension, `?` one character or none before a dot or the end, and `<`, `>` and `"` as Windows'
file systems match them. `tests/windows-crt/eggsync.c` is our own example, in the same boot
test.

## Named objects, file mappings and wildcard arguments

A mutex, event, semaphore or file mapping created with a name is shared with every Windows
program that opens the name (`OpenMutexW`, `OpenEventW`, `OpenSemaphoreW`,
`OpenFileMappingW`, and the `Create` functions, which then answer `ERROR_ALREADY_EXISTS`).
Names follow Windows' rules: `Global\` and `Local\` in either case, a bare name in `Local\`,
one namespace for every kind (another kind's name is `ERROR_INVALID_HANDLE`, a missing one
`ERROR_FILE_NOT_FOUND`), and a mutex whose owner ended comes back `WAIT_ABANDONED`. A wait for
one sees another program's change within 10 ms. `CreateFileMappingW` maps a file or, with
`INVALID_HANDLE_VALUE`, memory of its own; `MapViewOfFile(Ex)` gives views that read, write
(`FILE_MAP_WRITE` reaches the file and every other view), copy on write (`FILE_MAP_COPY`) or
run, `FlushViewOfFile` writes a view back and `UnmapViewOfFile` removes it.
`SignalObjectAndWait` signals one object and waits for another. A program built with
`setargv.obj` (MSVC) or `_dowildcard` (MinGW) gets each argument with `*` or `?` in its last
part replaced by the matching names, sorted, with the folder as written; a pattern that
matches nothing stays as it is. `tests/windows-crt/eggnamed.c` is our own example, in the same
boot test: it starts a copy of itself with `*.txt` and shares a mutex, an event, a semaphore
and a mapping with it (D189).

## Programs that start other programs

A Windows program can start another program, Windows or Linux, and read its output (D178,
D189):

- **`CreateProcessW` and `CreateProcessA`** find the program from a path or a name (the
  program's own folder, then the working folder, then `PATH`; `.exe` is added to a name
  without an extension), and give it its command line (split and quoted back exactly as the C
  runtime parses it), a working folder, and the caller's environment or the block it passes
  (ANSI, or UTF-16 with `CREATE_UNICODE_ENVIRONMENT`). `PROCESS_INFORMATION`'s process handle
  waits (`WaitForSingleObject`, `WaitForMultipleObjects`), gives the exit code
  (`GetExitCodeProcess`, `STILL_ACTIVE` while it runs) and ends the child
  (`TerminateProcess`); its thread handle waits for the same end.
- **Pipes.** `CreatePipe` makes an anonymous pipe. `ReadFile` at its end (all writers closed)
  fails with `ERROR_BROKEN_PIPE`, and `WriteFile` to a pipe nobody reads with
  `ERROR_NO_DATA`. `PeekNamedPipe` tells how many bytes wait and copies the first ones without
  taking them: `ReadFile` still gets them.
- **Inherited handles.** As on Windows, a child gets the handles marked inheritable when it
  is started with `bInheritHandles` TRUE, with the same values, and none but its standard
  ones otherwise: `SECURITY_ATTRIBUTES.bInheritHandle` (`CreatePipe`, `CreateFileW`/`A`),
  `SetHandleInformation` and `GetHandleInformation` (`HANDLE_FLAG_INHERIT`,
  `HANDLE_FLAG_PROTECT_FROM_CLOSE`) and `DuplicateHandle` set and read it. With
  `STARTF_USESTDHANDLES` the child's standard input, output and error are the handles
  `STARTUPINFO` names. Events, mutexes, semaphores and process handles are never inherited.
- **Creation flags.** `CREATE_SUSPENDED` starts the child at once; `ResumeThread` on its
  thread handle answers 1, then 0, as if it had waited. `CREATE_NEW_CONSOLE`,
  `CREATE_NO_WINDOW` and `DETACHED_PROCESS` change nothing: the child shares the terminal.
  `CREATE_NEW_PROCESS_GROUP` gives it a process group of its own.

`tests/windows-crt/eggspawn.c` is our own example: it starts a copy of itself with its output
through a pipe, then others to terminate and to resume; the Windows C runtime boot test runs
it in CI.

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
- **Controls** (D145, `docs/ux/windows-controls-v0.3.md`). `CreateWindowExW` with `WS_CHILD`
  makes a child window inside its parent's client area (it shares its top-level window's
  buffer; the program's own classes and the system's `BUTTON`, `EDIT`, `STATIC` and `LISTBOX`).
  Push buttons and default push buttons, check boxes (`BS_CHECKBOX`, `BS_AUTOCHECKBOX`),
  single-line edits (`ES_READONLY`; the caret only, no selection), labels (`SS_LEFT`,
  `SS_CENTER`, `SS_RIGHT`, word wrap, `SS_NOTIFY`) and single-selection lists (`LBS_SORT`,
  `LBS_NOTIFY`) are drawn in Windows' light colours and tell their parent through
  `WM_COMMAND` (`BN_CLICKED`, `EN_CHANGE`, `EN_UPDATE`, `EN_SETFOCUS`, `EN_KILLFOCUS`,
  `LBN_SELCHANGE`, `LBN_SETFOCUS`, `LBN_KILLFOCUS`, `STN_CLICKED`). A click focuses a control;
  the focused one gets the keys and keeps its caret while the window is inactive.
  `IsDialogMessageW` moves the focus with Tab and Shift+Tab, and sends Enter to the default
  button (or `IDOK`) and Esc as `IDCANCEL`.
- **Menus.** `CreateMenu`, `CreatePopupMenu`, `AppendMenuW` (strings, drop-downs, separators,
  `MF_GRAYED`, `MF_CHECKED`) and `SetMenu` (or `CreateWindowExW`'s menu) put a 24 px bar at the
  top of the window's content; the client area starts below it. A click opens a drop-down
  over the client area, a click on an item sends `WM_COMMAND` with its id (a bar item without
  a drop-down sends it too), a click elsewhere or Esc closes it; no key reaches the program
  while it is open.
- **Timers.** `SetTimer` and `KillTimer`, for a window or the thread, with or without a timer
  procedure; `WM_TIMER` comes after the paints, at most one waiting per timer, every 10 ms at
  the fastest and `0x7fffffff` ms at the slowest.
- **Opening and saving files.** comdlg32's `GetOpenFileNameW` and `GetSaveFileNameW` show
  EggOS's own picker (a dialog of the owner's window): the folder in Windows' form, its
  folders and the files the selected filter matches, the file name, and Open or Save; they
  return the chosen path in Windows' form with `nFileOffset`, `nFileExtension` and
  `lpstrFileTitle`, add `lpstrDefExt`, and honour `OFN_FILEMUSTEXIST` and
  `OFN_PATHMUSTEXIST`.

## What the layer provides

| Area | Functions (kernel32 unless noted) |
|------|-----------------------------------|
| Console | `GetStdHandle`, `WriteFile`, `WriteConsoleW`/`A`, `ReadFile`, `ReadConsoleW`/`A`, `GetConsoleMode`, `SetConsoleMode`, `GetConsoleCP`, `GetConsoleOutputCP`, `SetConsoleCP`, `SetConsoleOutputCP`, `GetACP`, `GetOEMCP` (all UTF-8, 65001) |
| Files | `CreateFileW`/`A`, `ReadFile`, `WriteFile`, `CloseHandle`, `GetFileSize`, `GetFileSizeEx`, `SetFilePointer`, `SetFilePointerEx`, `GetFileType`, `FlushFileBuffers` |
| Memory | `GetProcessHeap`, `HeapAlloc`, `HeapFree`, `HeapReAlloc`, `HeapSize`, `HeapCreate`, `HeapDestroy`, `VirtualAlloc`, `VirtualFree`, `VirtualProtect`; ntdll's `memcpy`, `memmove`, `memset`, `memcmp`, `strlen`, `__chkstk` |
| Process | `GetCommandLineW`/`A`, `ExitProcess`, `TerminateProcess` (itself), `GetCurrentProcess`, `GetCurrentProcessId`, `GetCurrentThread`, `GetCurrentThreadId` |
| Other processes and pipes (D178, D189) | `CreateProcessW`/`A`, `OpenProcess`, `GetProcessId`, `GetExitCodeProcess`, `TerminateProcess`, `CreatePipe`, `PeekNamedPipe`, `SetHandleInformation`, `GetHandleInformation`, `DuplicateHandle` (files, pipes, objects, the caller's process), `ResumeThread` on a child's thread handle |
| Environment | `GetEnvironmentVariableW`/`A`, `GetEnvironmentStrings`, `GetEnvironmentStringsW`/`A`, `FreeEnvironmentStringsW`/`A` |
| Time | `GetTickCount`, `GetTickCount64`, `QueryPerformanceCounter`, `QueryPerformanceFrequency` (10 MHz), `GetSystemTimeAsFileTime`, `GetSystemTimePreciseAsFileTime`, `Sleep`, `SleepEx` |
| Errors and TLS | `GetLastError`, `SetLastError`, `TlsAlloc`, `TlsFree`, `TlsGetValue`, `TlsSetValue`, and a program's static TLS (its TLS directory and callbacks) |
| Modules | `GetModuleHandleW`/`A`, `GetModuleFileNameW` (the program's full path in Windows' form), `LoadLibraryW`/`A`, `LoadLibraryExW`/`A`, `FreeLibrary`, `GetProcAddress` (kernel32, ntdll, shell32, user32, gdi32, comdlg32; `api-ms-win-core-*` names resolve to kernel32) |
| Threads | `CreateThread` (with `CREATE_SUSPENDED`), `ExitThread`, `ResumeThread`, `GetExitCodeThread`, `GetThreadId`, `GetCurrentThreadId`, `SwitchToThread`, `SetThreadPriority`, `GetThreadPriority`, `SetThreadDescription`; each thread has its own TEB, last error, TLS slots and copy of the program's static TLS (callbacks get `DLL_THREAD_ATTACH` and `DETACH`), and the process ends when its last thread does (returning from the entry point ends only the first thread) |
| Waits and objects | `WaitForSingleObject`(`Ex`), `WaitForMultipleObjects`(`Ex`) (any or all, timeouts) on threads, events, mutexes and semaphores; `CreateEventW`/`A`/`ExW`, `SetEvent`, `ResetEvent`, `CreateMutexW`/`A`/`ExW`, `ReleaseMutex`, `CreateSemaphoreW`/`A`/`ExW`, `ReleaseSemaphore`, `DuplicateHandle`, `CloseHandle` |
| Locks | `InitializeCriticalSection`(`AndSpinCount`, `Ex`), `EnterCriticalSection`, `TryEnterCriticalSection`, `LeaveCriticalSection`, `DeleteCriticalSection`; SRW locks (`AcquireSRWLockExclusive`/`Shared`, `TryAcquire...`, `Release...`); condition variables (`SleepConditionVariableCS`/`SRW`, `WakeConditionVariable`, `WakeAllConditionVariable`); `InitOnceExecuteOnce`, `InitOnceBeginInitialize`, `InitOnceComplete`; `WaitOnAddress`, `WakeByAddressSingle`/`All`. Interlocked operations are the compiler's own atomic instructions (lock-prefixed on x64, exclusive or LSE on ARM64) and need nothing from the layer |
| Fiber-local storage | `FlsAlloc` (callbacks run when a thread ends), `FlsFree`, `FlsGetValue`, `FlsSetValue` |
| Directories | `FindFirstFileW`, `FindFirstFileExW`, `FindNextFileW`, `FindClose` (`*` and `?`, names compared ignoring ASCII case, `*.*` matches everything, `.` and `..` listed), `GetFileAttributesW`, `GetFileAttributesExW`, `GetFileInformationByHandle`, `CreateDirectoryW`, `DeleteFileW`, `RemoveDirectoryW`, `GetCurrentDirectoryW`, `SetCurrentDirectoryW`, `GetFullPathNameW` |
| Windows (user32) | `RegisterClassExW`, `RegisterClassW`, `UnregisterClassW`, `CreateWindowExW`, `DestroyWindow`, `IsWindow`, `ShowWindow`, `UpdateWindow`, `GetClientRect`, `AdjustWindowRect`(`Ex`), `GetWindowLongW`, `SetWindowLongW`, `GetWindowLongPtrW`, `SetWindowLongPtrW` (the window procedure, instance, parent, id, styles, user data and the extra bytes), `DefWindowProcW`, `GetSystemMetrics`, `LoadCursorW`, `LoadIconW`, `MessageBoxW` |
| Messages (user32) | `GetMessageW`, `PeekMessageW` (`PM_REMOVE`), `TranslateMessage`, `DispatchMessageW`, `PostMessageW`, `PostQuitMessage`, `SendMessageW`; `WM_NCCREATE`, `WM_CREATE`, `WM_SHOWWINDOW`, `WM_SIZE`, `WM_PAINT`, `WM_ERASEBKGND`, `WM_SETFOCUS`, `WM_KILLFOCUS`, `WM_SYSCOMMAND`, `WM_CLOSE`, `WM_DESTROY`, `WM_NCDESTROY`, `WM_QUIT`, `WM_MOUSEMOVE`, `WM_LBUTTONDOWN`/`UP`, `WM_RBUTTONDOWN`/`UP`, `WM_MBUTTONDOWN`/`UP`, `WM_MOUSEWHEEL`, `WM_KEYDOWN`/`UP`, `WM_SYSKEYDOWN`/`UP`, `WM_CHAR`, `WM_SYSCHAR`, `WM_HELP` |
| Controls and dialogs (user32) | `BUTTON`, `EDIT`, `STATIC`, `LISTBOX` (`WM_SETTEXT`, `WM_GETTEXT`, `WM_GETTEXTLENGTH`, `WM_ENABLE`, `WM_SETFONT`; `BM_GETCHECK`, `BM_SETCHECK`, `BM_GETSTATE`, `BM_SETSTATE`, `BM_SETSTYLE`, `BM_CLICK`; `EM_LIMITTEXT`, `EM_GETLIMITTEXT`, `EM_SETREADONLY`; `LB_ADDSTRING`, `LB_INSERTSTRING`, `LB_DELETESTRING`, `LB_RESETCONTENT`, `LB_SETCURSEL`, `LB_GETCURSEL`, `LB_GETTEXT`, `LB_GETTEXTLEN`, `LB_GETCOUNT`, `LB_GETITEMHEIGHT`), `SetWindowTextW`, `GetWindowTextW`, `GetWindowTextLengthW`, `GetDlgItem`, `GetDlgCtrlID`, `SetDlgItemTextW`, `GetDlgItemTextW`, `SendDlgItemMessageW`, `IsDialogMessageW`, `SetFocus`, `GetFocus`, `SetCapture`, `ReleaseCapture`, `GetCapture`, `EnableWindow`, `IsWindowEnabled`, `IsWindowVisible`, `GetParent`, `MoveWindow` (child windows) |
| Menus and timers (user32) | `CreateMenu`, `CreatePopupMenu`, `AppendMenuW`, `SetMenu`, `GetMenu`, `DrawMenuBar`, `DestroyMenu`, `GetSubMenu`, `GetMenuItemCount`, `GetMenuItemID`, `SetTimer`, `KillTimer`; `WM_COMMAND`, `WM_TIMER` |
| File pickers (comdlg32) | `GetOpenFileNameW`, `GetSaveFileNameW`, `CommDlgExtendedError` |
| Painting (user32) | `BeginPaint`, `EndPaint`, `GetDC`, `ReleaseDC`, `InvalidateRect`, `FillRect`, `DrawTextW` (left, centre, right; top, centre, bottom on one line; `DT_WORDBREAK`, `DT_NOPREFIX`, `DT_EXPANDTABS`, `DT_NOCLIP`, `DT_CALCRECT`, `DT_END_ELLIPSIS`), `GetSysColor`, `GetSysColorBrush` |
| GDI (gdi32) | `CreateSolidBrush`, `CreatePen` (solid or `PS_NULL`), `GetStockObject`, `SelectObject`, `DeleteObject`, `SetTextColor`, `SetBkColor`, `SetBkMode`, `MoveToEx`, `LineTo`, `Rectangle`, `Ellipse`, `SetPixel`, `TextOutW`, `GetTextExtentPoint32W` |
| C runtime startup (kernel32) | `IsDebuggerPresent`, `IsProcessorFeaturePresent` (the baseline features, and the ARMv8.1 atomics on ARM64 processors that have them; the same table is at `0x7ffe0000` in `KUSER_SHARED_DATA`, D151), `InitializeSListHead`, `GetStartupInfoW`/`A`, `SetUnhandledExceptionFilter`, `UnhandledExceptionFilter`, `RtlCaptureContext`, `RtlLookupFunctionEntry`, `RtlVirtualUnwind` (no unwinding yet: no function entry is found) |
| C runtime (vcruntime140) | `memcpy`, `memmove`, `memset`, `memcmp`, `memchr`, `strchr`, `strrchr`, `strstr`, `wcschr`, `wcsrchr`, `wcsstr`, `__C_specific_handler`, `__current_exception`, `__current_exception_context`, `__std_type_info_destroy_list`, `_purecall` |
| C runtime (ucrtbase, `api-ms-win-crt-*`): startup and exit | `_initterm`, `_initterm_e`, `_configure_narrow_argv`, `_configure_wide_argv`, `__p___argc`, `__p___argv`, `__p___wargv`, `_initialize_narrow_environment`, `_initialize_wide_environment`, `_get_initial_narrow_environment`, `_get_initial_wide_environment`, `__p__environ`, `__p__wenviron`, `_get_narrow_winmain_command_line`, `_get_wide_winmain_command_line`, `_set_app_type`, `__setusermatherr`, `_configthreadlocale`, `_set_new_mode`, `_query_new_mode`, `__p__commode`, `__p__fmode`, `_set_fmode`, `_get_fmode`, `_seh_filter_exe`, `_register_thread_local_exe_atexit_callback`, `_crt_atexit`, `_crt_at_quick_exit`, `_initialize_onexit_table`, `_register_onexit_function`, `_execute_onexit_table`, `exit`, `quick_exit`, `_exit`, `_Exit`, `_cexit`, `_c_exit`, `abort`, `terminate`, `_invalid_parameter_noinfo`(`_noreturn`), `_errno`, `_get_errno`, `_set_errno`, `__doserrno` |
| C runtime: heap | `malloc`, `calloc`, `realloc`, `_recalloc`, `free`, `_msize` (the process heap) |
| C runtime: strings and characters | `strlen`, `strnlen`, `strcmp`, `strncmp`, `_stricmp`, `_strnicmp`, `strcpy`, `strncpy`, `strcat`, `strncat`, `_strdup`, `strcpy_s`, `strcat_s`, `wcslen`, `wcsnlen`, `wcscmp`, `wcsncmp`, `_wcsicmp`, `wcscpy`, `wcsncpy`, `wcscat`, `_wcsdup`, `wcscpy_s`, `wcscat_s`, `isalpha`, `isdigit`, `isalnum`, `isspace`, `isupper`, `islower`, `isxdigit`, `ispunct`, `iscntrl`, `isprint`, `tolower`, `toupper`, `towlower`, `towupper`, `iswspace`, `iswalpha`, `iswdigit`, `strtol`, `strtoul`, `strtoll`, `strtoull`, `atoi`, `atol`, `atoll`, `wcstol`, `wcstoul`, `_wtoi`, `_wtol` (the "C" locale) |
| C runtime: streams | `__acrt_iob_func` (`stdin`, `stdout`, `stderr`), `fopen`, `_wfopen`, `fopen_s`, `_wfopen_s`, `fclose`, `fread`, `fwrite`, `fgetc`, `getc`, `fputc`, `putc`, `ungetc`, `fgets`, `fputs`, `puts`, `fflush`, `fseek`, `_fseeki64`, `ftell`, `_ftelli64`, `rewind`, `fgetpos`, `fsetpos`, `feof`, `ferror`, `clearerr`, `_fileno`, `remove`, `_wremove`; `printf`'s family through `__stdio_common_vfprintf`(`_s`), `__stdio_common_vfwprintf`(`_s`), `__stdio_common_vsprintf`, `__stdio_common_vsprintf_s`, `__stdio_common_vsnprintf_s`, `__stdio_common_vswprintf`, `__stdio_common_vswprintf_s`, `__stdio_common_vsnwprintf_s` (`%e`, `%f` and `%g` too) |
| C runtime: low-level I/O (D179) | `_open`, `_wopen`, `_sopen_s`, `_wsopen_s` (Windows paths; `_O_TEMPORARY` removes the name at once; sharing modes checked, files always shared), `_close`, `_read`, `_write`, `_lseek`, `_lseeki64` on the descriptors `_fileno`, `_fdopen` and `_open_osfhandle` use; bytes as they are, in text mode too |
| C runtime: utility and time | `rand`, `srand` (our own generator), `abs`, `labs`, `llabs`, `_abs64`, `qsort`, `bsearch`, `_time64` |
| Classic Win32 apps (D174) | ANSI windows: `RegisterClassA`/`ExA`, `CreateWindowExA`, `DefWindowProcA`, `SendMessageA`, `SetWindowTextA`, `GetWindowTextA`, `GetWindowTextLengthA`, `SendDlgItemMessageA`, `GetDlgItemTextA`, `SetDlgItemTextA`, `MessageBoxA`, `AppendMenuA`, `IsDialogMessageA`, and the `A` names of `GetMessage`, `PeekMessage`, `DispatchMessage`, `PostMessage`, `GetWindowLong`(`Ptr`), `SetWindowLong`(`Ptr`), `LoadCursor`, `LoadIcon` (a window of a class `RegisterClassA` made gets its messages' text as UTF-8, the ANSI code page); `DefDlgProcW`/`A` (dialogs a program builds, owned `WS_POPUP` windows shown as the server's dialogs of their owner), `CheckDlgButton`, `IsDlgButtonChecked`; the `COMBOBOX` drop-down list (`CB_ADDSTRING`, `CB_INSERTSTRING`, `CB_DELETESTRING`, `CB_RESETCONTENT`, `CB_GETCOUNT`, `CB_GETCURSEL`, `CB_SETCURSEL`, `CB_GETLBTEXT`(`LEN`), `CB_FINDSTRING`(`EXACT`), `CB_SELECTSTRING`, `CB_SHOWDROPDOWN`, `CB_GETDROPPEDSTATE`, `CB_GETITEMHEIGHT`; `CBN_*`); `CheckMenuItem`, `EnableMenuItem`, `GetMenuState`, `GetMenuItemRect`; `GetWindowRect`, `ClientToScreen`, `ScreenToClient`, `SetWindowPos` and `MoveWindow` on any window; `SetRect`, `SetRectEmpty`, `CopyRect`, `OffsetRect`, `InflateRect`, `IntersectRect`, `UnionRect`, `IsRectEmpty`, `EqualRect`, `PtInRect`; `SystemParametersInfoA`/`W` (`SPI_GETWORKAREA`), `GetKeyboardState`, `GetKeyState`, `GetAsyncKeyState`, `SetForegroundWindow`, `GetForegroundWindow`, `MessageBeep`, `WinHelpA`/`W` (no help viewer: fails), `GetDialogBaseUnits`, the clipboard (`OpenClipboard`, `EmptyClipboard`, `SetClipboardData`, `GetClipboardData`, `IsClipboardFormatAvailable`, `CountClipboardFormats`, `CloseClipboard`; the program's own), `WM_LBUTTONDBLCLK` for `CS_DBLCLKS` classes; gdi32's `CreateCompatibleDC`, `DeleteDC`, `CreateCompatibleBitmap`, `BitBlt` and `PatBlt` (every raster operation), `Polygon`, `Polyline`, `IntersectClipRect`, `SelectClipRgn`, `CreateRectRgn`(`Indirect`), `CreateFontW`/`A`, `CreateFontIndirectW`/`A`, `GetTextMetricsW`/`A` (Egg Mono's metrics for every font), `GetTextExtentPoint32A`, `ExtTextOutW`/`A`, `TextOutA`, `GetDeviceCaps`, `SetMapMode` (`MM_TEXT`), `CreateHatchBrush` (drawn solid), `StartDocW`/`A`, `EndDoc`, `StartPage`, `EndPage`, `AbortDoc` (no printers: they fail); comdlg32's `GetOpenFileNameA`, `GetSaveFileNameA`, `PrintDlgW`/`A` (`PDERR_NODEFAULTPRN`); comctl32's `InitCommonControls` (also by its ordinal 17) and `InitCommonControlsEx` (the standard classes); kernel32's `CreateDirectoryA`, `DeleteFileA`, `RemoveDirectoryA`, `MoveFileW`/`A`, `MoveFileExW`/`A`, `GetModuleFileNameA`, `FormatMessageA`, `MulDiv`, `GlobalAlloc` moveable blocks with `GlobalLock`, `GlobalUnlock`, `GlobalSize` (and their `Local` names); the C runtime's `getenv`, `atof`, `strtod`, `strspn`, `strcspn`, `sqrtf`, `_wassert`, `_fdopen`, `_open_osfhandle` and `scanf`'s family through `__stdio_common_vsscanf` (`_s` forms included) |
| Common controls, GDI+ and COM (D178) | comctl32's `msctls_statusbar32` (`SB_*`, `CreateStatusWindowW`), `msctls_progress32` (`PBM_*`), `SysTabControl32` (`TCM_*`, `TCN_*`), `SysListView32` in report view with its `SysHeader32` (`LVM_*`, `HDM_*`, `LVN_*`, `NM_*`), `SysTreeView32` (`TVM_*`, `TVN_*`), `ToolbarWindow32` (`TB_*`, `WM_COMMAND`), `ImageList_*`, `InitCommonControlsEx` for every class; STATIC `SS_BITMAP`/`SS_ICON` with `STM_SETIMAGE`; subclassing controls (`GWLP_WNDPROC`, `CallWindowProcW`); `DialogBoxParamW`, `CreateDialogIndirectParamW`, `EndDialog`, `MapDialogRect`; `LoadAcceleratorsW`, `TranslateAcceleratorW`; `InsertMenuItemW`, `SetMenuItemInfoW`, `GetMenuItemInfoW`, `TrackPopupMenuEx`; window properties, `RegisterWindowMessageW`, `EnumWindows`, `GetWindow`, `GetAncestor`, the US keyboard layout (`ToUnicodeEx`, `VkKeyScanExW`, `MapVirtualKeyW`), `SendInput` to the program's own windows; gdi32's `CreateDIBSection`, `GetDIBits`, `GetObjectW`, `GetPixel`, pattern brushes; `CreateIconIndirect`, `CreateIconFromResourceEx`, `GetIconInfo`, `DrawIconEx`, `CopyImage`, `LoadImageW`; `FindResourceW`, `LoadResource`, `LockResource`, `SizeofResource`, `EnumResourceNamesW`; GDI+'s `GdiplusStartup`, `GdipCreateBitmapFromFile` (PNG, BMP), `GdipCreateHBITMAPFromBitmap` and the image functions; ole32's `CoInitialize`(`Ex`), `CoCreateInstance` (in-process: `Scripting.Dictionary`), `CLSIDFromProgID`, `CreateStreamOnHGlobal`; oleaut32's BSTRs, VARIANTs and SAFEARRAYs; advapi32 (an empty, read-only registry), wsock32, WinINet (HTTP), version resources, `StrCmpLogicalW`, shell32's folders, file operations and `ShellExecuteExW` of programs; kernel32's locale functions (en-US), `.ini` files, `CreateProcessW` and Toolhelp snapshots |
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

- **Programs on the C++ standard library's DLL (`msvcp140.dll`) and the rest of Rust's std**:
  they import more (iostreams, `CopyFileExW`, more of the C library), which later parts add. The layer names the first function it lacks. C++'s `std::exception_ptr`,
  `dynamic_cast` and `typeid` are missing too (D177's "Not yet"). Of MSVC's C runtime,
  `setlocale`, `setjmp`, the math library beyond `sqrtf`, wide streams, low-level I/O's line
  ending translation and permission modes (`_open` itself works, D179), `rename`, `tmpfile` and `_set_invalid_parameter_handler` are missing (D146's "Not yet"); a
  function given an invalid parameter returns its documented error instead of ending the
  program, and one lock serialises all of stdio (a thread blocked reading `stdin` holds up
  the others' stdio calls).
- **Named pipes** (`CreateNamedPipeW`, `ConnectNamedPipe`; overlapped reads and writes work on
  anonymous pipes and files), handles duplicated into another process, a child that really waits
  suspended (`CREATE_SUSPENDED` starts it at once), job objects, and the C runtime's
  `_spawn*`, `system` and `_popen` (D189's "Not yet"); **UTF-16 console polish** (line endings
  as `\r\n`, `ReadConsoleW` across multi-byte input): a later part.
- **x64 programs on aarch64** (and ARM64 programs on x86_64): an x64 program on Arm needs its
  code translated ahead of time (D17), which EggOS does not do yet (designed in D191,
  [translation](translation.md); only the decoder exists so far); ARM64EC and ARM64X hybrid
  images run only as plain ARM64 code (their x64 parts are not supported).
- **DOS wildcard special cases** (`a?` matching `a`, `*.` matching names without an
  extension); `lstrcmpW` and `lstrcmpiW` compare ordinally, not by locale;
  `DuplicateHandle(GetCurrentThread())` is refused.
- `SuspendThread`, `TerminateThread`, `FlsFree` and `TlsFree` clearing other threads' slots
  (only the caller's), the TLS expansion slots (64 TLS and 64 FLS slots per thread); opening a
  named mapping of a file from another program, and `Local\` names per user (D189's "Not
  yet").
- **Creating and removing** files and directories: the file system is read-only, so
  `CreateDirectoryW`, `DeleteFileW` and `RemoveDirectoryW` on a path that exists (or could)
  fail with `ERROR_ACCESS_DENIED`.
- **Time zones**: local time is UTC.
- **Exceptions through the layer's own code** (D177's "Not yet"): a fault inside a Windows
  function the program called ends the program, as does an exception thrown out of a window
  procedure through `DispatchMessage`, or one raised inside a filter or `__finally` block and
  caught outside its function. A stack overflow is not an exception yet, and on ARM64 a
  context resumed after a fault loses `x16`. On ARM64 a read of `CurrentEL`, which MSVC's
  atomics helpers make once, still reads exception level 0 without an exception (D151).
- **`KUSER_SHARED_DATA`**: the page at `0x7ffe0000` holds only `ProcessorFeatures`; its clocks,
  version numbers and the rest read as zero.
- **DLLs**: only the layer's own kernel32, ntdll, shell32, user32, gdi32, comdlg32, comctl32,
  vcruntime140, the Universal C Runtime, gdiplus, ole32, oleaut32, advapi32, wsock32, winmm,
  version, psapi, wininet, shlwapi, uxtheme and dwmapi (what the tables above list); a program
  that imports another DLL (ws2_32, its own) or a function they lack is refused, and the layer
  names it. Imports by ordinal work for comctl32, oleaut32 and wsock32.
- **GUI programs beyond parts 1 and 2** (D129, D145): multi-line edits, selection and the
  clipboard in edits, `ES_PASSWORD` and `ES_NUMBER`, radio buttons and group boxes, combo
  boxes, scroll bars, owner-drawn controls,
  `WM_CTLCOLOR*` (controls draw Windows' default colours), hover looks, per-control pointers,
  double clicks, menu keys (F10, Alt and mnemonics), submenus,
  `EnableMenuItem` and `CheckMenuItem`, the picker's overwrite and not-found questions and its
  filter choice, `GetTextMetricsW`, fonts other
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
