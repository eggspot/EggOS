# Running macOS programs

EggOS runs macOS programs natively, console programs and AppKit apps: arm64 programs on
aarch64 and x86_64 programs on x86_64. The program's own machine code runs directly on the
CPU, with no emulator, no virtual machine and no translation (D17). The macOS layer (D120), an optional component at
`/usr/lib/eggos/macos`, does what dyld does on a Mac: it reads the program's Mach-O file
(thin, or fat/universal with a slice per CPU), maps its segments, binds the functions it imports
from libSystem to its own implementations, written in Rust on EggOS's system calls, applies the
file's chained fixups and calls `main(argc, argv, envp, apple)`. A libSystem call is a plain
function call; no system call happens unless the function needs the kernel (a write, a file
read), and `malloc` allocates without one.

Part 1 (D120) runs console programs that use the libSystem functions listed below; part 2
(D123) adds what small real tools and Rust's standard library need: threads, thread-local
variables, locks, `getopt`, calendar time, `errno` per thread and the C locale. Part 3 (D142)
runs Objective-C console programs: the layer has its own Objective-C runtime (libobjc) and the
core of Foundation (see "Objective-C programs"). Part 4 (D147) runs GUI apps built in code on
AppKit's core, in an `.app` bundle or not (see "GUI apps"). Part 5 (D180) runs Swift console
programs on the layer's own Swift runtime (see "Swift programs"). Swift GUI apps, C++'s runtime
and programs that need other libraries do not run yet (see below).

## Add a program to the image

```
cargo xtask image --arch aarch64 --add path/to/hello
cargo xtask run --arch x86_64 --add path/to/hello      # build that image and boot it
```

`--add` works as for [Linux programs](linux-programs.md): the file goes to `/bin/hello`. The
aarch64 and x86_64 release images carry the macOS layer at `/usr/lib/eggos/macos` (D152), with
or without added files, so a program in the host folder (`--apps`) runs too
(`/media/host/hello`). At the prompt, type
`hello` (or its path) and its arguments; the shell reports the exit status in `$?`, and an
`abort()` as `Aborted` and `134`.

A file without code for the machine's CPU (an x86_64-only program on aarch64, an arm64e
program) is refused with a message naming the CPUs it has and status 126. On riscv64 (no macOS
program exists for it, and its image carries no macOS layer), a macOS program is refused with
`Exec format error` and status 126. A Java class file, which starts with the same bytes as a fat
file, is not taken for one.

## Install a program as an app

On aarch64 and x86_64, `app install /media/host/Tool.app` installs an `.app` bundle whole
(or `app install /media/host/tool` a Mach-O tool) into `/Applications/Tool/`, with a launcher
row and a desktop icon that run its `Contents/MacOS` program in the terminal; `app uninstall
Tool` removes it. See [Apps](apps.md) (D134).

## Build a program that runs

C, Rust or any language whose program links only libSystem, with Apple's linker and a
deployment target of macOS 13 or later, which gives chained fixups:

```
MACOSX_DEPLOYMENT_TARGET=13.0 clang -O2 -o hello hello.c
lipo -create -output hello-universal hello-arm64 hello-x86_64      # a fat file, if wanted
cargo xtask macos-programs     # builds tests/macos-console for both CPUs, on a Mac
```

Rust programs on the standard library run too, built with `cargo build --target
aarch64-apple-darwin` (or `x86_64-apple-darwin`) and `MACOSX_DEPLOYMENT_TARGET=13.0`: their
arguments, environment, standard streams, files, directories, `std::thread` (spawn, join,
names), `Mutex`, `thread_local!` and `std::process::exit` work. A panic prints its message;
with the default `panic = "unwind"` unwinding stops at the first unwinder function with status
127 (`panic = "abort"` aborts with 134).

arm64 macOS programs are compiled for Apple's M1 and later, so they use instructions an
ARMv8.0-A CPU lacks (LSE atomics, RCPC loads): on aarch64 they need a CPU of ARMv8.4-A level or
later, and their boot test runs QEMU's `max` CPU.

`tests/macos-console` is such a program: `#![no_std]` Rust that defines `main` and calls
libSystem's C functions, built for `aarch64-apple-darwin` and `x86_64-apple-darwin`
(`cargo xtask macos-programs` builds both on a Mac). The boot test (`cargo xtask qemu`, on a
Mac or with `EGGOS_MACOS_PROGRAMS` naming a folder of the programs; CI builds them on its macOS
runner) runs it from `eggsh` by name and by path and checks its arguments and `apple[]`, the
environment (`getenv`, `setenv`, `envp`), `printf`'s integer, width, precision and floating-point
formats, `snprintf`'s truncation, `puts`, the string functions, `qsort`, `strtol` and `atoi`,
reading a file through `fopen` and `fgets` and through `open` and `read`, a missing file's
`ENOENT` and the read-only file system's refusal, `stat` (Darwin's layout), listing a directory
(Darwin's `struct dirent`), `chdir` and `getcwd`, `malloc`, `calloc` and `realloc`, the clocks,
`mach_absolute_time`, `usleep`, `gettimeofday`, standard error, `atexit`, `exit(42)`, `abort()`
and a call to a function the layer lacks. It also runs a fat file of both slices, and checks the
refusals of a program that links `libz` and of the other CPU's program.

Two more programs come from the same package. `macos-tools` (`no_std`, C functions only) parses
`-v --verbose -n 3 --list /usr/share/eggos --name=eggos rest more` with `getopt_long`, lists the
directory with `opendir` and `readdir`, formats the epoch and a fixed time with `localtime_r`,
`gmtime_r`, `timegm` and `strftime`, sleeps with `nanosleep`, reads a missing file's `errno`
through `__error` and `strerror`, asks `setlocale`, `nl_langinfo` and `localeconv`, and an
unknown option ends it with `getopt`'s message and status 2. `macos-std` is a Rust std program:
its arguments, an environment variable, a file, `/bin`'s listing, a missing file's
`ErrorKind::NotFound`, four named threads that add to a `Mutex` and count calls in a
`thread_local!`, and `std::process::exit(5)`.

## What the layer provides

| Area | Functions |
|------|-----------|
| stdio | `printf`, `fprintf`, `dprintf`, `sprintf`, `snprintf`, `asprintf`, `vprintf`, `vfprintf`, `vdprintf`, `vsprintf`, `vsnprintf`, `vasprintf`, `puts`, `fputs`, `fputc`, `putc`, `putchar`, `fwrite`, `fread`, `fgetc`, `getc`, `getchar`, `ungetc`, `fgets`, `fflush`, `fopen`, `fdopen`, `fclose`, `fileno`, `feof`, `ferror`, `clearerr`, `fseek`, `fseeko`, `ftell`, `ftello`, `rewind`, `setvbuf`, `perror`, and `__stdinp`, `__stdoutp`, `__stderrp` |
| Strings and memory | `strlen`, `strnlen`, `strcmp`, `strncmp`, `strcasecmp`, `strncasecmp`, `strcpy`, `strncpy`, `strcat`, `strncat`, `strlcpy`, `strchr`, `strrchr`, `strstr`, `strdup`, `strndup`, `strerror`, `memchr`, `memcmp`, `bcmp`, `memcpy`, `memmove`, `memset`, `bzero`, and the `_FORTIFY_SOURCE` checks `__memcpy_chk`, `__memmove_chk`, `__memset_chk`, `__strcpy_chk`, `__strncpy_chk`, `__strcat_chk`, `__sprintf_chk`, `__snprintf_chk`, `__vsnprintf_chk` |
| Heap | `malloc`, `calloc`, `realloc`, `reallocf`, `free`, `malloc_size`, `posix_memalign`, `aligned_alloc`, `valloc` |
| Numbers | `atoi`, `atol`, `atoll`, `strtol`, `strtoll`, `strtoul`, `strtoull`, `abs`, `labs`, `llabs`, `qsort`, `bsearch`, `rand`, `srand`, `arc4random`, `arc4random_buf`, `arc4random_uniform` |
| Files | `open`, `close`, `read`, `write`, `lseek`, `stat`, `lstat`, `fstat` (on x86_64 only their `$INODE64` names: the unsuffixed ones are the old 32-bit-inode ABI, and calling them stops with status 127), `isatty`, `access`, `getcwd`, `chdir`, `unlink`, `rmdir`, `mkdir`, `opendir`, `readdir` (`readdir$INODE64` on x86_64, likewise), `closedir`, `dirfd` |
| Files, part 2 | `dup`, `dup2`, `pipe`, `fcntl` (`F_DUPFD`, `F_DUPFD_CLOEXEC`, `F_GETFD`, `F_SETFD`, `F_GETFL`, `F_SETFL`), `writev` |
| Process and environment | `exit`, `_exit`, `_Exit`, `abort`, `atexit`, `raise`, `kill`, `getpid`, `getppid`, `getuid`, `geteuid`, `getenv`, `setenv`, `putenv`, `unsetenv`, `environ`, `_NSGetArgc`, `_NSGetArgv`, `_NSGetEnviron`, `_NSGetProgname`, `_NSGetExecutablePath`, `__error` (`errno`, one per thread), `strerror_r`, `__stack_chk_guard`, `__stack_chk_fail`, `getpagesize`, `sysconf` (page size, CPUs, `_SC_OPEN_MAX`, `_SC_CLK_TCK`, `_SC_ARG_MAX`) |
| Options | `getopt`, `getopt_long`, `optarg`, `optind`, `opterr`, `optopt`, `optreset` |
| Threads | `pthread_create`, `pthread_join`, `pthread_detach`, `pthread_exit`, `pthread_self`, `pthread_equal`, `pthread_main_np`, `pthread_threadid_np`, `pthread_mach_thread_np`, `pthread_get_stackaddr_np`, `pthread_get_stacksize_np`, `pthread_setname_np`, `pthread_getname_np`, `pthread_attr_init`, `pthread_attr_destroy`, `pthread_attr_setstacksize`, `pthread_attr_getstacksize`, `pthread_attr_setdetachstate`, `pthread_attr_getdetachstate`, `pthread_attr_setguardsize`, `pthread_key_create`, `pthread_key_delete`, `pthread_getspecific`, `pthread_setspecific`, `sched_yield`, thread-local variables (`_tlv_bootstrap`, `_tlv_atexit`) |
| Locks | `pthread_mutex_init`, `pthread_mutex_destroy`, `pthread_mutex_lock`, `pthread_mutex_trylock`, `pthread_mutex_unlock` (normal, error-checking, recursive, and the static initializers), `pthread_mutexattr_init`, `pthread_mutexattr_destroy`, `pthread_mutexattr_settype`, `pthread_cond_init`, `pthread_cond_destroy`, `pthread_cond_wait`, `pthread_cond_timedwait`, `pthread_cond_signal`, `pthread_cond_broadcast`, `pthread_once`, `dispatch_semaphore_create`, `dispatch_semaphore_wait`, `dispatch_semaphore_signal`, `dispatch_time`, `dispatch_retain`, `dispatch_release` |
| Signals | `sigaction`, `__sigaction`, `signal`, `sigaltstack`, `sigprocmask`, `pthread_sigmask`, `sigsuspend`, `sigpending`, `sigemptyset`, `sigfillset`, `sigaddset`, `sigdelset`, `sigismember`, `pause`, `killpg` |
| Other programs | `fork`, `vfork` (as `fork`), `execve`, `execv`, `execvp`, `execvP`, `execl`, `execle`, `execlp`, `posix_spawn`, `posix_spawnp`, `posix_spawn_file_actions_init`, `_destroy`, `_addopen`, `_addclose`, `_adddup2`, `_addinherit_np`, `_addchdir_np`, `_addfchdir_np` (and the unsuffixed `addchdir`, `addfchdir`), `posix_spawnattr_init`, `_destroy`, `_setflags`, `_getflags`, `_setpgroup`, `_getpgroup`, `_setsigmask`, `_getsigmask`, `_setsigdefault`, `_getsigdefault`, `wait`, `waitpid`, `wait3`, `wait4`, `popen`, `pclose`, `system` |
| Memory | `mmap` (Darwin's flags), `munmap`, `mprotect`, `madvise` (a hint: accepted) |
| Time | `clock_gettime`, `clock_gettime_nsec_np`, `gettimeofday`, `time`, `mach_absolute_time`, `mach_continuous_time`, `mach_timebase_info` (1/1: nanoseconds), `nanosleep`, `usleep`, `sleep`, `gmtime_r`, `gmtime`, `localtime_r`, `localtime`, `timegm`, `mktime`, `strftime`, `tzset` |
| Locale | `setlocale`, `nl_langinfo`, `localeconv`, `__mb_cur_max` (the C locale only) |
| dyld | `_dyld_image_count`, `_dyld_get_image_header`, `_dyld_get_image_vmaddr_slide`, `_dyld_get_image_name`, `dlsym` (the layer's own functions), `dlopen` (loads nothing), `dlclose`, `dlerror` |

Darwin's numbers become Linux's at the boundary: `open`'s flags, error numbers (`EAGAIN` is
35, `ENOSYS` 78), clocks (`CLOCK_MONOTONIC` is 6), signals, `struct stat` and `struct dirent`.
`printf` supports the flags `- + space # 0`, `*` widths and precisions, the length modifiers
`hh h l ll q j z t L` (`%Lf`'s 80-bit x86_64 `long double` printed through the nearest
double) and the conversions `d i u o x X c s p f F e E g G %`; `%n` writes nothing.
Floating-point output is rounded from the double's value scaled in double precision, so in
rare cases (a value whose decimal expansion sits next to a rounding boundary, or one above
about 10^18) its last digit differs from a Mac's, which prints the exact decimal value.

An import the layer lacks does not stop the program from starting: it is bound to a stub, and
only calling it ends the program with `eggos-macos: unimplemented libSystem function <name>`
and status 127 (as `dlsym` failures and missing weak functions surface on a Mac only when
used). An import from a library other than libSystem, libobjc, Foundation and CoreFoundation
refuses the program before any of its code runs:
`macos: /bin/x: needs /usr/lib/libz.1.dylib, which EggOS does not provide yet`, status 126.

Threads are Linux threads of the program's process. Each has a control block that its thread
pointer (`tpidr_el0` on aarch64, the FS base on x86_64, neither of which macOS programs use)
points at, holding its `errno`, its copy of the program's thread-local template
(`__thread_data` and `__thread_bss`) and its `pthread_key_t` values; `_tlv_bootstrap`, which
every thread-local variable's descriptor calls, is the accessor itself: three instructions that
return the calling thread's copy plus the variable's offset. Locks are futexes in the program's
own memory. Time is UTC (`localtime_r` is `gmtime_r`, `%Z` is `UTC`), and the only locale is C
(`nl_langinfo(CODESET)` is `US-ASCII`, as on a Mac).

### Signals and other programs

Signals use Darwin's numbers (`SIGUSR1` is 30, `SIGCHLD` 20) and structures (D190). A handler
`sigaction` or `signal` installs runs when the signal arrives: the layer installs one function
of its own as the Linux handler of every caught signal, which calls the program's handler with
Darwin's number and, with `SA_SIGINFO`, Darwin's `siginfo_t` (the sender's pid, user and
`si_code`, a fault's address, a child's pid and status); the handler returns through the
kernel's trampoline and `rt_sigreturn`, and `errno` is kept across it. `signal` restarts the
calls a handler interrupts, as BSD's does. `SA_ONSTACK` runs it on the stack `sigaltstack` set
(at least `MINSIGSTKSZ`, 32 KiB), and masks, `sigsuspend` and `sigpending` translate Darwin's
32-bit `sigset_t`; Darwin's `SIGEMT` and `SIGINFO` never arrive (Linux has neither).

`fork` copies the process (any thread but the caller is gone in the child, as on a Mac), the
`exec` family runs whatever the path names (another macOS program starts the layer again, a
Linux program runs as it is), and `execvp` and `posix_spawnp` search `PATH` (`/usr/bin:/bin`
when it is unset). `posix_spawn` starts a child that takes the attributes (`POSIX_SPAWN_SETSID`,
`SETPGROUP`, `RESETIDS`, `SETSIGDEF`, `SETSIGMASK`, `CLOEXEC_DEFAULT`, and `SETEXEC`, which
runs the program in the caller) then the file actions in order, and reports a failure to start
(`ENOENT` for a missing program) as its result, leaving no child behind; it refuses
`POSIX_SPAWN_START_SUSPENDED` with `ENOTSUP`. Wait statuses use Darwin's signal numbers.
`system` and `popen` run EggOS's shell, `/bin/eggsh`, as `sh -c -- command`; `system` ignores
`SIGINT` and `SIGQUIT` and blocks `SIGCHLD` while it waits, as a Mac does. The test program
`tests/macos-process/main.c`, built by `cargo xtask macos-programs` with
`clang -mmacosx-version-min=13.0`, checks all of it in the macOS programs boot test.

## Objective-C programs

A program built with clang for macOS 13 or later, with ARC and `-framework Foundation`, runs
from `eggsh` like any other: `clang -fobjc-arc -framework Foundation -mmacosx-version-min=13.0
-arch arm64 hello.m -o hello` on a Mac, then add `hello` to the image. The layer accepts
`/usr/lib/libobjc.A.dylib`, Foundation and CoreFoundation besides libSystem and binds their
imports to its own runtime (D142). The boot test's `macos-objc`
(`tests/macos-objc/main.m`, built by `cargo xtask macos-programs`) prints the same lines as on a
Mac.

What works:

- **Classes and categories** from `__objc_classlist` and `__objc_catlist`, on the program's own
  classes and on Foundation's (a category on NSString merges into the layer's NSString),
  method lists in both the pointer and the relative form, `+load` (classes, superclasses first,
  then categories, before `main`), `+initialize` (sent before a class's first message,
  superclasses first, other threads waiting), `[super ...]` in instance and class methods,
  `@property` (strong, copy, assign, atomic or not), non-fragile ivars (moved when a superclass
  is larger than the program was compiled for), ARC's `.cxx_destruct`.
- **Messages**: `objc_msgSend` and `objc_msgSendSuper2` in assembly on arm64 and x86_64 (and
  x86_64's `_stret` forms for large structs): a per-class method cache probed with no lock and
  no stack, about ten instructions on a hit; a miss finds the method up the superclass chain
  and fills the cache. A message to nil returns zeros. An unknown selector ends the program as
  on a Mac: `*** Terminating app due to uncaught exception 'NSInvalidArgumentException',
  reason: '-[Counter frobnicate]: unrecognized selector sent to instance 0x...'`, then
  `libc++abi: terminating due to uncaught exception of type NSException` and `abort` (status
  134; the Mac's call stack lines are left out).
- **Memory**: reference counts kept in a word before each object, `retain`, `release`,
  `autorelease`, `dealloc`, ARC's functions (`objc_retain`, `objc_release`, `objc_storeStrong`,
  `objc_autoreleaseReturnValue`, `objc_retainAutoreleasedReturnValue`, `objc_alloc`,
  `objc_alloc_init`, `objc_opt_new`, `objc_opt_class`, `objc_opt_isKindOfClass`,
  `objc_opt_respondsToSelector`, the property accessors), and autorelease pools per thread
  (`@autoreleasepool`, NSAutoreleasePool); a thread's leftover autoreleased objects are released
  when it ends.
- **The runtime's functions**: `objc_getClass`, `objc_lookUpClass`, `objc_getMetaClass`,
  `object_getClass`, `object_getClassName`, `class_getName`, `class_getSuperclass`,
  `class_isMetaClass`, `class_respondsToSelector`, `class_getInstanceSize`,
  `class_getInstanceMethod`, `class_getClassMethod`, `method_getImplementation`,
  `method_getName`, `class_addMethod`, `class_createInstance`, `object_dispose`,
  `sel_registerName`, `sel_getUid`, `sel_getName`, `sel_isEqual`.
- **Foundation's core**: NSObject (`alloc`, `new`, `init`, `class`, `isKindOfClass:`,
  `respondsToSelector:`, `isEqual:`, `hash`, `description`, `copy`, `performSelector:`, ...);
  NSString (constant `@"..."` literals, ASCII or UTF-16, and dynamic strings: creation from
  UTF-8, characters and formats, `length`, `characterAtIndex:`, `UTF8String`, comparison,
  `hasPrefix:`, `hasSuffix:`, `containsString:`, appending, substrings, ASCII case changes,
  `componentsSeparatedByString:`, `stringByReplacingOccurrencesOfString:withString:`, number
  values) and NSMutableString (`appendString:`, `appendFormat:`, `setString:`,
  `insertString:atIndex:`, `deleteCharactersInRange:`); NSArray and NSMutableArray (`@[...]`
  literals, subscripts, fast enumeration with the mutation check, `componentsJoinedByString:`,
  `sortedArrayUsingSelector:`, the mutating methods); NSDictionary and NSMutableDictionary
  (`@{...}` literals, subscripts, fast enumeration of keys, `allKeys`, `allValues`, the
  mutating methods); NSNumber (`@42`, `@3.5`, `@YES`, `@(x)` and the `numberWith...:` and
  `...Value` methods); NSBundle's `mainBundle` (the executable's directory, no identifier, an
  empty information dictionary); `NSLog` and `NSLogv` (`YYYY-MM-DD HH:MM:SS.mmm name[pid:tid]
  message` on standard error, in UTC); `NSStringFromClass`, `NSClassFromString`,
  `NSStringFromSelector`, `NSSelectorFromString`; `%@` and `%C` in every format.
- **Blocks** (D175): block literals on the stack and as constants, `Block_copy` and
  `Block_release` (and ARC's `objc_retainBlock`), captured objects, blocks and `__block`
  variables kept by the compiler's helpers, and `copy`, `retain`, `release` and `autorelease`
  on blocks; `enumerateObjectsUsingBlock:` on arrays and sets and timer blocks call them.
- **`__weak` references** (D175): `objc_initWeak`, `objc_storeWeak`, `objc_loadWeakRetained`,
  `objc_loadWeak`, `objc_destroyWeak`, `objc_copyWeak` and `objc_moveWeak`; a weak variable
  becomes nil when its object is freed and never yields one being deallocated.
- **Exceptions** (D175): `@throw`, `-[NSException raise]`, `+raise:format:`, `@catch` by class
  or `id`, `@finally`, `@throw;` to rethrow, through any number of the program's frames,
  unwound with the image's compact unwind information; an uncaught exception runs the handler
  `NSSetUncaughtExceptionHandler` set, then ends the program as on a Mac
  (`*** Terminating app due to uncaught exception 'Name', reason: '...'`, status 134).
- **More of Foundation** (D175): NSException; NSProcessInfo (`processName`, `arguments`,
  `environment`, `processIdentifier`, `hostName`, processors, memory, uptime); NSDate; NSError
  with the error domains and `userInfo` keys; NSData and NSMutableData; NSSet and NSMutableSet
  (hashed); NSTimer and NSRunLoop (`runUntilDate:` sleeps until each timer is due);
  `rangeOfString:` and `rangeOfString:options:`.
- **Descriptions as a Mac prints them**: nested arrays and dictionaries with the same
  indentation, strings quoted unless they are only ASCII letters and digits (with `\"`, `\n`
  and `\Uxxxx` escapes), dictionaries with string keys in key order, doubles with 16
  significant digits and floats with 7.

A Foundation class the layer lacks stops the program only when it is first used:
`eggos-macos: unimplemented Foundation class NSUUID`, status 127; a missing function
names its library the same way (`unimplemented Foundation function NSHomeDirectory`).

## Swift programs

A Swift program built for macOS 13 or later runs from `eggsh` like any other: `swiftc -O
-target arm64-apple-macos13.0 hello.swift -o hello` on a Mac, then add `hello` to the image.
Such a program links `/usr/lib/swift/libswiftCore.dylib`, the system's Swift runtime, which the
layer provides itself, in Rust (D180). The boot test's `macos-swift`
(`tests/macos-swift/main.swift`, built by `cargo xtask macos-programs`) prints the same lines as
on a Mac, `macos-swift conflict` stops at an exclusivity violation as a Mac does, and
`macos-swift fatal error` ends with `fatalError`'s message.

What works:

- **Classes, structs, protocols and generics** of the program: objects counted the Swift way
  and freed with their `deinit` at their last release; Swift classes are Objective-C classes
  under `_SwiftObject`, and Objective-C's `retain` and `release` reach their count.
- **Values of any type through their value witnesses**: generic functions, `Optional`,
  existentials (`Any`, `any P`), the builtin types the compiler names.
- **`String`**: literals, small strings, appending and growing, interpolation, comparison and
  hashing (by UTF-8 bytes); **`Array`** and **`Dictionary`** with any element, key and value
  types the program's own code specializes; `Hasher` with a random seed per run
  (`SWIFT_DETERMINISTIC_HASHING` turns it off).
- **`print`** of strings, `Bool` and the numbers (`Double` as Swift writes it: `0.1`, `2.0`,
  `1e-05`, `1.5e+300`), interleaved in order with the C library's `printf`;
  `CommandLine.argc`.
- **Exclusivity checks**: two overlapping accesses to the same variable, one of them a
  modification, end the program with Swift's message.
- **The program's conformances to the standard library's protocols** (`Equatable`, `Hashable`,
  `RawRepresentable`, `CaseIterable`), used through generic code: the runtime builds their
  witness tables.
- **Dynamic casts** (`as?`, `as!`) to concrete types, classes, optionals, `Any` and
  `AnyObject`; NSNumber and NSString cast to Swift's numbers, `Bool` and `String` when the
  value fits exactly.
- **Weak references** (`weak var`, `[weak self]`) to Swift and Objective-C objects, `nil` once
  the object goes.
- **`String`'s characters**: `count`, iteration, indices moved by characters, the character
  and range subscripts, `lowercased()`, `Character` in sets and dictionaries; characters follow
  Unicode's grapheme rules for Latin, Greek, Cyrillic, Hangul, combining marks, emoji sequences
  and flags.
- **`type(of:)` names, `Int.random`, `fatalError`** (`file:line: Fatal error: message`).

What does not yet: Foundation's Swift overlay (`Data`, bridging to NSString, NSArray and
NSDictionary), the Dispatch and CoreGraphics overlays, casts to a protocol (`as? any P`),
the program's own generic types named in metadata, printing other types, full Unicode tables,
and Swift GUI apps (D180's "Not yet"). A program that uses one of these ends with
`eggos-macos: unimplemented ...` and status 127 when it reaches it.

## GUI apps

A program that links AppKit (`-framework AppKit`, or the Cocoa umbrella, D175) gets the
layer's own AppKit (D147,
`userland/macos/src/appkit/`), drawn as `docs/ux/macos-gui-v0.3.md` designs it: a macOS app in
Dark Mode on the EggOS desktop. Install an `.app` folder with `app install` (docs/apps.md) and
open it from the launcher, the dock or the desktop; its windows are windows of the window
server (docs/windows.md) with EggOS's chrome, and its main menu shows in the macOS profile's top
bar while one of its windows is focused.

- **NSApplication**: `sharedApplication`, `NSApp`, `setDelegate:` (retained),
  `setActivationPolicy:`, `run` (the delegate's `applicationWillFinishLaunching:` and
  `applicationDidFinishLaunching:`, then the event loop), `stop:`, `terminate:` (asks
  `applicationShouldTerminate:`, tells `applicationWillTerminate:`), `setMainMenu:`,
  `keyWindow`, `windows`, `sendAction:to:from:` (to the target, or the key window's first
  responder, its superviews, the window, its delegate, the application and its delegate),
  `sendEvent:`. The last window closed ends the app; Cmd+Q always quits.
- **NSWindow**: `initWithContentRect:styleMask:backing:defer:` (resizable or not), `title`,
  `contentView`, `makeKeyAndOrderFront:`, `close`, `performClose:` (`windowShouldClose:`),
  `delegate` (`windowWillClose:`), `makeFirstResponder:`, `initialFirstResponder`,
  `backgroundColor`, `setContentSize:`, `display`, `sendEvent:`.
- **NSView and NSResponder**: `initWithFrame:`, `frame`, `bounds`, `setFrame:`, `addSubview:`,
  `removeFromSuperview`, `subviews`, `superview`, `window`, `setNeedsDisplay:`, `drawRect:`,
  `hidden`, `convertPoint:fromView:` and `toView:`, `hitTest:`, `acceptsFirstResponder`,
  `mouseDown:`, `mouseUp:`, `mouseDragged:`, `rightMouseDown:`, `scrollWheel:`, `keyDown:`,
  `keyUp:`, `flagsChanged:`.
- **NSEvent**: `type`, `locationInWindow`, `modifierFlags`, `characters`,
  `charactersIgnoringModifiers`, `keyCode` (Apple's virtual key codes), `buttonNumber`,
  `clickCount`, `timestamp`, `window`.
- **Drawing**: NSColor's fixed and semantic colours (the desktop's dark tokens) and
  `colorWithRed:green:blue:alpha:`, `set`, `setFill`, `setStroke`; `NSRectFill`,
  `NSFrameRect`, `NSEraseRect`; NSBezierPath (`bezierPathWithRect:`,
  `bezierPathWithOvalInRect:`, `moveToPoint:`, `lineToPoint:`, `closePath`, `fill`, `stroke`,
  `fillRect:`, `strokeRect:`); NSString's `drawAtPoint:withAttributes:`,
  `drawInRect:withAttributes:` and `sizeWithAttributes:` with `NSForegroundColorAttributeName`
  and `NSFontAttributeName`; NSFont, which is Egg Sans at every size.
- **Controls**: NSButton (`buttonWithTitle:target:action:`, `title`, `keyEquivalent` (Return
  makes the default button), `enabled`, `performClick:`), NSTextField
  (`textFieldWithString:`, `labelWithString:`, `stringValue`, `placeholderString`,
  `editable`, `bezeled`, `textColor`, `alignment`; Enter sends the action, Tab moves between
  fields), NSControl's `target`, `action` and `tag`.
- **NSAlert**: `messageText`, `informativeText`, `addButtonWithTitle:` (up to three),
  `runModal` (1000, 1001, 1002).
- **NSMenu and NSMenuItem**: `initWithTitle:`, `addItem:`,
  `addItemWithTitle:action:keyEquivalent:`, `separatorItem`, `submenu`, `enabled`,
  `keyEquivalentModifierMask`; the top level's submenus are the top bar's menus.
- **NSBundle** in an `.app`: `bundlePath`, `resourcePath`, `infoDictionary`,
  `objectForInfoDictionaryKey:` and `bundleIdentifier` from `Contents/Info.plist`.
- **Views moving between windows** (D175): `viewWillMoveToWindow:` and `viewDidMoveToWindow`,
  to the view and its subviews, top down.
- **The standard About panel** (D175, `docs/ux/macos-gui-v0.3.md` section 8):
  `orderFrontStandardAboutPanel:` and `orderFrontStandardAboutPanelWithOptions:` show the app's
  installed icon, name, `Version <CFBundleShortVersionString> (<CFBundleVersion>)` and
  `NSHumanReadableCopyright` in a window of the layer's own; Esc closes it.
- **Timers in the event loop** (D175): scheduled NSTimers fire while `run` waits for events.
- **Agents** (D175): every window answers `wm show` with its views as an element tree (buttons,
  text fields, labels, groups and the app's own views as graphics) and `wm act` presses
  buttons, focuses views and sets text fields' text.

`tests/macos-gui/main.m` (`mac-paint.app`) is a complete example. An AppKit class the layer
lacks stops the app when it is first used (`eggos-macos: unimplemented AppKit class NSImage`).

## What does not work yet

- **Objective-C beyond the core**: exceptions through frames only `__eh_frame` (DWARF)
  describes or through the layer's own code (an exception thrown in an event handler or a block
  Foundation called ends the program as uncaught), exceptions Foundation itself raises (they
  still end the program at once), associated objects, key-value coding and observing,
  `NSInvocation` and message forwarding (`forwardInvocation:`, `forwardingTargetForSelector:`,
  `+resolveInstanceMethod:`), `method_setImplementation`, `class_replaceMethod`,
  `conformsToProtocol:` and protocol objects, `objc_msgSend_fpret`, and most of Foundation:
  NSFileManager, NSNotification, NSUUID, character sets, path methods, string encodings
  (`dataUsingEncoding:`), locales, time zones and run loop modes. Dictionaries look keys up by
  comparing them one by one (no hash index yet), and keep their keys in insertion order where
  macOS's order is unspecified, as sets keep their objects. Subclasses of Foundation's class clusters (a custom NSString subclass) are not
  supported. ARC's return-value handoff is not used yet, so returned objects always go
  through the autorelease pool: correct, a little slower than on a Mac. Manual
  retain/release programs use the same messages but are not covered by a test yet.
- **Most of Swift's standard library and its overlays** (see "Swift programs"), **C++'s
  runtime**, and **unwinding** beyond Objective-C's (`_Unwind_RaiseException`
  and the rest of `_Unwind_*`): a Rust panic with `panic = "unwind"` stops at the first
  unwinder call with status 127.
- **Signals, in part**: a handler's third argument (the `ucontext_t`) is null, so a handler
  cannot read or change the interrupted registers; `siglongjmp` out of a handler, `sigwait`
  and `siginterrupt` are missing.
- **Other programs, in part**: `POSIX_SPAWN_START_SUSPENDED` is refused, `popen`'s `r+` mode
  is refused, `pthread_atfork` is missing (so a `fork` while another thread holds a lock of the
  layer's can hang the child), `execvp` does not run a file without a format through the
  shell, and `posix_spawn` copies the process before it runs the program instead of sharing
  its memory as `vfork` does.
- **`scanf`**, **time zones** (all times are UTC), **locales other than C** (`setlocale`
  refuses them with a null pointer), and stdio streams shared between threads (each stream
  assumes one thread at a time).
- **`getopt_long` does not reorder words**: it stops at the first operand, as BSD's `getopt`
  does, so options after an operand stay operands; `getopt_long_only` is missing.
- **AppKit beyond its core** (D147's and D175's "Not yet"): nibs and storyboards, flipped
  views, layers, Auto Layout, scroll views, tables, images, cursors, sheets and panels (other
  than the About panel), checkboxes and
  other button types, submenus below the top bar's, the menu in the Windows and Linux profiles,
  alpha and anti-aliasing, and fonts other than Egg Sans.
- **Other libraries and frameworks**: no libc++, only the core of Foundation, CoreFoundation
  and AppKit (above) and the Cocoa umbrella over them, no `dlopen`, no dyld shared cache; a
  program that links any library other than libSystem, libobjc, Foundation, CoreFoundation,
  AppKit and Cocoa is refused. Weak libraries are allowed and their imports are missing. Imports a program makes
  from itself (C++'s weak definitions, through its export trie) are missing too.
- **Older programs**: files without chained fixups, which use `LC_DYLD_INFO` opcodes (arm64
  programs built for macOS 11, x86_64 ones built for macOS 12 or earlier), programs that start with `LC_UNIXTHREAD`, 32-bit programs, arm64e
  programs (pointer authentication) and encrypted ones are refused with a message.
- **Mach**: no ports, no messages, no `task_*` or `vm_*` calls; `mach_absolute_time` and
  `mach_timebase_info` only.
- **Pages**: arm64 macOS uses 16 KiB pages and EggOS 4 KiB ones. Segments are laid out at
  their own addresses and offsets, so every 16 KiB boundary is also a 4 KiB one and protections
  stay per segment; `getpagesize` and `sysconf(_SC_PAGESIZE)` report 4096, which a program that
  assumes 16 KiB on arm64 might not expect.
- **Writing files**: the file system is read-only, so `open` for writing fails with `EROFS`.
- **Apps from the App Store, `.dmg` and `.pkg` installers, `.app` bundles**: later parts (D17).
