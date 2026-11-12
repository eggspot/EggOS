KERNEL_LOAD_ADDRESS = 0x40400000;
KERNEL_VIRTUAL_OFFSET = 0x80000000;

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

    .bss (NOLOAD) : AT(ADDR(.bss) - KERNEL_VIRTUAL_OFFSET) ALIGN(4096) {
        __bss_start = .;
        *(.bss .bss.*)
        . = ALIGN(16);
        __bss_end = .;
    }

    .stack (NOLOAD) : AT(ADDR(.stack) - KERNEL_VIRTUAL_OFFSET) ALIGN(4096) {
        __stack_guard = .;
        . += 4K;
        __stack_bottom = .;
        . += 64K;
        __stack_top = .;
    }

    __kernel_end = .;
    _start_physical = _start - KERNEL_VIRTUAL_OFFSET;

    /DISCARD/ : {
        *(.eh_frame .eh_frame_hdr .ARM.exidx .ARM.extab .comment .note .note.*)
    }
}

ASSERT((__kernel_start >> 30) == ((__kernel_end - 1) >> 30),
       "the kernel image crosses a 1 GiB block")
