/* EggOS kernel layout for x86_64 PCs: loaded at its link address (1 MiB) by a PVH loader. */
ENTRY(_start32)

SECTIONS
{
    . = 0x100000;
    __kernel_start = .;

    /* The PVH entry note: kept, and first, so the loader finds it in a PT_NOTE segment. */
    .note.Xen : {
        KEEP(*(.note.Xen))
    }

    .text : ALIGN(4096) {
        KEEP(*(.text.boot))
        *(.text .text.*)
    }

    .rodata : ALIGN(4096) {
        *(.rodata .rodata.*)
    }

    .data : ALIGN(4096) {
        *(.data .data.*)
        /* Static linking leaves the GOT empty or tiny; keep it inside .data. */
        *(.got .got.*)
    }

    .bss (NOLOAD) : ALIGN(4096) {
        __bss_start = .;
        *(.bss .bss.*)
        *(COMMON)
        . = ALIGN(16);
        __bss_end = .;
    }

    /* Boot stack, above a guard page the kernel leaves unmapped (D29), so an overflow faults
       instead of running into .bss. */
    .stack (NOLOAD) : ALIGN(4096) {
        __stack_guard = .;
        . += 4K;
        __stack_bottom = .;
        . += 64K;
        __stack_top = .;
    }

    __kernel_end = .;

    /* Where each part of the image starts, for the kernel page tables' permissions (D29). */
    __text_start = ADDR(.text);
    __rodata_start = ADDR(.rodata);
    __data_start = ADDR(.data);

    /DISCARD/ : {
        *(.eh_frame .eh_frame_hdr .comment .note .note.*)
    }
}
