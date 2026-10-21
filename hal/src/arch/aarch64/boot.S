/*
 * AArch64 boot stub: park secondary CPUs, zero .bss, set the stack,
 * call eggos_kernel_start(x0 from the loader).
 */

.pushsection .text.boot, "ax"
.global _start
_start:
    mrs x1, mpidr_el1
    and x1, x1, #0xff
    cbnz x1, 2f                 /* only CPU 0 boots; the rest wait */
    mov x19, x0

    adrp x1, __bss_start
    add x1, x1, :lo12:__bss_start
    adrp x2, __bss_end
    add x2, x2, :lo12:__bss_end
1:
    cmp x1, x2
    b.hs 3f
    str xzr, [x1], #8
    b 1b
3:
    adrp x1, boot_stack_top
    add x1, x1, :lo12:boot_stack_top
    mov sp, x1

    mov x0, x19
    bl eggos_kernel_start
2:
    wfe
    b 2b
.popsection

.pushsection .bss.boot, "aw", @nobits
    .balign 16
boot_stack:
    .skip 64 * 1024
boot_stack_top:
.popsection
