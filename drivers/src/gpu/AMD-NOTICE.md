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
