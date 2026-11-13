SECTIONS
{
    __eggos_vvar = . - 4096;
    . = SIZEOF_HEADERS;
    .hash : { *(.hash) }
    .gnu.hash : { *(.gnu.hash) }
    .dynsym : { *(.dynsym) }
    .dynstr : { *(.dynstr) }
    .gnu.version : { *(.gnu.version) }
    .gnu.version_d : { *(.gnu.version_d) }
    .dynamic : { *(.dynamic) }
    .rodata : { *(.rodata .rodata.*) }
    .text : { *(.text .text.*) }
    /DISCARD/ : { *(.data .data.* .bss .bss.* .comment .eh_frame .eh_frame_hdr) }
}
