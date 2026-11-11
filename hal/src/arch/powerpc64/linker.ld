/* EggOS kernel layout for little-endian POWER9 on QEMU `powernv9` (D304): skiboot copies the
   image to memory and enters `_start`, which moves it to its linked physical address if it is
   elsewhere. It is linked, and runs, in quadrant 3 with the direct map: physical 0x20010000
   at KERNEL_VIRTUAL_OFFSET above it, below skiboot's 0x30000000. */
KERNEL_LOAD_ADDRESS = 0x20010000;
/* The same value as KERNEL_VIRTUAL_OFFSET in mod.rs (checked at the end). */
KERNEL_VIRTUAL_OFFSET = 0xc000000000000000;

ENTRY(_start)

SECTIONS
{
    . = KERNEL_LOAD_ADDRESS + KERNEL_VIRTUAL_OFFSET;
    __kernel_start = .;

    .text : AT(ADDR(.text) - KERNEL_VIRTUAL_OFFSET) {
        KEEP(*(.text.boot))
        . = ALIGN(4096);
        KEEP(*(.text.vectors))
        . = ALIGN(4096);
        KEEP(*(.text.trap))
        *(.text .text.*)
    }

    .rodata : AT(ADDR(.rodata) - KERNEL_VIRTUAL_OFFSET) ALIGN(4096) {
        *(.rodata .rodata.*)
    }

    .data : AT(ADDR(.data) - KERNEL_VIRTUAL_OFFSET) ALIGN(4096) {
        *(.data .data.* .sdata .sdata.*)
    }

    .got : AT(ADDR(.got) - KERNEL_VIRTUAL_OFFSET) ALIGN(256) {
        *(.got .got.* .toc .toc.*)
    }

    __file_end = .;

    .bss (NOLOAD) : AT(ADDR(.bss) - KERNEL_VIRTUAL_OFFSET) ALIGN(4096) {
        __bss_start = .;
        *(.bss .bss.* .sbss .sbss.*)
        *(COMMON)
        . = ALIGN(16);
        __bss_end = .;
    }

    /* Boot stack, above a guard page the kernel leaves unmapped (D29). */
    .stack (NOLOAD) : AT(ADDR(.stack) - KERNEL_VIRTUAL_OFFSET) ALIGN(4096) {
        __stack_guard = .;
        . += 4K;
        __stack_bottom = .;
        . += 64K;
        __stack_top = .;
    }

    __kernel_end = .;

    __text_start = ADDR(.text);
    __rodata_start = ADDR(.rodata);
    __data_start = ADDR(.data);

    /DISCARD/ : {
        *(.eh_frame .eh_frame_hdr .comment .note .note.*)
    }
}

ASSERT(KERNEL_VIRTUAL_OFFSET == __kernel_virtual_offset_hal,
       "KERNEL_VIRTUAL_OFFSET differs from the HAL's (mod.rs)")
ASSERT(__kernel_end - KERNEL_VIRTUAL_OFFSET <= 0x30000000,
       "the kernel image reaches skiboot at 0x30000000")
