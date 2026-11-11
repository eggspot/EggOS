/* EggOS kernel layout for s390x on QEMU `s390-ccw-virtio` (D321): QEMU loads the image at its
   physical address, 1 MiB, above the lowcore and its boot information, and starts it there in
   real mode. It runs at KERNEL_VIRTUAL_OFFSET above it, the top 2 GiB of the 53-bit space. */
KERNEL_LOAD_ADDRESS = 0x100000;
/* The same value as KERNEL_VIRTUAL_OFFSET in mod.rs (checked at the end). */
KERNEL_VIRTUAL_OFFSET = 0x001fffff80000000;

/* QEMU enters at the physical address of `_start`. */
ENTRY(_start_physical)

SECTIONS
{
    . = KERNEL_LOAD_ADDRESS + KERNEL_VIRTUAL_OFFSET;
    __kernel_start = .;

    .text : AT(ADDR(.text) - KERNEL_VIRTUAL_OFFSET) {
        KEEP(*(.text.boot))
        . = ALIGN(4096);
        KEEP(*(.text.interrupts))
        KEEP(*(.text.trap))
        *(.text .text.*)
    }

    .rodata : AT(ADDR(.rodata) - KERNEL_VIRTUAL_OFFSET) ALIGN(4096) {
        *(.rodata .rodata.*)
    }

    .data : AT(ADDR(.data) - KERNEL_VIRTUAL_OFFSET) ALIGN(4096) {
        *(.data .data.* .sdata .sdata.*)
        *(.got .got.*)
    }

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
    _start_physical = _start - KERNEL_VIRTUAL_OFFSET;

    /DISCARD/ : {
        *(.eh_frame .eh_frame_hdr .comment .note .note.*)
    }
}

ASSERT(KERNEL_VIRTUAL_OFFSET == __kernel_virtual_offset_hal,
       "KERNEL_VIRTUAL_OFFSET differs from the HAL's (mod.rs)")
ASSERT(__kernel_end - KERNEL_VIRTUAL_OFFSET <= 0x40000000,
       "the kernel image does not fit in the first gigabyte")
