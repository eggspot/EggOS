/*
 * AArch64 kernel image for QEMU virt, loaded at its link address. RAM starts
 * at 0x4000_0000; the loader puts the device tree there, so start above it.
 */
ENTRY(_start)

SECTIONS
{
    . = 0x40080000;

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
