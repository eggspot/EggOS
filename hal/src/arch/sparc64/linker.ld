/* EggOS kernel layout for SPARC64 on QEMU `sun4u` (D326): QEMU loads the image at its physical
   address, 4 MiB, where OpenBIOS's identity mapping lets `_start` run; `_start` locks 4 MiB TLB
   entries that map the image at KERNEL_VIRTUAL_OFFSET above it, the top 2 GiB, and jumps there.
   The trap table comes first after the boot code, 32 KiB aligned for %tba. */
KERNEL_LOAD_ADDRESS = 0x400000;
/* The same value as KERNEL_VIRTUAL_OFFSET in mod.rs (checked at the end). */
KERNEL_VIRTUAL_OFFSET = 0xffffffff80000000;

/* OpenBIOS enters at the physical address of `_start`. */
ENTRY(_start_physical)

SECTIONS
{
    . = KERNEL_LOAD_ADDRESS + KERNEL_VIRTUAL_OFFSET;
    __kernel_start = .;

    .text : AT(ADDR(.text) - KERNEL_VIRTUAL_OFFSET) {
        KEEP(*(.text.boot))
        . = ALIGN(32768);
        KEEP(*(.text.traps))
        *(.text .text.*)
    }

    .rodata : AT(ADDR(.rodata) - KERNEL_VIRTUAL_OFFSET) ALIGN(8192) {
        *(.rodata .rodata.*)
    }

    .data : AT(ADDR(.data) - KERNEL_VIRTUAL_OFFSET) ALIGN(8192) {
        *(.data .data.*)
        *(.got .got.*)
    }

    .bss (NOLOAD) : AT(ADDR(.bss) - KERNEL_VIRTUAL_OFFSET) ALIGN(8192) {
        __bss_start = .;
        *(.bss .bss.*)
        *(COMMON)
        . = ALIGN(16);
        __bss_end = .;
    }

    /* Boot stack, above a guard page the kernel leaves unmapped (D29). */
    .stack (NOLOAD) : AT(ADDR(.stack) - KERNEL_VIRTUAL_OFFSET) ALIGN(8192) {
        __stack_guard = .;
        . += 8K;
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
ASSERT(__kernel_end - __kernel_start <= 0x800000,
       "the kernel image does not fit in the two 4 MiB entries `_start` locks")
