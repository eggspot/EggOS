/* EggOS kernel layout for x86_64 PCs: loaded at 1 MiB by a PVH loader. It is linked, and runs,
   in the top 2 GiB (D33): KERNEL_VIRTUAL_OFFSET above its load address. */
KERNEL_LOAD_ADDRESS = 0x100000;
/* The same value as KERNEL_VIRTUAL_OFFSET in mod.rs (checked at the end). */
KERNEL_VIRTUAL_OFFSET = 0xffffffff80000000;

/* The PVH note names the physical entry point; the ELF entry is the same address. */
ENTRY(_start32_physical)

SECTIONS
{
    . = KERNEL_LOAD_ADDRESS + KERNEL_VIRTUAL_OFFSET;
    __kernel_start = .;

    /* The PVH entry note: kept, and first, so the loader finds it in a PT_NOTE segment. */
    .note.Xen : AT(ADDR(.note.Xen) - KERNEL_VIRTUAL_OFFSET) {
        KEEP(*(.note.Xen))
    }

    .text : AT(ADDR(.text) - KERNEL_VIRTUAL_OFFSET) ALIGN(4096) {
        KEEP(*(.text.boot))
        *(.text .text.*)
    }

    .rodata : AT(ADDR(.rodata) - KERNEL_VIRTUAL_OFFSET) ALIGN(4096) {
        *(.rodata .rodata.*)
    }

    .data : AT(ADDR(.data) - KERNEL_VIRTUAL_OFFSET) ALIGN(4096) {
        *(.data .data.*)
        /* Static linking leaves the GOT empty or tiny; keep it inside .data. */
        *(.got .got.*)
    }

    .bss (NOLOAD) : AT(ADDR(.bss) - KERNEL_VIRTUAL_OFFSET) ALIGN(4096) {
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
    _start32_physical = _start32 - KERNEL_VIRTUAL_OFFSET;

    /DISCARD/ : {
        *(.eh_frame .eh_frame_hdr .comment .note .note.*)
    }
}

ASSERT(KERNEL_VIRTUAL_OFFSET == __kernel_virtual_offset_hal,
       "KERNEL_VIRTUAL_OFFSET differs from the HAL's (mod.rs)")
/* The boot page tables map the kernel's virtual addresses onto the first gigabyte. */
ASSERT(__kernel_end - KERNEL_VIRTUAL_OFFSET <= 0x40000000,
       "the kernel image does not fit in the first gigabyte")
