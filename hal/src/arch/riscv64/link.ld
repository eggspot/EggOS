/*
 * RISC-V 64 kernel image for QEMU virt, loaded at its link address. OpenSBI
 * occupies the start of RAM (0x8000_0000) and jumps to 0x8020_0000.
 */
ENTRY(_start)

SECTIONS
{
    . = 0x80200000;

    .text : {
        KEEP(*(.text.boot))
        *(.text .text.*)
    }

    .rodata : ALIGN(4096) { *(.rodata .rodata.*) }

    .data : ALIGN(4096) { *(.data .data.*) }

    .bss : ALIGN(4096) {
        __bss_start = .;
        *(.bss .bss.*)
        *(COMMON)
        . = ALIGN(16);
        __bss_end = .;
    }

    /DISCARD/ : { *(.eh_frame .eh_frame_hdr .comment) }
}
