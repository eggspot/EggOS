# The HAL's assembly: entry, exit and switch conventions

The architecture modules under `hal/src/arch/` hold the only assembly in EggOS (boot entry, trap
vectors, user-mode entry and exit, the context switch). Code carries no comments (D66), so the
register conventions those routines rely on are written down here. Each architecture's decision
(D47 for user mode, D290 LoongArch64, D313 MIPS64, the PowerPC64 and SPARC64 bring-ups) has the
design; this page is the routine-by-routine reference.

## Every architecture

- **Context switch.** The switch routine's first argument register is where the running
  thread's context is saved, its second the context to resume. A new thread's first switch
  returns to a start stub whose two callee-saved registers hold the thread's entry and its
  argument (LoongArch64 and MIPS64: `s0`, `s1`; PowerPC64: `r14`, `r15`; SPARC64: `%l0`, `%l1`,
  with its register window filled from the frame above the switch's).
- **User mode (D47).** `<arch>_enter_user(context)` keeps the kernel's callee-saved registers on
  the kernel stack and the stack pointer in the `UserContext`, points the CPU's user-exit path at
  the context, and returns to user mode with the program's registers. An exception or system
  call from user mode stores the program's registers into that context and returns from
  `<arch>_enter_user` on the kernel stack it left.
- **Test programs (`TestProgram`, D47).** Position-independent, no data; copied to a user page,
  never run where they are linked. The Linux ones use each architecture's own system-call
  numbers (generic: `write` 64, `exit_group` 94), and a callee-saved register collects the
  differences the test checks.

## LoongArch64

- While a program runs, `EENTRY` points at the user entry and `SAVE0` holds the `UserContext`;
  on the way out `EENTRY` goes back to the kernel's vector and the kernel's `$r21` (the per-CPU
  register) is restored. Entry to user mode returns to privilege level 3.
- Boot: `.bss` is zeroed through PC-relative addresses in the cached window; the EFI system
  table QEMU builds is checked by its signature and its configuration tables searched for the
  device tree (without it there is neither a device tree nor a command line); the command line
  is copied before its page is zeroed. Physical page 0 becomes the zero table that the page walk
  reaches through an empty directory entry.
- Boot page tables (D33): the high root, a level-2 table of 1 GiB leaves for the direct map's
  first 512 GiB (read, write) and one for the kernel's gigabyte at its virtual address (read,
  write, execute). From direct address mode the code turns the direct mapping windows on, moves
  to its alias in the cached window (direct address mode drops an address's high bits, so the
  alias reaches the same instructions), turns translation on and returns to the caller's alias
  (clobbers `t0`, `t1`). The TLB refill handler and the page walk are pointed at the boot
  tables: the walk's format, 4 KiB pages, the refill handler's physical address, the boot high
  root and the zero table as the low root; then every cached translation is dropped (clobbers
  `t0`).
- Secondary CPUs arrive through QEMU's boot code via mailbox 0 (D40), in direct address mode;
  mailbox 1 holds the kernel virtual address of the CPU's `CpuStart`.

## MIPS64

- From user mode the registers go into the `UserContext` that `KScratch2` holds; the return is
  in kernel mode with `Status.EXL` clear and interrupts masked, with the kernel's callee-saved
  registers and `gp`.
- `mips64_enter_user` points `KScratch2` at the context, sets `UserLocal`, `EPC`, `hi` and `lo`,
  then `Status` to user mode with interrupts enabled (`EXL` keeps both off until the `eret`).
- Linux n64 system calls report errors by setting `a3`, with the error number positive in `v0`.
- CPU set-up: kernel mode with interrupts masked, the exception base, user access to
  `UserLocal` and the counter through `rdhwr`, 4 KiB pages, read and execute inhibit, an empty
  TLB (clobbers `$12` to `$15`).
- Secondary CPUs arrive through QEMU's boot code with `a1` their `CpuStart` and `sp` the stack
  mailbox 1 held.

## PowerPC64

- From problem state the program's registers go into its context, then the code returns to
  `ppc64_enter_user`'s caller on the kernel stack it left; `ppc64_enter_user` names the context
  in the CPU's scratch area. Linux's PowerPC system calls fail when `CR0.SO` is set, with the
  error number positive in `r3`.
- The FP/vector state is 1040 bytes: 64 registers of 16 bytes, `FPSCR`, `VSCR`, `VRSAVE`
  (pointed to by `r3`).
- Boot: when not loaded where it is linked, the image copies itself there, flushes it and goes
  on there (`r27` then holds the anchor's address). The interrupt vectors go to physical
  `0x4000`, where AIL 3 takes interrupts at `0xc000...4000`. The boot radix tree's entries are
  big-endian: the root's entry 0 to the level-3 table, its entry 0 to the level-2 table, whose
  512 entries map 1 GiB each (read, write, execute). Partition table entry 0 is this tree, host
  radix, plus the process table; process table entry 0 is the kernel's tree, for quadrant 3.
  Turning translation on loads the kernel's TOC and stack and the CPU's scratch area.
- MMU set-up writes `PTCR`, `LPCR` (host radix, process tables in use, AIL 3, external
  interrupts to the hypervisor, no hypervisor decrementer) and `PIDR` 0, then drops every
  translation (`r10` the partition table's physical address; clobbers `r9`, `r10`, `r12`). The
  cache flush writes back and invalidates the `r4` bytes at physical `r3` (clobbers `r5`, `r6`).
  `HSPRG0` is the CPU's exception scratch area (found by `PIR`), holding the common entry's
  address and the kernel's TOC (clobbers `r9` to `r12`).
- Secondary CPUs are started by `OPAL_START_CPU` in hypervisor real mode, big-endian, with `r3`
  their `PIR`; `SECONDARY_STARTS[PIR]` holds the kernel virtual address of their `CpuStart`.

## SPARC64

- A trap from the kernel has `TSTATE` in `%g1`. The fault path takes the trap type in `%g1` and
  gives `%g2` the fault address (the tag access of a TLB miss, else the data fault address) and
  `%g3` the data fault status (clobbers `%g4`).
- User entry takes the context in `%o0` and the program's `%fprs` in `%o1`. Setting the MMU
  globals takes the program's root in `%o0` and the kernel's (physical; 0 keeps it) in `%o1`.

## x86_64

- Calibrating the local APIC timer needs a reference clock (the 8254 PIT, else the ACPI PM
  timer); with neither, boot stops with a panic message on the console.
