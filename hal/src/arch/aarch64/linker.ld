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
