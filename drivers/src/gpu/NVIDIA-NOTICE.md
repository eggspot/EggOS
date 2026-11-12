# Notices for NVIDIA's Falcon engines and FWSEC (`drivers/src/gpu/nvidia_falcon.rs`, `nvidia_fwsec.rs`)

The register places and fields `nvidia_falcon.rs` uses (D497) are taken from NVIDIA's published
register manuals in its open GPU kernel modules (`src/common/inc/swref/published/`:
`dev_falcon_v4.h`, `dev_riscv_pri.h`, `dev_gsp.h` and `dev_sec_pri.h` for Turing and Ampere).
The video BIOS layouts `nvidia_fwsec.rs` reads and the FWSEC command it prepares (D512) follow
the same modules' `src/nvidia/src/kernel/gpu/vbios/kern_vbios.c`,
`arch/turing/kern_vbios_tu102.c` and `src/nvidia/inc/kernel/platform/pci_exp_table.h`. NVIDIA
publishes these files under the MIT licence below; EggOS's code is its own, written from them.
The notice is reproduced as the licence requires.

## NVIDIA register manuals

SPDX-FileCopyrightText: Copyright (c) 2003-2022 NVIDIA CORPORATION & AFFILIATES
SPDX-License-Identifier: MIT

Permission is hereby granted, free of charge, to any person obtaining a
copy of this software and associated documentation files (the "Software"),
to deal in the Software without restriction, including without limitation
the rights to use, copy, modify, merge, publish, distribute, sublicense,
and/or sell copies of the Software, and to permit persons to whom the
Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in
all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL
THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.

## GSP-RM's boot and queues (`nvidia_gsp_boot.rs`, D522; `nvidia_gsp_queue.rs`, D523)

The WPR2 layout, the WPR metadata, radix3, libos's memory regions, the RM arguments and the
queue headers follow NVIDIA's open GPU kernel modules (`kernel_gsp_tu102.c`,
`gsp_fw_wpr_meta.h`, `gsp_fw_heap.h`, `gsp_init_args.h`, `libos_init_args.h`,
`rmRiscvUcode.h`) for the r535 ABI, under the MIT licence above. The firmware files' headers
(`nvfw_bin_hdr`, `nvfw_hs_header_v2`, `nvfw_hs_load_header_v2`) and how the booter's signature is
chosen by the fuse are as nouveau's `include/nvfw/fw.h`, `include/nvfw/hs.h`,
`nvkm/subdev/gsp/ga102.c` and `nvkm/falcon/ga100.c` describe them; those files carry the MIT
licence, copyright Red Hat Inc. and NVIDIA Corporation, with the same terms as above. No other
code from them or from the Linux kernel is used; EggOS's code is its own, written from them.

The queue element and remote call layouts, the function and event numbers, the system
information, registry and CPU sequencer formats (D523) follow the r535 headers of NVIDIA's open
GPU kernel modules (`rpc_global_enums.h`, `rpc_headers.h`, `message_queue_priv.h`) as nouveau's
MIT-licensed `nvkm/subdev/gsp/rm/r535/` files (copyright Red Hat Inc. and NVIDIA Corporation)
use them.

The remote alloc and control call layouts, the object classes and the display control
commands and parameters `nvidia_gsp_display.rs` uses (D524) are from the same modules' r535
headers (`g_rpc-structures.h`, `ctrl0073*.h`, `cl0000.h`, `cl0080.h`), under the MIT licence
above.
