/*
 * RISC-V 64 boot stub: zero .bss, set the stack,
 * call eggos_kernel_start(device tree address from a1).
 */

.pushsection .text.boot, "ax"
.global _start
_start:
    mv s0, a1

    la t0, __bss_start
    la t1, __bss_end
1:
    bgeu t0, t1, 2f
    sd zero, 0(t0)
    addi t0, t0, 8
    j 1b
2:
    la sp, boot_stack_top

    mv a0, s0
    call eggos_kernel_start
3:
    wfi
    j 3b
.popsection

.pushsection .bss.boot, "aw", @nobits
    .balign 16
boot_stack:
    .skip 64 * 1024
boot_stack_top:
.popsection
