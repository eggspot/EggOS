# Running macOS programs

EggOS runs macOS console programs natively: arm64 programs on aarch64 and x86_64 programs on
x86_64. The program's own machine code runs directly on the CPU, with no emulator, no virtual
machine and no translation (D17). The macOS layer (D120), an optional component at
`/usr/lib/eggos/macos`, does what dyld does on a Mac: it reads the program's Mach-O file
(thin, or fat/universal with a slice per CPU), maps its segments, binds the functions it imports
from libSystem to its own implementations, written in Rust on EggOS's system calls, applies the
file's chained fixups and calls `main(argc, argv, envp, apple)`. A libSystem call is a plain
function call; no system call happens unless the function needs the kernel (a write, a file
read), and `malloc` allocates without one.

This is part 1: console programs that use the libSystem functions listed below. Programs on
Rust's standard library, Objective-C or Swift, and anything that needs threads, signals or
other libraries, do not run yet (see below).

## Add a program to the image

```
cargo xtask image --arch aarch64 --add path/to/hello
cargo xtask run --arch x86_64 --add path/to/hello      # build that image and boot it
```

`--add` works as for [Linux programs](linux-programs.md): the file goes to `/bin/hello`. Adding
a macOS program (a 64-bit Mach-O or fat file) to an aarch64 or x86_64 image also installs the
macOS layer; an image without one does not carry the layer at all (D18). At the prompt, type
`hello` (or its path) and its arguments; the shell reports the exit status in `$?`, and an
`abort()` as `Aborted` and `134`.

A file without code for the machine's CPU (an x86_64-only program on aarch64, an arm64e
program) is refused with a message naming the CPUs it has and status 126. On riscv64 (no macOS
program exists for it), and on images without the layer, a macOS program is refused with
`Exec format error` and status 126. A Java class file, which starts with the same bytes as a fat
file, is not taken for one.

## Build a program that runs

C, Rust or any language whose program links only libSystem, with Apple's linker and a
deployment target of macOS 13 or later, which gives chained fixups:

```
MACOSX_DEPLOYMENT_TARGET=13.0 clang -O2 -o hello hello.c
MACOSX_DEPLOYMENT_TARGET=13.0 cargo build --release --target aarch64-apple-darwin
lipo -create -output hello-universal hello-arm64 hello-x86_64      # a fat file, if wanted
```

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

## What the layer provides

| Area | Functions |
|------|-----------|
| stdio | `printf`, `fprintf`, `dprintf`, `sprintf`, `snprintf`, `asprintf`, `vprintf`, `vfprintf`, `vdprintf`, `vsprintf`, `vsnprintf`, `vasprintf`, `puts`, `fputs`, `fputc`, `putc`, `putchar`, `fwrite`, `fread`, `fgetc`, `getc`, `getchar`, `ungetc`, `fgets`, `fflush`, `fopen`, `fdopen`, `fclose`, `fileno`, `feof`, `ferror`, `clearerr`, `fseek`, `fseeko`, `ftell`, `ftello`, `rewind`, `setvbuf`, `perror`, and `__stdinp`, `__stdoutp`, `__stderrp` |
| Strings and memory | `strlen`, `strnlen`, `strcmp`, `strncmp`, `strcasecmp`, `strncasecmp`, `strcpy`, `strncpy`, `strcat`, `strncat`, `strlcpy`, `strchr`, `strrchr`, `strstr`, `strdup`, `strndup`, `strerror`, `memchr`, `memcmp`, `bcmp`, `memcpy`, `memmove`, `memset`, `bzero`, and the `_FORTIFY_SOURCE` checks `__memcpy_chk`, `__memmove_chk`, `__memset_chk`, `__strcpy_chk`, `__strncpy_chk`, `__strcat_chk`, `__sprintf_chk`, `__snprintf_chk`, `__vsnprintf_chk` |
| Heap | `malloc`, `calloc`, `realloc`, `reallocf`, `free`, `malloc_size`, `posix_memalign`, `aligned_alloc`, `valloc` |
| Numbers | `atoi`, `atol`, `atoll`, `strtol`, `strtoll`, `strtoul`, `strtoull`, `abs`, `labs`, `llabs`, `qsort`, `bsearch`, `rand`, `srand`, `arc4random`, `arc4random_buf`, `arc4random_uniform` |
| Files | `open`, `close`, `read`, `write`, `lseek`, `stat`, `lstat`, `fstat` (and their `$INODE64` names), `isatty`, `access`, `getcwd`, `chdir`, `unlink`, `rmdir`, `mkdir`, `opendir`, `readdir`, `closedir`, `dirfd` |
| Process and environment | `exit`, `_exit`, `_Exit`, `abort`, `atexit`, `raise`, `kill`, `getpid`, `getppid`, `getuid`, `geteuid`, `getenv`, `setenv`, `putenv`, `unsetenv`, `environ`, `_NSGetArgc`, `_NSGetArgv`, `_NSGetEnviron`, `_NSGetProgname`, `__error` (`errno`), `__stack_chk_guard`, `__stack_chk_fail`, `getpagesize`, `sysconf` (page size, CPUs) |
| Time | `clock_gettime`, `clock_gettime_nsec_np`, `gettimeofday`, `time`, `mach_absolute_time`, `mach_continuous_time`, `mach_timebase_info` (1/1: nanoseconds), `nanosleep`, `usleep`, `sleep` |
| dyld | `_dyld_image_count`, `_dyld_get_image_header`, `_dyld_get_image_vmaddr_slide` |

Darwin's numbers become Linux's at the boundary: `open`'s flags, error numbers (`EAGAIN` is
35, `ENOSYS` 78), clocks (`CLOCK_MONOTONIC` is 6), signals, `struct stat` and `struct dirent`.
`printf` supports the flags `- + space # 0`, `*` widths and precisions, the length modifiers
`hh h l ll q j z t L` and the conversions `d i u o x X c s p f F e E g G %`; `%n` writes nothing.
Floating-point output is rounded from the double's value scaled in double precision, so in
rare cases (a value whose decimal expansion sits next to a rounding boundary, or one above
about 10^18) its last digit differs from a Mac's, which prints the exact decimal value.

An import the layer lacks does not stop the program from starting: it is bound to a stub, and
only calling it ends the program with `eggos-macos: unimplemented libSystem function <name>`
and status 127 (as `dlsym` failures and missing weak functions surface on a Mac only when
used). An import from a library other than libSystem refuses the program before any of its code
runs: `macos: /bin/x: needs /usr/lib/libz.1.dylib, which EggOS does not provide yet`, status
126.

## What does not work yet

- **Programs on Rust's standard library, Objective-C, Swift or C++'s runtime.** A Rust std
  program imports about 80 functions: thread-local variables (`_tlv_bootstrap`, which dyld
  sets up), pthread mutexes and thread ids, `sigaction` and `sigaltstack`, `mmap`, libdispatch
  semaphores and the unwinder; it starts, then stops at the first of them with status 127. The
  next part adds them.
- **Threads** (`pthread_create` and everything else in pthread), **signal handlers**
  (`sigaction`, `signal`), **`mmap`**, **`fork`, `exec` and `posix_spawn`**, **pipes**,
  **`scanf`** and **locale** functions.
- **Other libraries and frameworks**: no libc++, no CoreFoundation, Foundation or Cocoa, no
  `dlopen`, no dyld shared cache; a program that links any library other than libSystem is
  refused. Weak libraries are allowed and their imports are missing.
- **Older programs**: files without chained fixups (built for macOS 12 or earlier, which use
  `LC_DYLD_INFO` opcodes), programs that start with `LC_UNIXTHREAD`, 32-bit programs, arm64e
  programs (pointer authentication) and encrypted ones are refused with a message.
- **Mach**: no ports, no messages, no `task_*` or `vm_*` calls; `mach_absolute_time` and
  `mach_timebase_info` only.
- **Pages**: arm64 macOS uses 16 KiB pages and EggOS 4 KiB ones. Segments are laid out at
  their own addresses and offsets, so every 16 KiB boundary is also a 4 KiB one and protections
  stay per segment; `getpagesize` and `sysconf(_SC_PAGESIZE)` report 4096, which a program that
  assumes 16 KiB on arm64 might not expect.
- **Writing files**: the file system is read-only, so `open` for writing fails with `EROFS`.
- **Apps from the App Store, `.dmg` and `.pkg` installers, `.app` bundles**: later parts (D17).
