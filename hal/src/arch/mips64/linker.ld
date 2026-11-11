/* EggOS kernel layout for MIPS64 on QEMU `loongson3-virt` (D313): QEMU loads the image at its
   physical address, 2 MiB, and enters it at its CKSEG0 address, where it is linked and runs:
   KERNEL_VIRTUAL_OFFSET above its load address. The exception vectors come first after the
   boot code, 4 KiB aligned for EBase. */
KERNEL_LOAD_ADDRESS = 0x200000;
/* The same value as KERNEL_VIRTUAL_OFFSET in mod.rs (checked at the end). */
KERNEL_VIRTUAL_OFFSET = 0xffffffff80000000;

/* QEMU enters at `_start`'s CKSEG0 address. */
ENTRY(_start)

SECTIONS
{
    . = KERNEL_LOAD_ADDRESS + KERNEL_VIRTUAL_OFFSET;
    __kernel_start = .;

    .text : AT(ADDR(.text) - KERNEL_VIRTUAL_OFFSET) {
        KEEP(*(.text.boot))
        . = ALIGN(4096);
        KEEP(*(.text.vectors))
        *(.text .text.*)
    }

    .rodata : AT(ADDR(.rodata) - KERNEL_VIRTUAL_OFFSET) ALIGN(4096) {
        *(.rodata .rodata.*)
    }

    .data : AT(ADDR(.data) - KERNEL_VIRTUAL_OFFSET) ALIGN(4096) {
        *(.data .data.* .sdata .sdata.*)
        *(.MIPS.abiflags)
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

    /DISCARD/ : {
        *(.eh_frame .eh_frame_hdr .comment .note .note.* .MIPS.options .reginfo .pdr)
    }
}

ASSERT(KERNEL_VIRTUAL_OFFSET == __kernel_virtual_offset_hal,
       "KERNEL_VIRTUAL_OFFSET differs from the HAL's (mod.rs)")
ASSERT(__kernel_end - KERNEL_VIRTUAL_OFFSET <= 0x10000000,
       "the kernel image does not fit in the low 256 MiB that CKSEG0 and QEMU's RAM share")
