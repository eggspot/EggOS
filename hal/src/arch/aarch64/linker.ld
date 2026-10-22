/* EggOS kernel layout for AArch64 on QEMU `virt` (RAM starts at 0x40000000). The kernel loads
   2 MiB in: QEMU puts its 1 MiB device tree at the start of RAM only if it fits below (D28). It
   is linked, and runs, in the top 2 GiB (D33): KERNEL_VIRTUAL_OFFSET above its load address. */
KERNEL_LOAD_ADDRESS = 0x40200000;
/* The same value as KERNEL_VIRTUAL_OFFSET in mod.rs (checked at the end). */
KERNEL_VIRTUAL_OFFSET = 0xffffffff40000000;

/* The loader enters at the physical address of `_start`. */
ENTRY(_start_physical)

SECTIONS
{
    . = KERNEL_LOAD_ADDRESS + KERNEL_VIRTUAL_OFFSET;
    __kernel_start = .;

    .text : AT(ADDR(.text) - KERNEL_VIRTUAL_OFFSET) {
        KEEP(*(.text.boot))
        *(.text .text.*)
    }

    .rodata : AT(ADDR(.rodata) - KERNEL_VIRTUAL_OFFSET) ALIGN(4096) {
        *(.rodata .rodata.*)
    }

    .data : AT(ADDR(.data) - KERNEL_VIRTUAL_OFFSET) ALIGN(4096) {
        *(.data .data.*)
    }

    .bss (NOLOAD) : AT(ADDR(.bss) - KERNEL_VIRTUAL_OFFSET) ALIGN(16) {
        __bss_start = .;
        *(.bss .bss.*)
        *(COMMON)
        . = ALIGN(16);
        __bss_end = .;
    }

    /* Boot stack, above a guard page the kernel leaves unmapped (D29), so an overflow faults
       instead of running into .bss. */
    .stack (NOLOAD) : AT(ADDR(.stack) - KERNEL_VIRTUAL_OFFSET) ALIGN(4096) {
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
    _start_physical = _start - KERNEL_VIRTUAL_OFFSET;

    /DISCARD/ : {
        *(.eh_frame .eh_frame_hdr .comment .note .note.*)
    }
}

ASSERT(KERNEL_VIRTUAL_OFFSET == __kernel_virtual_offset_hal,
       "KERNEL_VIRTUAL_OFFSET differs from the HAL's (mod.rs)")
/* The boot page tables map the kernel's gigabyte with one block. */
ASSERT((__kernel_start >> 30) == ((__kernel_end - 1) >> 30),
       "the kernel image crosses a 1 GiB boundary")
