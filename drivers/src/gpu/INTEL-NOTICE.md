# Notices for Intel's display engine (`drivers/src/gpu/intel*.rs`)

Register places, fields and sequences for Intel's Type-C ports (`intel_type_c.rs`, D525) and
the Type-C PHYs that follow are taken from Intel's i915 driver (`drivers/gpu/drm/i915/` in the
Linux source tree: `display/intel_tc.c`, `display/intel_display_regs.h`, `i915_reg.h`,
`display/intel_display_power_well.c`, `intel_pcode.c`), whose files Intel publishes under the
MIT licence below, separately from the rest of that tree. No code from them or from the Linux
kernel is used; EggOS's code is its own, written from them. Earlier Intel work used Intel's
public graphics programmer's reference manuals. The notice is reproduced as the licence
requires.

## Intel i915 display files

Copyright © 2019 Intel Corporation

Permission is hereby granted, free of charge, to any person obtaining a
copy of this software and associated documentation files (the "Software"),
to deal in the Software without restriction, including without limitation
the rights to use, copy, modify, merge, publish, distribute, sublicense,
and/or sell copies of the Software, and to permit persons to whom the
Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice (including the next
paragraph) shall be included in all copies or substantial portions of the
Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.  IN NO EVENT SHALL
THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS
IN THE SOFTWARE.
