# Notices for Apple silicon's coprocessors (`drivers/src/apple/`)

The ASC mailbox's register places and message layout (`mailbox.rs`, D543) and RTKit's
management messages, endpoints, power states and system-endpoint buffer requests (`rtkit.rs`,
D543) are taken from the Asahi Linux contributors' drivers in the Linux kernel,
`drivers/soc/apple/mailbox.c`, `mailbox.h`, `rtkit.c` and `rtkit-internal.h`. They are
published under `GPL-2.0-only OR MIT`; EggOS uses them under the MIT licence below, as facts
about the hardware and its firmware's protocol. EggOS's code is its own, written from them.
The notice is reproduced as the licence requires.

The DART's registers, table layout and entry formats (`dart.rs`, D544) are taken from m1n1's
`src/dart.c` as released in m1n1 1.9.9, the last release under the MIT licence (m1n1 moved
to the GPL from 2.0.0 on; nothing later is used). So are AFK and EPIC's messages, ring
layout and command format (`epic.rs`, D545, from `src/afk.c` and `afk.h`) and the binary
property list format (`properties.rs`, D545, from `src/dcp/parser.c`, Copyright 2021 Alyssa
Rosenzweig, published under `GPL-2.0-only OR MIT` and used under MIT). Its notice follows the
first.

## Apple mailbox and RTKit drivers

Copyright (C) The Asahi Linux Contributors

Permission is hereby granted, free of charge, to any person obtaining a copy of this software
and associated documentation files (the "Software"), to deal in the Software without
restriction, including without limitation the rights to use, copy, modify, merge, publish,
distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the
Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or
substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING
BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND
NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM,
DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.

## m1n1 1.9.9

MIT License

Copyright The Asahi Linux Contributors

Permission is hereby granted, free of charge, to any person obtaining a copy of this software
and associated documentation files (the "Software"), to deal in the Software without
restriction, including without limitation the rights to use, copy, modify, merge, publish,
distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the
Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or
substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING
BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND
NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM,
DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
