# Notices for Qualcomm's MDP5 (`drivers/src/gpu/qualcomm_mdp5.rs`)

The MDP5 source pipes' registers, the flush bits of control path 0 and the two block layouts
(`qualcomm_mdp5.rs`, D549) are taken from Qualcomm's Little Kernel bootloader as kept in
lk2nd: `platform/msm_shared/mdp5.c`, `platform/msm_shared/include/mdp5.h` and the
`platform/<chip>/include/platform/iomap.h` headers of the MSM8226, MSM8974, MSM8916, MSM8953,
MSM8994 and MSM8996. They are published under the BSD 3-clause licence below. EggOS's code is
its own, written from them. The notice is reproduced as the licence requires.

## Qualcomm Little Kernel

Copyright (c) 2012-2016, The Linux Foundation. All rights reserved.

Redistribution and use in source and binary forms, with or without modification, are
permitted provided that the following conditions are met:
  * Redistributions of source code must retain the above copyright notice, this list of
    conditions and the following disclaimer.
  * Redistributions in binary form must reproduce the above copyright notice, this list of
    conditions and the following disclaimer in the documentation and/or other materials
    provided with the distribution.
  * Neither the name of The Linux Foundation nor the names of its contributors may be used to
    endorse or promote products derived from this software without specific prior written
    permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND ANY EXPRESS OR
IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NON-INFRINGEMENT ARE DISCLAIMED. IN NO EVENT SHALL THE
COPYRIGHT OWNER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL,
EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE
GOODS OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND
ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING
NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED
OF THE POSSIBILITY OF SUCH DAMAGE.
