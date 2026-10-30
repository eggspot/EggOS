# x86_64 programs on Arm: ahead-of-time translation

An x86_64 Windows or macOS program installed on an aarch64 machine has its code translated
once, when it is installed, into AArch64 code that is cached on disk and then runs directly on
the CPU. Nothing is interpreted and nothing is translated while the program runs (D17, D191).
On matching hardware (x86_64 programs on x86_64, arm64 programs on Arm) there is no translation
at all. Translated programs are measured and reported apart from native ones.

The translator is the crate `translate/` (`eggos-translate`, `no_std`, no third-party crates),
used by the Windows layer (`userland/windows`) and the macOS layer (`userland/macos`). It is
written from Intel's and AMD's manuals and Arm's architecture reference; no code is taken from
other translators.

Status: slice 39a built the x86-64 decoder and slice 39b the AArch64 encoder, the block
former, the flag liveness pass and the lowering of the integer, stack and control-flow
instructions; slice 39c lowered SSE and SSE2, MXCSR, `cpuid`, `rdtsc`, `xgetbv`, `lahf`, `sahf`,
`pushf`, `popf`, `cld`, `std`, `cmpxchg8b`, `cmpxchg16b` and the refused-site stubs; x87 and
strings (slice 39c2), the bridges, the cache and the install and launch integration are slices
39c2 to 39e (D191's "Not yet").

## What is translated

| Group | Instructions |
|---|---|
| Integer | `mov`, `movzx`, `movsx`, `movsxd`, `lea`, `xchg`, `add` to `cmp`, `test`, `inc`, `dec`, `neg`, `not`, `mul`, `imul` (1, 2, 3 operands), `div`, `idiv`, shifts and rotates (`shl`, `shr`, `sar`, `rol`, `ror`, `rcl`, `rcr`, `shld`, `shrd`), `bt`/`bts`/`btr`/`btc`, `bsf`, `bsr`, `tzcnt`, `lzcnt`, `popcnt`, `bswap`, `cmovcc`, `setcc`, `cbw`/`cwde`/`cdqe`, `cwd`/`cdq`/`cqo` |
| Flags | `clc`, `stc`, `cmc`, `cld`, `std`, `lahf`, `sahf`, `pushf`, `popf` |
| Stack and control | `push`, `pop`, `enter`, `leave`, `call`, `ret`, `jmp`, `jcc`, `loop`, `loope`, `loopne`, `jrcxz`, `int3`, `int` (Windows' `int 0x29` fast-fail), `ud2` |
| Atomics and order | `lock` with `add` ... `xor`, `inc`, `dec`, `not`, `neg`, `xadd`, `xchg`, `cmpxchg`, `cmpxchg8b`, `cmpxchg16b`, `bts`/`btr`/`btc`; `mfence`, `lfence`, `sfence`, `pause` |
| Strings | `movs`, `cmps`, `stos`, `lods`, `scas` with `rep`/`repe`/`repne`, `xlat` |
| SSE, SSE2 | scalar and packed single and double arithmetic, compares, conversions, moves (aligned, unaligned, half, non-temporal), logic, shuffles and unpacks, the 128-bit integer set (`paddb` ... `pxor`, shifts, `pmovmskb`, `pinsrw`, `pextrw`, `pshufd`, ...), `movd`/`movq`, `ldmxcsr`, `stmxcsr` |
| x87, minimal | `fld`/`fst`/`fstp` (32, 64, 80-bit memory and `st(i)`), `fild`/`fist`/`fistp` (16, 32, 64), `fadd`, `fsub`, `fsubr`, `fmul`, `fdiv`, `fdivr` and their `p` forms, `fxch`, `fchs`, `fabs`, `fld1`, `fldz`, `fcomi`, `fucomi` (and `p`), `fldcw`, `fnstcw`, `fnstsw`, `fnclex`, `fninit`, `fwait` |
| System | `syscall` (macOS programs that trap directly), `cpuid`, `rdtsc`, `rdtscp`, `xgetbv`, `rdrand`, `rdseed`, `endbr64`, `nop` forms, prefetches, `clflush` |

Not translated, with the reason the decoder gives (`Refusal`): VEX (AVX, AVX2, FMA, BMI2,
BMI1's `andn`/`bextr`/`blsi`...), EVEX (AVX-512), XOP, MMX and 3DNow!, SSE3, the `0F 38` and
`0F 3A` maps (SSSE3, SSE4.1, SSE4.2, AES-NI, SHA, `movbe`, `crc32`), x87 beyond the subset
(transcendentals, BCD, `fcom`, environment and state saves), `fxsave`/`xsave`, the
address-size prefix on memory operands, the operand-size prefix on near branches (Intel and
AMD disagree), and system, I/O, segment, far-transfer and transactional instructions. SSE3,
SSSE3 and SSE4.1/4.2 come next, since programs built for `x86-64-v2` (Windows 11 24H2's
baseline) may use them without checking.

The translated `cpuid` reports exactly what is translated: SSE, SSE2, CMOV, CX8, CX16, POPCNT
and LZCNT (ABM), and not BMI1, so `F3 0F BC` runs as `bsf`, as on a CPU without BMI1. No AVX,
no OSXSAVE.

**An instruction outside the list** is found at install, not at run time. Discovery follows
both sides of a program's `cpuid` checks, so real programs (the UCRT's `memcpy`, Rust's
`memchr`, game engines) contain AVX2 or SSE4.2 paths they never take on a CPU that does not
report them. The install therefore succeeds and lists, per module and per extension, how many
such sites there are; each site becomes a stub that, if it ever runs, ends the program with a
report in the log, the console and the crash dialog: the program, the module and offset, the
bytes, the missing extension, and that EggOS reports no such extension (decided by research,
D191). The install warns plainly ("this program probably will not run") only when an entry
point or an export starts in refused code. Nothing is ever skipped silently.

## Finding the code

Code is found by recursive descent from every known entry, then the bytes no path reached:

1. entries: the entry point, exports, TLS callbacks, PE `.pdata` function starts
   (`RUNTIME_FUNCTION.BeginAddress`), relocation targets inside executable sections (PE
   `.reloc`, Mach-O rebases), Mach-O `LC_FUNCTION_STARTS` and symbol tables, and RIP-relative
   `lea` targets inside executable sections found while decoding;
2. recursive descent: both sides of every conditional branch, call targets and the
   instruction after each call, jump tables recognised by their bounds check
   (`cmp`/`ja` + `jmp [base + index * 4/8]` or the `movsxd` + `add` form);
3. a linear sweep of the executable bytes left over, where the descent was ambiguous; what
   decodes is translated as extra entries, what does not is left as data.

A jump at run time to an address nothing found is a fatal, reported error (module, offset,
nearest known function); it is never translated then.

## Registers and state

| x86-64 | AArch64 |
|---|---|
| `rax`, `rcx`, `rdx`, `rbx` | `x0`, `x1`, `x2`, `x3` |
| `rsp` | `x4` (a plain register: x86's stack is 8-byte aligned, Arm's `sp` must be 16) |
| `rbp`, `rsi`, `rdi` | `x5`, `x6`, `x7` |
| `r8` to `r15` | `x8` to `x15` |
| scratch | `x16`, `x17` |
| the `gs:` base (TEB on Windows, the thread's TSD on macOS) | `x18`, reloaded from the state block after every native call |
| indirect-branch table of the current image | `x19` |
| return shadow stack | `x20` |
| guest state block (spilled flags, PF/AF source, `fs:` base, x87 stack, MXCSR) | `x21` |
| translator temporaries | `x22` to `x28`, and `v16` to `v19` and `v31` |
| `xmm0` to `xmm15` | `v0` to `v15` |
| x87 `st(0)` to `st(7)` | the state block, as doubles, with the top index |

`x29`, `x30` and `sp` stay the host's: `sp` is the native stack the bridges run on. The
shadow stack is a 64 KiB ring aligned to 64 KiB, so a deep recursion overwrites old pairs and
their returns go through the entries table. The state block holds the `fs:` base at offset 0,
the `gs:` base at 8, PF's byte at 16, AF's source byte at 17, the direction flag at 18, MXCSR at 20
(its rounding control and FZ/DAZ mirrored in FPCR by `ldmxcsr`), the x87 control word at 24,
its status word at 26, its top index at 28 and its eight registers as doubles from 32 (96
bytes in all). A refused site's stub loads the site's guest address into `x16` and executes
`brk #0xE002`; the layer's handler looks the site up and ends the program with the report.

**Blocks and exits.** A block runs from an entry to its first branch, call, return, loop or
trap. Each exit is a placeholder the image linker patches into a branch to the target's
block, or to the image's indirect dispatch (guest target in `x16`, NZCV untouched). At every
exit C holds CF itself, never its inverse.

**Flags.** CF, ZF, SF and OF live in NZCV. Each block's flag liveness is computed backwards,
so a flag result nobody reads is never computed. After a subtraction Arm's C is the inverse of
x86's borrow; the generator tracks which sense C holds and picks the matching Arm condition
instead of fixing C, using `CFINV`, `SETF8`/`SETF16` and `RMIF` (FEAT_FlagM) where the CPU has
them. PF and AF are lazy: the last result's low byte (and operands for AF) is saved in the
state block only when a later `jp`, `setp`, `lahf` or `pushf` can read it.

**Memory order.** x86 is TSO; Arm is weaker. On a CPU with a hardware TSO mode the kernel turns
it on for translated threads when it switches to them, and plain `ldr`/`str` are correct. On
every other CPU, guest loads are `ldapr` (FEAT_LRCPC) or `ldar`, and guest stores are `stlr`;
locked instructions are LSE atomics (`casal`, `ldaddal`, `swpal`, ...) or exclusive pairs
without LSE; `mfence` is `dmb ish`, `sfence` `dmb ishst`, `lfence` `dmb ishld`. SSE accesses
of 4 or 8 bytes go through a general register with the same loads and stores; 16-byte accesses
are a plain `ldr q` followed by `dmb ishld`, and `dmb ish` followed by a plain `str q`. A faster
mode that keeps provably thread-private stack accesses plain needs escape analysis and is not the
default (decided by research, D191).

## The translated image

Each translated module is one cache file: a header, the AArch64 code, and three tables:

- **entries**: guest RVA to translated offset for every block that can be reached
  indirectly (function starts, return sites, jump-table targets, exports), an open-addressed
  hash per image whose base `x19` holds; each indirect `call`/`jmp` site also keeps a
  one-entry inline cache;
- **back-map**: translated offset to guest RVA for every instruction that can fault or call
  out, so exceptions, signals, SEH unwinding (`.pdata`/`.xdata` describe guest frames) and
  crash reports see the guest's `rip`;
- **sites**: the refused instructions, with their bytes and refusal.

The guest image stays mapped where its loader puts it, readable, never executable: data, jump
tables and constants inside `.text` are read from it. `call` pushes the guest's return address
on the guest stack, as x86 does, and the (guest, host) pair on the shadow stack; `ret` pops
both and jumps straight to the host address when they match, through the entries table when
they do not (`longjmp`, exceptions, code that edits its return address).

## Calls into the layers and back

The layers are native AArch64 code. An import slot the loader fills with a layer function gets
a bridge address instead, which the entries table maps to a thunk generated at install for that
export's signature:

- **Win64 to AAPCS64**: `rcx`, `rdx`, `r8`, `r9` (`x1`, `x2`, `x8`, `x9`) and `xmm0`-`xmm3`
  move to `x0`-`x3` and `v0`-`v3`, stack arguments past the 32-byte shadow space are copied,
  `xmm6`-`xmm15` (callee-saved on Win64, only half preserved by AAPCS64) are saved around the
  call, the result comes back in `rax`/`xmm0`, and `x18` is reloaded;
- **SysV (macOS) to AAPCS64**: `rdi`, `rsi`, `rdx`, `rcx`, `r8`, `r9` and `xmm0`-`xmm7`;
  every XMM register is caller-saved there;
- **variadic functions** get a pointer to the guest's argument registers and stack, which the
  layers' variadic entries (`printf`, `execl`, ...) read in the guest's convention;
- **callbacks** (entry points, TLS callbacks, window procedures, thread starts, `atexit`,
  `qsort` comparators, exception handlers, signal handlers) go the other way: the layers call
  program code through one "call the program" function that, in a translated process, enters
  the translated code with the guest's convention and returns through a bridge.

`syscall` in a translated macOS program enters the macOS layer's system-call handling with
the guest's registers, the same path a native x86_64 macOS program's traps take.

## What cannot work

Self-modifying code and code generated at run time (JIT compilers, packers, .NET, Java and
JavaScript engines) cannot be translated ahead of time. Guest pages never become executable:
`VirtualProtect`/`VirtualAlloc` with an execute right, or `mprotect` with `PROT_EXEC`, fails
with the layer's "not supported" error and a log line naming the program. x87 arithmetic runs
in double precision, not 80-bit extended; 80-bit loads and stores convert.

## The cache

`/var/cache/eggos/translate/<key>.tx`, where the key is the SHA-256 (the `crypto` crate) of the
input file, the translator version and the code-generation mode (memory-order strategy, LSE,
LRCPC, FlagM), so a different CPU or a new translator never reuses stale code. The header
repeats the key, and a trailing checksum is checked on load; any mismatch translates again. A
file is written to a temporary name in the same directory and renamed into place. The cache is
capped at 2 GiB or 5% of the volume, whichever is smaller; the least recently launched entries
(launch touches the file's time) are removed first, never one in use, always by exact name.
Removing an app removes its own entries.

## Install and launch

`app install` (GUI, CLI and the MCP tool share it) on an aarch64 machine stops refusing x86_64
programs: it translates the program and every x86_64 module it brings, shows progress, writes
the cache files and reports the refused sites. Launching uses the cache; when an entry is
missing or stale (the program was copied in by hand, the translator changed), the launch
translates first, with progress, before any of the program runs.

## Benchmarks

Reported in their own section of the benchmark results, never mixed with native numbers:
translation speed (`translate_install_bytes`, guest bytes per second), and per kind the
translated run against the same program run natively on x86_64 and its arm64 build on the same
Arm machine: integer loop, calls and returns, indirect calls through tables, SSE scalar and
packed float, `memcpy`/`memset` through `rep movs`/`rep stos`, locked operations, and a layer
call through a bridge.
