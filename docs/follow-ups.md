# EggOS follow-ups

Every open follow-up in the repo, by milestone (owner, 2026-11-13: code for everything, not only
the roadmap rows). Sources: the "Not yet" lists in `docs/decisions.md`, the "Follow-ups" notes in
`docs/roadmap.md` and the open parts of its in-progress rows. Items a later decision already did are
left out; each line names its source decisions. A line is ticked when its code is written, with the
decision that wrote it ("code written, unbuilt" until the build-and-fix pass, then "built, untested
on hardware" where that applies). Building and running everything is the build-and-fix pass's work
and is not listed per item.

## M4 Scheduler, processes, threads and memory

- [x] Double-free detection for single frames, debug-only, with the per-CPU frame caches (D28, D31) — code written, unbuilt, D551
- [x] Buddy allocator: free runs that straddle a range's untouched part and the free stack; one bitmap per range; anti-fragmentation grouping (migrate types); a high watermark on the free stack giving frames back (D31) — code written, unbuilt, D551
- [x] Per-CPU frame and heap caches instead of one heap lock (D30, D31) — code written, unbuilt, D551
- [x] Empty heap slabs given back to the frame allocator (D30) — code written, unbuilt, D551
- [x] 1 GiB pages on x86_64 (CPUID) for the direct map (D29) — code written, unbuilt, D552
- [x] Svpbmt on RISC-V for device and non-cacheable mappings (D29) — code written, unbuilt, D552
- [x] Global kernel mappings and PCID/ASIDs (D33, D45) — code written, unbuilt, D552
- [x] A new thread starts with its creator's FP/SIMD control state (rounding mode, exception masks), as Linux copies it (D91) — already done (`inherit_fp` in `clone`, D91's follow-up)
- [x] Launchers wait for the process, not its first thread (process-wide completion), and a test of a `CLONE_CHILD_CLEARTID` waiter (D92) — code written, unbuilt, D553
- [x] Robust-list walk on every way a thread ends once memory is shared between processes, with compare-and-exchange on user memory (D93) — code written, unbuilt, D553
- [x] TLB shootdown only to CPUs running the process, ranged, and by broadcast invalidation (AArch64 `tlbi ...is`, RISC-V SBI remote fence / Svinval) (D94) — code written, unbuilt, D553 (ranged and broadcast; CPUs that keep the space loaded are still asked, D295)
- [x] Signals interrupt only the target thread's CPU (threads record their CPU) and `send_to_process` interrupts after dropping the lock (D95, D97) — code written, unbuilt, D295 and D553
- [x] `execve` and `fork` record the new first thread's wait slot in the signal block (D97) — already done (`wait::record_running` in fork and exec, D105)
- [x] Per-thread CPU clock ids for the calling thread and other processes, children's times in `getrusage`, `times` and `wait4` (D198) — code written, unbuilt, D553
- [ ] System time apart from user time; another thread's CPU clock; another thread's or process's affinity (D198)
- [ ] Per-CPU run-queue locks (a lock-free local run queue with per-CPU wake lists) and the invariant-TSC check (D40-D45, D198)
- [x] RISC-V Svinval for ranged TLB invalidation where harts have it (D94, D261) — code written, unbuilt, D552
- [x] Random seeds from the CPU: RNDR on AArch64 and the Zkr `seed` CSR on RISC-V, reseeding on the virtio-rng schedule, and vmgenid reseeding (D56, D282) — code written, unbuilt, D553

## M5 Driver model, interrupts and ACPI

- [x] Unmapping device windows and unbinding drivers, with cross-CPU TLB shootdown of kernel mappings (D29, D110) — code written, unbuilt, D554 (windows; unbinding bound drivers stays)
- [x] On x86_64, map ACPI tables on demand and reserved PVH ranges uncached instead of the cached direct map (D29) — code written, unbuilt, D554
- [x] Console input taken on any CPU, not only the boot CPU (D98) — code written, unbuilt, D554 (x86_64)
- [x] Device-tree `dma-ranges` for devices whose DMA addresses differ from the CPU's (D269) — code written, unbuilt, D554
- [x] Plain MSI (without MSI-X) steered to other CPUs; moving lines off CPUs that go offline; balancing by load (D536) — code written, unbuilt, D555 (plain MSI; offline CPUs and load balancing stay open below)
- [x] RISC-V AIA: the APLIC's direct mode, PCI MSIs straight into the IMSIC, interrupts on harts other than the boot hart (D532) — code written, unbuilt, D555
- [ ] x86_64: interrupt remapping (VT-d / AMD-Vi) so x2APIC destinations above 255 work, and the HPET as a timer of its own (D531, D536)
- [x] AML: loading tables from AML (`Load`, `LoadTable`), the global lock's hardware handshake, real waits on mutexes and events, GPIO and serial-bus connection fields (D537, D538) — code written, unbuilt, D556 (real waits and GPIO/serial fields stay open below)
- [ ] AML: real waits on mutexes and events with several AML threads; GPIO and serial-bus connection fields (D537)
- [ ] Moving device lines off CPUs that go offline, and balancing lines by load (D536)
- [ ] The power state's front-ends: `power` CLI, MCP `power_status`, a battery indicator on the bar; acting on the lid and critical temperatures (D556)
- [x] ACPI: the ECDT, EC burst mode, GPE block devices (`ACPI0006`), acting on lid, battery, thermal and dock notifications (D539) — code written, unbuilt, D556
- [ ] ACPI sleep states other than S5 (S3 suspend to RAM with wake events) (D538, D539)

## M6 Linux system-call ABI

- [ ] Line discipline: `VREPRINT`, `VDISCARD`, `IXON` flow control, output flags the UART obeys, tab erase from the output's column (column tracking), a long echo written outside the lock (D100)
- [ ] `noexec` mounts (D103)
- [x] `umask` per process (the `umask` call, inherited by `fork`, applied by every create) instead of the fixed 022 (D125) — code written, unbuilt, D557
- [x] `sendmmsg` and `recvmmsg` (D138) — code written, unbuilt, D557
- [ ] An out-of-memory killer that ends the largest program, and a second reserve tier for an ending program (D155)
- [ ] `RLIMIT_AS` and `RLIMIT_DATA` enforced by `brk` and `mmap` (D53, D155)
- [ ] Netlink sockets (`NETLINK_ROUTE`: links, addresses, routes) so iproute2 and musl's `getifaddrs` work (D157)
- [ ] `TCP_INFO` and `SO_LINGER`'s timeout (D158)
- [ ] A vDSO (`AT_SYSINFO_EHDR`): `clock_gettime`, `gettimeofday`, `time`, `getcpu` without a system call, with the counter readable from user mode (D51, D73, D167)
- [ ] `ETXTBSY` for writes to a running program's file (D167)
- [ ] `/proc/<pid>/mountinfo`, `/proc/thread-self` and `/proc/<pid>/task/<tid>` directories (D168, D171)
- [ ] `/sys`: `/sys/devices/system/cpu/{online,possible,present}`, `/sys/kernel/mm/transparent_hugepage/enabled`, and `/proc/sys/vm/overcommit_memory` (D168)
- [ ] `F_SETLEASE`/`F_GETLEASE` and `F_NOTIFY` (D171)
- [x] `copy_file_range`, `splice`, `tee` and `sendfile` (D176) — code written, unbuilt, D557
- [x] Extended attributes: `setxattr`, `getxattr`, `listxattr`, `removexattr` and their `l`/`f` forms (D176) — code written, unbuilt, D557
- [x] `prctl` `PR_SET_NAME`/`PR_GET_NAME` with `/proc/<pid>/comm` and `/proc/<pid>/task/<tid>/comm` (D176) — code written, unbuilt, D557
- [x] `chown`, `fchown`, `lchown`, `fchownat` (D176) — code written, unbuilt, D557
- [ ] `fanotify` (D179)
- [x] `pidfd_open`, `pidfd_send_signal` and `pidfd_getfd` (D183) — code written, unbuilt, D557
- [ ] Address-space layout randomization of the `mmap` area, the break and the interpreter's base (D51, D53, D201)
- [x] POSIX capabilities: `capget`/`capset`, the bounding and ambient sets, `PR_CAPBSET_*`, file capabilities not yet (D270) — code written, unbuilt, D557
- [ ] RISC-V misaligned loads and stores emulated as Linux does on harts that trap them (D65)
- [ ] `MAP_GROWSDOWN` and `MAP_POPULATE` (D53, D167)
- [ ] `ptrace` for debuggers: `PTRACE_TRACEME`, `ATTACH`/`SEIZE`, `PEEK`/`POKE`, `GETREGS`/`SETREGS`, `CONT`, `SINGLESTEP` with `TRAP_TRACE`, hardware breakpoints with `TRAP_HWBKPT`, and the `PTRACE_MODE_READ` check on `/proc/<pid>` files (D65, D168)

## M7 VFS and file systems

- [ ] EggFS: a backup copy of the superblock used by the checker to repair a damaged one (D192)
- [ ] EggFS: an on-disk name index (version 3) for fast first lookups and hashed `readdir` order (D192)
- [ ] Block devices as `S_IFBLK` nodes with `BLKGETSIZE64`, `BLKSSZGET` and `BLKFLSBUF` for Linux tools (D192)
- [ ] A mounted volume's label in `/proc/driver/disks` (D192)
- [ ] `utimensat` setting the access time, and the `noatime`/`strictatime` mount options (D534)

## M8 virtio and the network stack

- [ ] virtio: event index (`VIRTIO_F_EVENT_IDX`) and indirect descriptors (D111)
- [ ] virtio: used ring checked against the chains in flight, so a faulty device cannot return a descriptor twice (D111)
- [ ] virtio-iommu, cache maintenance for non-coherent DMA, and the doorbell's ordering for real devices (`dmb osh`, `fence w,o`) (D111)
- [ ] virtio-gpu: more than one scanout, display hot-unplug and teardown (D112)
- [ ] virtio-input: keyboard LEDs through the status queue, more than four input devices, hot-plug (D113)
- [ ] Network: IPv6 (addresses, NDP, SLAAC, ICMPv6, UDP and TCP over IPv6, `AF_INET6` sockets) (D157)
- [ ] TCP: window scaling, selective acknowledgments, keep-alive probes, urgent data (D158)
- [ ] virtio multiqueue: virtio-net's several queue pairs and virtio-blk's `num_queues`, each following its CPU (D540)
- [ ] virtio-net: IPv6 and UDP segmentation offload and receive-side coalescing (`GUEST_TSO4`, mergeable buffers) (D541)

## M9 More architectures

- [ ] RISC-V: the vector extension (V) for programs, with its first-use trap and per-thread state, and in signal frames (D54, D261)
- [ ] RV32: secondary harts (`start_cpu`), console input and device interrupts through the PLIC, Sstc (D481)
- [ ] 32-bit Arm: secondary CPUs through PSCI `CPU_ON`, console input and device interrupts through the GIC, ASIDs, PAN (D482)
- [ ] RV32 and 32-bit Arm: RAM beyond the direct map's reach (high memory mapped on demand) (D475, D476)
- [ ] MIPS64, SPARC64, s390x, LoongArch64 and POWER: device interrupts spread over CPUs (D536)

## M10 Userland, shell, utilities and the MCP server

- [ ] `fetch`: proxies (`http_proxy`/`https_proxy`), basic authentication, cookies, gzip/deflate `Content-Encoding` (D158)
- [ ] TLS: Ed25519 and ECDSA P-521 certificates (D159)
- [ ] Text Editor: syntax colouring (D149)
- [ ] D-Bus: a session bus of our own (`eggos-dbus`, the wire protocol, `org.freedesktop.DBus` names) for single-instance apps, portals and accessibility (D176)
- [ ] eggsh: arithmetic expansion `$((...))` and `let` (D182)
- [ ] eggsh: `trap`, `getopts`, `printf`, `exec`, `command`, `readonly`, `umask`, `times` built-ins (D182)
- [ ] eggsh: aliases (`alias`, `unalias`) and `set -u`, `-x`, `-f`, `-n` with `$-` (D182)
- [ ] eggsh: `disown`, `suspend`, `set -b`, and `history` with `^R` reverse search (D181)
- [ ] `shutdown` with a delay and a message, and `wall` (D183)
- [ ] MCP over the network: `Mcp-Session-Id`, the `GET` event stream, more than one connection at a time, per-address limits (D193)
- [ ] MCP over the network: mDNS advertisement of the agent endpoint (`_mcp._tcp`) (D193)
- [ ] MCP: `subscriptions/listen` and `input_required` with a signed `requestState` (D193)
- [ ] Time zones: past rules of a zone (transitions from TZif data) and `localtime`/`_tzset` in the Windows layer (D194)
- [ ] Text Editor: a terminal editor (`edit` in a console without a window) (D149)

## M11 Graphics stack and compositor

- [ ] Display buffers larger than 4 MiB from several DMA regions mapped virtually contiguous (D112, D127)
- [ ] virtio-gpu 3D (virgl, then Venus) (D112)
- [ ] Terminal: 2x scale, the kernel's panic message on screen, rendering from a snapshot so a writer never waits for a render, a sleeping display lock (D112, D114)
- [ ] eggway: several outputs, with `wl_registry.global_remove` when a screen goes away (D140)
- [ ] eggway: `zxdg_output_manager_v1`, `xdg_toplevel_icon_v1` and `xdg_wm_dialog_v1` (D179)
- [ ] virtio-gpu: the cursor queue on interrupts; several flushes in flight; per-context timelines (`ring_idx`) and sync objects (D414, D460)
- [ ] `zwp_linux_dmabuf_v1` version 4 feedback objects and multi-plane formats (D459)
- [ ] Mesa's device enumeration nodes in `/sys` (`/sys/dev/char/226:128`, `/sys/class/drm`) (D415)

## M12 Adaptive shell, desktop and accessibility

- [ ] Pointer acceleration and per-device mouse preferences (D195)
- [ ] Virtual desktops: the overview (Super+Tab, F3, Activities) with thumbnails and a desktops strip, the taskbar's `Task view`, `wm list`'s `DESKTOP` column, MCP `wm_desktop_switch`/`wm_desktop_close`/`wm_desktop_move` (D204)
- [ ] Screenshots: `screenshot` with the region picker and its keys, MCP `screen_capture`, the window server's `CAPTURE_SCREEN` and `SHOW_NOTICE` (D204)
- [ ] Accessibility: a screen reader over the element trees (speech through a sound device when one exists, braille-free text log otherwise) (D161, D224)
- [ ] Accessibility: screen zoom, a larger pointer, bounce keys, toggle keys and mouse keys (D224)
- [ ] Touch gestures: two-finger scroll, pinch to zoom, three-finger swipe between desktops, with touch events delivered to apps (D256, D296)
- [ ] Phone form (D303's deviations): `wm show shell` and the shell's element tree, the home grid's long-press menu, its half-shown seventh row and arrow keys, switching to the console when a touch screen is too small
- [ ] Tablet form (D306's deviations): window cards in the chooser, the dock's icon long-press menu, Super+Shift+Left/Right, a split pair as one switcher card, `wm move`/`wm size` picking a side or share
- [ ] TV form (D307's deviations): the network glyph and `Agent` chip in the strip, a boot option for `input::pretend_remote`, an attached pointer's clicks on the home screen
- [ ] Components pane (D300's deviations): per-cause failure lines from the command's failure codes, check times in the local time zone, tooltips on disabled buttons, focus on `Update all` from the chip, the chip in the shell's element tree
- [ ] App actions window (D301's deviations): a quiet success shown as a desktop notice, `Copy output`, type-to-jump in the menu, the read-only output area
- [ ] Light theme and high contrast in what draws itself: the shell's panels, the terminal's grid and the light high-contrast ANSI table, Files' and the Text Editor's own drawing (D219, D224)
- [ ] Toolkit: submenus and checkable menu items, a menu bar's overflow, table column reordering and fit-to-content, multi-selection and cell focus, the text area's undo and word moves (D219)

## M13 Real hardware, vendor drivers and mitigations

- [ ] PAN on AArch64 (`SCTLR_EL1.SPAN` clear, `PSTATE.PAN` set, cleared only inside user-access windows) (D201)
- [ ] NVMe Deallocate (TRIM) and AHCI `DATA SET MANAGEMENT` TRIM, issued by EggFS when blocks are freed (D238)
- [ ] NVMe namespaces past the first and AHCI port multipliers (D238)
- [ ] Linux-style disk names (`nvme0n1`, `sda`) beside `vdX`, and more than four disks (D238)
- [ ] Ethernet: Intel igb (I210/I211), ixgbe (82599/X540/X550) and Realtek RTL8139 drivers (D240)
- [ ] Ethernet: jumbo frames and Wake-on-LAN (D240)
- [ ] Wi-Fi: WPA2/WPA3-Enterprise (802.1X with EAP-PEAP/MSCHAPv2 and EAP-TLS) and roaming between access points of one network (D246)
- [ ] x86_64: the paranoid GS check in the NMI, double-fault, machine-check and debug handlers, and NMIs from ring 3 handled by the kernel (D47, D49, D248)
- [ ] x86_64: emulating UMIP's five instructions (`sgdt`, `sidt`, `sldt`, `smsw`, `str`) for programs that use them (D251)
- [ ] AArch64: GICv3 ITS for PCI MSIs (D237)
- [ ] UEFI runtime services: `GetTime`/`SetTime` as a clock and `GetVariable`/`SetVariable` for a boot entry written at install (D236, D241)
- [ ] Installer: keep other operating systems' partitions (dual boot) (D241)
- [ ] x86_64: STIBP for SMT siblings where the CPU needs it (D260, D267)
- [ ] Wi-Fi: power saving (the firmware's power tables and PS-Poll/U-APSD off by default), and HT/VHT/HE rates and widths in the link setup (D277, D279)
- [ ] Wi-Fi: PMKSA caching, answering SA Query, and BIP on group management frames (D285)
- [ ] Wi-Fi: 6 GHz channels and SAE hash-to-element (D275, D285)
- [ ] Intel HDMI: HDMI 2.0 scrambling above 340 MHz, AVI and audio infoframes, deep colour, HDMI on display 11 and later (D466)
- [ ] Intel display 11 Type-C ports (MG PHY and PLLs) and the combo PHY's drive tables (D465)
- [ ] ACPI `_PRT` legacy PCI interrupt routing through the AML interpreter; INTx through `interrupt-map` on device-tree PCI hosts; shared INTx lines (D413)
- [ ] GPIO controllers (Intel and AMD pin controllers) and GPIO interrupts for I2C-HID devices (D462)
- [ ] `simple-framebuffer`: 16-bit `r5g6b5` formats (D542)
- [ ] AMD DCN: DMCUB commands through the inbox, the other DCN versions' register places, link training and higher rates, HDMI, the scaler, the second pipe, AUX and EDID, watermarks from the mode equations, vertical-blank interrupts, and DCE for older cards (D496, D514, D521)
- [ ] NVIDIA: Turing's GSP boot (version 2 descriptors, the non-PKC booter path), flips into the kernel's own surfaces in video memory, mode changes after the first, more heads, the cursor channel, completion notifiers, GSP-RM's unload booter at shutdown (D512, D522, D530)
- [ ] Intel Type-C: Thunderbolt alternate mode (fixed PLL, AUX I/O select), lane reversal, legacy Type-C connectors, giving the PHY back on unplug, UHBR rates on display 14 (D526, D528)
- [ ] Apple DCP: keeping RTKit answered after boot, flips through swaps between two framebuffers, mode changes, the external `dcpext` instance (D547)
- [ ] Rockchip VOP and Allwinner DE2: mode setting (clocks, HDMI/eDP/MIPI bridges), VOP2 on RK356x and RK3588, cursor planes (D548)
- [ ] Qualcomm MDP5: mode setting (clocks, DSI and HDMI bridges) and the split display's second pipe (D549)
- [ ] Qualcomm DPU (Snapdragon 845 and later, `qcom,sdm845-dpu` and successors including Snapdragon X): the firmware's mode taken over, then flips, then mode setting with the DSI and DisplayPort controllers and their PHYs (owner, 2026-11-13, overruling D549's scope)

## M14 Component manager

- [ ] Binary deltas inside a changed file for component updates (D293)

## M15 Compatibility layers

- [ ] Windows: a benchmark of program start and the hot Win32 calls, run inside the benchmark kernel (D118, D121)
- [ ] macOS: a benchmark of program start and the hot libSystem calls, run inside the benchmark kernel (D120, D123)
- [ ] Windows: `SuspendThread`, `ResumeThread` of a suspended thread, and `TerminateThread` (D121)
- [ ] Windows: ARM64EC and ARM64X hybrid images (D124)
- [ ] Objective-C runtime: associated objects (`objc_setAssociatedObject`/`objc_getAssociatedObject`/`objc_removeAssociatedObjects`) (D142)
- [ ] Objective-C runtime: message forwarding (`forwardingTargetForSelector:`, `+resolveInstanceMethod:`, `forwardInvocation:` with NSInvocation and NSMethodSignature) (D142)
- [ ] Objective-C runtime: `method_setImplementation`, `method_exchangeImplementations`, `class_replaceMethod`, `objc_msgSend_fpret` (D142)
- [ ] AppKit: `NSSlider` (D147)
- [ ] Windows GUI: `WM_DRAWITEM`/`WM_MEASUREITEM` owner-drawn buttons, list boxes, combo boxes and menus (D145)
- [ ] Android: `__system_property_set` through the `property_service` socket and `persist.` properties kept across boots (D188)
- [ ] Windows: `CopyFileExW`, `GetFinalPathNameByHandleW`, `CreateSymbolicLinkW`, `CreateHardLinkW`, `LockFileEx`/`UnlockFileEx` (D189)
- [ ] Windows: `InitializeProcThreadAttributeList`, `UpdateProcThreadAttribute`, `DeleteProcThreadAttributeList` and `STARTUPINFOEXW` (D189)
- [ ] Windows: `MapViewOfFileExNuma` and `SEC_IMAGE` mappings (D189)
- [ ] Translation: x87 transcendentals and the rest (`fsin`, `fcos`, `fsincos`, `fptan`, `fpatan`, `f2xm1`, `fyl2x`, `fyl2xp1`, `fprem`, `fprem1`, `fscale`, `fxtract`, `fxam`, `ftst`, `fi*` integer forms, `fnstenv`/`fldenv`/`fnsave`/`frstor`) (D191)
- [ ] Translation: SSSE3 (`pshufb`, `palignr`, `phadd*`, `pmaddubsw`, `pabs*`, `psign*`, `pmulhrsw`) and the rest of SSE4.1/4.2 (Windows 11 24H2's baseline) (D191)
- [ ] Translation: AVX/AVX2 (VEX decoding, 256-bit registers as register pairs) (D191)
- [ ] Translation: a one-entry inline cache per indirect `call`/`jmp` site (D191)
- [ ] Translation: callbacks with floating-point arguments or results and more than six integer arguments; Objective-C methods with more than four arguments (D191)
- [ ] Translation: the `translated_loop_ratio` benchmark (D191)
- [ ] macOS libSystem: `sigwait`, `siginterrupt`, `sigqueue`, `pthread_atfork` with the layer's locks taken around `fork` (D190)
- [ ] macOS libSystem: extended locales (`newlocale`, `uselocale`, `freelocale`, `*_l` functions), `strcoll`, `strxfrm`, `wcwidth`, `wcswidth` (D190)
- [ ] Windows menus: extended templates (`MENUEX`, `LoadMenuIndirectW`) (D336)
- [ ] Windows edits: word wrap in multi-line edits, `EM_SETRECT`, `EM_SETTABSTOPS`, `EM_FMTLINES`, Windows' 30,000-character default limit (D341)
- [ ] Windows scroll bars: `EnableScrollBar`, `ScrollWindowEx`, `ScrollWindow`, `WM_NCHITTEST` answering `HTVSCROLL`/`HTHSCROLL`, the arrows' repeat while held (D342, D343)
- [ ] Windows menus: Alt pressed and released alone selects the bar; mnemonic underlines only once Alt is pressed (D338)
- [ ] Windows combo boxes: `CB_GETEDITSEL`, `CB_LIMITTEXT`, auto-complete from the list (D340)
- [ ] Windows: `SetThreadStackGuarantee` and `GetThreadStackLimits`; the main thread's overflow as `STATUS_STACK_OVERFLOW` (D346)
- [ ] Windows C++: `__CxxFrameHandler3`/`4` honour `EXCEPTION_COLLIDED_UNWIND` (D347)
- [ ] Winsock: `listen`/`accept` with `FD_ACCEPT`, `FD_OOB`, `WSAEventSelect`, `WSAEnumNetworkEvents` (D348)
- [ ] Windows hooks: `WH_CBT`'s activation, focus and move codes; low-level hooks on the installing thread's loop; injected-input flags (D349)
- [ ] Windows edits: dragging the edit's thumb and arrow repeat while held (D351)
- [ ] WinINet: `INTERNET_FLAG_IGNORE_CERT_*`, `InternetSetOption` security flags, TLS 1.2 servers (with a TLS 1.2 client in `eggos-tls`) (D352)
- [ ] Common controls: per-subitem custom draw (`CDRF_NOTIFYSUBITEMDRAW`), `CDRF_NEWFONT`, custom draw for trackbars, toolbar item states and `TBCDRF_*` (D353, D367, D368)
- [ ] Common controls: label editing started by a slow second click, the edit growing with its text (D354, D361)
- [ ] Tooltips: `TTM_HITTESTW`, `TTM_ENUMTOOLSW`, `TTM_GETCURRENTTOOLW`, `TTM_GETBUBBLESIZE`, `TTM_ADJUSTRECT`, tab controls' `TCS_TOOLTIPS`, `TBSTYLE_EX_MIXEDBUTTONS`, tips longer than 79 units (D355, D356, D362, D363)
- [ ] Tray: `NIN_POPUPOPEN` and `WM_MOUSEMOVE` (version 4), double clicks, `NIN_KEYSELECT`/`NIM_SETFOCUS`, `Shell_NotifyIconGetRect`, `NIN_BALLOONTIMEOUT`/`NIN_BALLOONHIDE`, submenus in shell menus (D357, D364, D370, D371)
- [ ] Layered windows: `UpdateLayeredWindowIndirect` with `prcDirty`, `AlphaBlend` writing alpha, child layered windows (D366, D369)
- [ ] Foundation: an index for NSSet and NSCountedSet and the `nsdictionary_lookup` benchmark (D359)
- [ ] Foundation: binary property lists written (NSKeyedArchiver's default), `$classes` hierarchy, NSError for damaged archives, secure coding's allowed classes, `decodeBytesForKey:returnedLength:` (D360, D365)
- [ ] AppKit: view-based NSTableView, multiple selection with a full NSIndexSet, sorting, per-row heights, row hover through `mouseMoved:` (D373)
- [ ] AppKit: NSImage decoding JPEG, GIF, TIFF, BMP, `.ico` and `.icns` through `eggos-image`, system images, `lockFocus`, `TIFFRepresentation`, `CGImage` (D374)
- [ ] NSUserDefaults: suites, global and argument domains, `NSUserDefaultsDidChangeNotification`, `URLForKey:` (D381)
- [ ] NSTrackingArea: cursor updates, enter/exit while dragging, `trackingArea`/`userData` on events; several screens in NSScreen (D382)
- [ ] NSAttributedString: paragraph styles, underline, links, `enumerateAttributesInRange:`, RTF import (D384)
- [ ] NSGridView: merged cells, padding, hidden rows and columns, inserting and moving rows and columns (D385)
- [ ] CoreGraphics: clipping paths, dashes, caps and joins, alpha blending and anti-aliasing, `CGContextDrawImage`, gradients, shadows, `CGPath` objects (D386)
- [ ] Swift: resilient class stubs, generic class metadata instantiation (`swift_allocateGenericClassMetadata`), `@objc` accessors through KVC (D387)
- [ ] libc++abi: `std::exception_ptr`/`std::rethrow_exception`, `__dynamic_cast`, libc++'s standard exceptions (D380)
- [ ] Windows shell namespace: `IFileDialog` (`FileOpenDialog`/`FileSaveDialog` COM classes) over the system picker, `IShellItemArray`, virtual folders (This PC, Recycle Bin) (D392, D399)
- [ ] COM out of process: interfaces other than `IDispatch` from type information, callbacks and connection points, `VT_ARRAY`, by-reference results (D400)
- [ ] Common controls: `LVS_OWNERDATA` virtual lists with `LVN_ODCACHEHINT`; ANSI notifications to ANSI parents (D402, D403)
- [ ] msvcp140 iostreams: `wchar_t` streams (`wcout`, `wcin`), `num_put`/`numpunct`/`codecvt` facets, `ios_base::failure` thrown when enabled (D405)
- [ ] macOS: storyboard segues (`performSegueWithIdentifier:sender:`, `prepareForSegue:`), split and tab view controllers, child view controllers (D406)
- [ ] macOS: `NSDocument` with its window controllers (D406)
- [ ] macOS: utility-panel and floating-panel window levels (D407)
- [ ] macOS: `RegisterEventHotKey` (Carbon hot keys through the window server's global hot keys) (D398)
- [ ] macOS: `std::bad_cast`/`std::bad_typeid` thrown; ambiguous and non-public bases in `__dynamic_cast` (D408)
- [ ] macOS: `dlopen` of an app's own library at run time; thread-local variables in a carried library (D409)
- [ ] Android: split APKs and app bundles; the manifest's label and icon as the app's name and icon (D470)
- [ ] Android: a garbage collector for the layer's object heap (D472)
- [ ] Android: the DEX compiler's aarch64 and riscv64 back ends, with register allocation (D471, D472)
- [ ] Android: stack traces, `ExceptionInInitializerError`, exceptions crossing the framework's code (D504)
- [ ] Android: `TextWatcher`, `onEditorAction`, input types (numbers, passwords) (D507)
- [ ] Android: `RecyclerView` view types, recycling holders, item decorations, grids; row views shown as themselves (D511, D516)
- [ ] Android: density buckets, `scaleType`, `setImageBitmap` (D509)
- [ ] Android: other resource types (dimensions, colours, styles), configurations beyond the default, `ConstraintLayout`, fragments, more activities and intents (D500)
- [ ] iOS: multi-touch, pinches and rotations, velocities, recognizers that wait for each other to fail, safe areas, `UIScreen` and size classes (D501, D505, D515)
- [ ] Auto Layout: priorities and inequalities, right-to-left leading/trailing, stack view distributions and alignments, baselines (D506, D510)
- [ ] WebP: animated WebP (`ANIM`/`ANMF`) and fancy chroma upsampling (D520)

## M16 Editions

- [ ] Editions that also remove what they do not include (`component edition <name> --exact`) (D258)
- [ ] Editions' own settings: update policy and managed policies for professional and enterprise (`/etc/eggos/policy`) (D258)
- [ ] An edition choice in first run (D258)

## Leftovers (no milestone yet)

- [ ] Sound: an audio stack of our own (virtio-sound and Intel HD Audio drivers, a mixer in a sound server, `sound` CLI, MCP tools and a Settings > Sound pane), with NSSound, `PlaySound`/`waveOut` and SDL audio on it (D383)
