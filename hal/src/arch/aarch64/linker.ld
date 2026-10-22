/* EggOS kernel layout for AArch64 on QEMU `virt` (RAM starts at 0x40000000). The kernel links
   2 MiB in: QEMU puts its 1 MiB device tree at the start of RAM only if it fits below (D28). */
ENTRY(_start)

SECTIONS
{
    . = 0x40200000;
    __kernel_start = .;

    .text : {
        KEEP(*(.text.boot))
        *(.text .text.*)
    }

    .rodata : ALIGN(4096) {
        *(.rodata .rodata.*)
    }

    .data : ALIGN(4096) {
        *(.data .data.*)
    }

    .bss (NOLOAD) : ALIGN(16) {
        __bss_start = .;
        *(.bss .bss.*)
        *(COMMON)
        . = ALIGN(16);
        __bss_end = .;
    }

    /* Boot stack. No guard page until paging (M2): an overflow runs into .bss. */
    .stack (NOLOAD) : ALIGN(4096) {
        . += 64K;
        __stack_top = .;
    }

    __kernel_end = .;

    /DISCARD/ : {
        *(.eh_frame .eh_frame_hdr .comment .note .note.*)
    }
}
