/* EggOS kernel layout for RISC-V 64 on QEMU `virt`: OpenSBI occupies 0x80000000 and jumps to 0x80200000. */
ENTRY(_start)

SECTIONS
{
    . = 0x80200000;
    __kernel_start = .;

    .text : {
        KEEP(*(.text.boot))
        *(.text .text.*)
    }

    .rodata : ALIGN(4096) {
        *(.rodata .rodata.* .srodata .srodata.*)
    }

    .data : ALIGN(4096) {
        *(.data .data.* .sdata .sdata.*)
    }

    .bss (NOLOAD) : ALIGN(16) {
        __bss_start = .;
        *(.sbss .sbss.* .bss .bss.*)
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
