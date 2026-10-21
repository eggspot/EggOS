/* x86_64 kernel image, loaded at its link address (1 MiB) by a PVH loader. */
ENTRY(_start32)

SECTIONS
{
    . = 0x100000;

    .note.Xen : { KEEP(*(.note.Xen)) }

    .text : ALIGN(4096) {
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
