/*
 * x86_64 boot stub: PVH entry in 32-bit protected mode, switch to long mode,
 * call eggos_kernel_start(hvm_start_info physical address).
 */

/* PVH entry point note (XEN_ELFNOTE_PHYS32_ENTRY), read by the loader. */
.pushsection .note.Xen, "a", @note
    .balign 4
    .long 4                     /* name size */
    .long 8                     /* descriptor size */
    .long 18                    /* XEN_ELFNOTE_PHYS32_ENTRY */
    .asciz "Xen"
    .balign 4
    .quad _start32
.popsection

.pushsection .text.boot, "ax"
.code32
.global _start32
_start32:
    cli
    cld
    movl %ebx, %esi             /* keep hvm_start_info for the kernel */

    /* Zero .bss: nothing below may rely on it before this. */
    movl $__bss_start, %edi
    movl $__bss_end, %ecx
    subl %edi, %ecx
    xorl %eax, %eax
    rep stosb

    movl $boot_stack_top, %esp

    /* Identity-map the first 1 GiB with 2 MiB pages: PML4[0] -> PDPT[0] -> PD. */
    movl $boot_pdpt, %eax
    orl $0x3, %eax              /* present, writable */
    movl %eax, boot_pml4
    movl $boot_pd, %eax
    orl $0x3, %eax
    movl %eax, boot_pdpt
    xorl %ecx, %ecx
1:
    movl %ecx, %eax
    shll $21, %eax
    orl $0x83, %eax             /* present, writable, 2 MiB page */
    movl %eax, boot_pd(, %ecx, 8)
    incl %ecx
    cmpl $512, %ecx
    jne 1b

    /* Long mode: PAE, page tables, EFER.LME, then paging. */
    movl %cr4, %eax
    orl $(1 << 5), %eax
    movl %eax, %cr4
    movl $boot_pml4, %eax
    movl %eax, %cr3
    movl $0xc0000080, %ecx
    rdmsr
    orl $(1 << 8), %eax
    wrmsr
    movl %cr0, %eax
    orl $0x80000001, %eax       /* PG | PE */
    movl %eax, %cr0

    lgdt boot_gdt_pointer
    ljmp $0x08, $start64

.code64
start64:
    movw $0x10, %ax
    movw %ax, %ds
    movw %ax, %es
    movw %ax, %ss
    movw %ax, %fs
    movw %ax, %gs
    movq $boot_stack_top, %rsp
    movl %esi, %edi             /* boot_info, zero-extended */
    call eggos_kernel_start
2:
    cli
    hlt
    jmp 2b
.popsection

.pushsection .rodata.boot, "a"
    .balign 8
boot_gdt:
    .quad 0                     /* null descriptor */
    .quad 0x00af9a000000ffff    /* 0x08: 64-bit code, ring 0 */
    .quad 0x00cf92000000ffff    /* 0x10: data, ring 0 */
boot_gdt_end:
boot_gdt_pointer:
    .word boot_gdt_end - boot_gdt - 1
    .long boot_gdt
.popsection

.pushsection .bss.boot, "aw", @nobits
    .balign 4096
boot_pml4:
    .skip 4096
boot_pdpt:
    .skip 4096
boot_pd:
    .skip 4096
boot_stack:
    .skip 64 * 1024
boot_stack_top:
.popsection
