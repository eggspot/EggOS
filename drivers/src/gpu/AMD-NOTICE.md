# Notices for AMD's display engine (`drivers/src/gpu/amd_dcn.rs`)

The register places and fields `amd_dcn.rs` uses (D496) are taken from AMD's published register
headers for the display core, `dcn_3_0_0` to `dcn_3_5_1` `_offset.h` and `_sh_mask.h`
(`drivers/gpu/drm/amd/include/asic_reg/dcn/` in the Linux source tree), which AMD publishes under
the MIT licence below, separately from the rest of that tree. No other code from them or from the
Linux kernel is used. The notice is reproduced as the licence requires.

## AMD register headers

Copyright (C) 2020  Advanced Micro Devices, Inc.

Permission is hereby granted, free of charge, to any person obtaining a
copy of this software and associated documentation files (the "Software"),
to deal in the Software without restriction, including without limitation
the rights to use, copy, modify, merge, publish, distribute, sublicense,
and/or sell copies of the Software, and to permit persons to whom the
Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included
in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS
OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.  IN NO EVENT SHALL
THE COPYRIGHT HOLDER(S) BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN
AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN
CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.

## AMD's DMUB service

`amd_dmcub.rs` (D514) loads the display microcontroller as AMD's DMUB service does
(`drivers/gpu/drm/amd/display/dmub/src/dmub_srv.c` and `dmub_dcn31.c` in the Linux source tree,
which AMD publishes under the MIT licence above, copyright 2019 and 2020 Advanced Micro Devices,
Inc.): the same windows, regions and order of register writes, written in EggOS's own code.

## AMD's display core (DC)

`amd_dcn.rs`'s mode setting (D521) follows the order and the field meanings of AMD's display
core (`drivers/gpu/drm/amd/display/dc/` in the Linux source tree, which AMD publishes under the
MIT licence above): the timing generator's totals, blanks and syncs (`optc1_program_timing`),
the DisplayPort stream attributes (`enc1_stream_encoder_dp_set_stream_attribute`), the DTO's
phase and modulo, the scaler's blank positions and the hub's request timing fields as the
display mode library defines them. They are written in EggOS's own code.
