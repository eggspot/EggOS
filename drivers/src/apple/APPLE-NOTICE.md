# Notices for Apple silicon's coprocessors (`drivers/src/apple/`)

The ASC mailbox's register places and message layout (`mailbox.rs`, D543) and RTKit's
management messages, endpoints, power states and system-endpoint buffer requests (`rtkit.rs`,
D543) are taken from the Asahi Linux contributors' drivers in the Linux kernel,
`drivers/soc/apple/mailbox.c`, `mailbox.h`, `rtkit.c` and `rtkit-internal.h`. They are
published under `GPL-2.0-only OR MIT`; EggOS uses them under the MIT licence below, as facts
about the hardware and its firmware's protocol. EggOS's code is its own, written from them.
The notice is reproduced as the licence requires.

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
