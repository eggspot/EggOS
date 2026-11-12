//! Intel's display engine, display versions 9 to 14 (Skylake to Meteor Lake; D268): the pipe and
//! primary plane the firmware left lit are taken over as they are (no mode setting), the plane
//! is flipped between two buffers the driver maps into the global GTT, and the 64x64 ARGB cursor
//! plane shows the pointer.

use crate::io::RegisterIo;

/// Whether device `device` has a display-9 engine (Skylake to Comet Lake).
pub fn is_display_9(device: u16) -> bool {
    is_supported(device) && !DISPLAY_11_AND_LATER.contains(&((device >> 8) as u8))
}

/// Whether device `device` has a display-11 engine (Ice Lake, Jasper and Elkhart Lake).
pub fn is_display_11(device: u16) -> bool {
    DISPLAY_11.contains(&((device >> 8) as u8))
}

/// Whether [`DisplayEngine::set_mode`] lights a port on device `device`: display 9's and 11's
/// (D447, D465).
pub fn sets_modes(device: u16) -> bool {
    is_display_9(device) || is_display_11(device)
}

/// Intel's PCI vendor id.
pub const VENDOR_INTEL: u16 = 0x8086;
/// The bytes of one global-GTT page.
pub const GTT_PAGE: u64 = 4096;
/// The alignment of a plane's or the cursor's surface in the global GTT.
pub const SURFACE_ALIGNMENT: u64 = 256 * 1024;
/// The cursor's side, in pixels.
pub const CURSOR_SIDE: u32 = 64;
/// The bytes of the cursor's image.
pub const CURSOR_BYTES: usize = (CURSOR_SIDE * CURSOR_SIDE * 4) as usize;
/// How many pipes the engine may have.
pub const MAX_PIPES: usize = 4;
/// The PCI configuration register holding the graphics memory controls (`GGC`).
pub const GRAPHICS_CONTROL: u16 = 0x50;

const PIPE_STRIDE: usize = 0x1000;
const PLANE_CTL: usize = 0x70180;
const PLANE_STRIDE: usize = 0x70188;
const PLANE_SIZE: usize = 0x70190;
const PLANE_SURF: usize = 0x7019c;
const PLANE_SURFLIVE: usize = 0x701ac;
const CUR_CTL: usize = 0x70080;
const CUR_BASE: usize = 0x70084;
const CUR_POS: usize = 0x70088;
const GTT_FLUSH: usize = 0x101008;
const DDI_BUF_CTL: usize = 0x64000;
const DP_TP_CTL: usize = 0x64040;
const PORT_REGISTER_STRIDE: usize = 0x100;
const DDI_BUF_ENABLE: u32 = 1 << 31;
const DDI_PORT_WIDTH_SHIFT: u32 = 1;
const DDI_BUF_SWING_SHIFT: u32 = 24;
const DP_TP_ENABLE: u32 = 1 << 31;
const DP_TP_PATTERN_SHIFT: u32 = 8;
const DP_TP_PATTERN_1: u32 = 0;
const DP_TP_PATTERN_2: u32 = 1;
const DP_TP_NORMAL: u32 = 3;
const DPLL_CTRL1: usize = 0x6c058;
const DPLL_CTRL2: usize = 0x6c05c;
const DPLL_STATUS: usize = 0x6c060;
const DPLL_ENABLES: [usize; 4] = [0x46010, 0x46014, 0x46040, 0x46060];
const DPLL_FOR_PORTS: usize = 1;
const PLL_ENABLE: u32 = 1 << 31;
const TRANSCODER: usize = 0x60000;
const TRANSCODER_STRIDE: usize = 0x1000;
const PIPE_SOURCE: usize = 0x1c;
const DATA_M: usize = 0x30;
const DATA_N: usize = 0x34;
const LINK_M: usize = 0x40;
const LINK_N: usize = 0x44;
const TU_SHIFT: u32 = 25;
const DDI_FUNC_CTL: usize = 0x60400;
const DDI_FUNC_ENABLE: u32 = 1 << 31;
const DDI_PORT_SHIFT: u32 = 28;
const DDI_MODE_DP_SST: u32 = 2 << 24;
const DDI_SYNC_VERTICAL_HIGH: u32 = 1 << 17;
const DDI_SYNC_HORIZONTAL_HIGH: u32 = 1 << 16;
const TRANS_CLK_SEL: usize = 0x46140;
const TRANS_CLK_PORT_SHIFT: u32 = 29;
const PIPE_CONF: usize = 0x70008;
const PIPE_CONF_ENABLE: u32 = 1 << 31;
const BITS_PER_PIXEL: u32 = 24;
const MASTER_INTERRUPT: usize = 0x44200;
const MASTER_ENABLE: u32 = 1 << 31;
const MASTER_PIPE_SHIFT: u32 = 16;
const GFX_MASTER_INTERRUPT: usize = 0x19_0010;
const GFX_MASTER_DISPLAY: u32 = 1 << 16;
const PIPE_INTERRUPT_MASK: usize = 0x44404;
const PIPE_INTERRUPT_IDENTITY: usize = 0x44408;
const PIPE_INTERRUPT_ENABLE: usize = 0x4440c;
const PIPE_INTERRUPT_STRIDE: usize = 0x10;
const PIPE_VBLANK: u32 = 1;
const SCALER_CONTROL: usize = 0x68180;
const SCALER_WINDOW_POSITION: usize = 0x68170;
const SCALER_WINDOW_SIZE: usize = 0x68174;
const SCALER_PIPE_STRIDE: usize = 0x800;
const SCALER_ENABLE: u32 = 1 << 31;
const SCALER_FILTER_BILINEAR: u32 = 3 << 23;
const DIGITAL_PORTS: usize = 5;
const DISPLAY_11: [u8; 2] = [0x8a, 0x4e];
const COMBO_PORTS: usize = 2;
const COMBO_PLL_ENABLES: [usize; 2] = [0x46010, 0x46014];
const COMBO_PLL_CONFIGURATION: usize = 0x16_4000;
const COMBO_PLL_STRIDE: usize = 8;
const PLL_POWER_ENABLE: u32 = 1 << 27;
const PLL_POWER_STATE: u32 = 1 << 26;
const PLL_LOCKED: u32 = 1 << 30;
const PORT_CLOCK_ROUTING: usize = 0x16_4280;
const PORT_CLOCK_OFF_SHIFT: u32 = 10;
const STRAPS: usize = 0x51004;
const REFERENCE_SHIFT: u32 = 29;
const HDMI_PLLS: [usize; 2] = [2, 3];
const DPLL_CONFIGURATION: usize = 0x6c040;
const DPLL_CONFIGURATION_STRIDE: usize = 8;
const DPLL_OVERRIDE: u32 = 1;
const DPLL_HDMI_MODE: u32 = 1 << 5;
const DDI_MODE_HDMI: u32 = 0;
const DISPLAY_11_AND_LATER: [u8; 8] = [0x8a, 0x4e, 0x9a, 0x4c, 0x46, 0xa7, 0x4f, 0x56];

const PLANE_ENABLE: u32 = 1 << 31;
const PLANE_FORMAT_SHIFT: u32 = 23;
const PLANE_FORMAT_MASK: u32 = 0x1f;
const PLANE_FORMAT_XRGB_8888: u32 = 8;
const PLANE_ORDER_RGBX: u32 = 1 << 20;
const PLANE_TILING_SHIFT: u32 = 10;
const PLANE_TILING_MASK: u32 = 0x7;
const PLANE_STRIDE_MASK: u32 = 0xfff;
const STRIDE_UNIT: u32 = 64;
const SURFACE_MASK: u32 = 0xffff_f000;
const CURSOR_MODE_64_ARGB: u32 = 0x27;
const CURSOR_NEGATIVE: u32 = 1 << 15;
const CURSOR_FIELD: u32 = 0x7fff;
const GTT_PRESENT: u64 = 1;
const GTT_FLUSH_ENABLE: u32 = 1;
const GTT_SIZE_SHIFT: u16 = 6;
const GTT_SIZE_MASK: u16 = 0x3;
const GTT_ENTRY_BYTES: u64 = 8;

const DISPLAY_9_TO_14: [u8; 13] = [0x19, 0x31, 0x3e, 0x46, 0x4c, 0x4e, 0x59, 0x7d, 0x87, 0x8a, 0x9a, 0x9b, 0xa7];
/// Why the display engine is not taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// A device this driver does not know.
    UnknownDevice,
    /// No pipe's primary plane is lit.
    NothingLit,
    /// The lit plane is tiled, or not 32-bit XRGB in BGRX order.
    UnsupportedPlane,
    /// The global GTT is too small for the driver's buffers.
    NoGttSpace,
    /// A flip did not take before the deadline.
    FlipTimedOut,
}

impl Refusal {
    /// The reason, for the console.
    pub fn description(self) -> &'static str {
        match self {
            Refusal::UnknownDevice => "an Intel graphics device this driver does not know",
            Refusal::NothingLit => "the firmware lit no display on it",
            Refusal::UnsupportedPlane => "the firmware's plane is tiled or not 32-bit XRGB",
            Refusal::NoGttSpace => "its graphics translation table has no room",
            Refusal::FlipTimedOut => "the display did not take a new frame in time",
        }
    }
}

/// Whether `device` is one of Intel's integrated graphics devices with display versions 9 to 14,
/// by its device id's high byte (discrete cards, whose surfaces live in their own memory, are
/// not).
pub fn is_supported(device: u16) -> bool {
    DISPLAY_9_TO_14.contains(&((device >> 8) as u8))
}

/// The bytes the global GTT translates, from the `GGC` register's GTT size field: 1, 2 or 4 MiB
/// of 8-byte entries, each translating one 4 KiB page.
pub fn gtt_span(graphics_control: u16) -> u64 {
    match (graphics_control >> GTT_SIZE_SHIFT) & GTT_SIZE_MASK {
        0 => 0,
        size => ((1u64 << size) << 20) / GTT_ENTRY_BYTES * GTT_PAGE,
    }
}

/// The plane the firmware left lit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LitPlane {
    /// Its pipe, 0 for A.
    pub pipe: usize,
    /// Its visible width and height, in pixels.
    pub width: u32,
    pub height: u32,
    /// Its row length in bytes.
    pub stride_bytes: u32,
    /// Where its surface is in the global GTT.
    pub surface: u32,
}

/// The stride, in bytes, the driver's buffers use for `width` pixels.
pub fn stride_for(width: u32) -> u32 {
    (width * 4).next_multiple_of(STRIDE_UNIT)
}

/// The cursor position register's value for a top-left corner at (`x`, `y`), which may be off
/// the screen to the left or top.
pub fn cursor_position(x: i32, y: i32) -> u32 {
    let field = |value: i32| {
        let magnitude = value.unsigned_abs().min(CURSOR_FIELD);
        if value < 0 { magnitude | CURSOR_NEGATIVE } else { magnitude }
    };
    field(x) | (field(y) << 16)
}

/// The global GTT entry that maps the page at device address `page`.
pub fn gtt_entry(page: u64) -> u64 {
    (page & !(GTT_PAGE - 1)) | GTT_PRESENT
}

/// The display engine, through its registers in `regs` (BAR 0), whose second half is the global
/// GTT.
pub struct DisplayEngine<R: RegisterIo> {
    regs: R,
    gtt_offset: usize,
    pipe: usize,
    mirrors: u8,
    display_11: bool,
}

impl<R: RegisterIo> DisplayEngine<R> {
    /// The engine of device `device` behind `regs`, `bar_bytes` long, and the plane the
    /// firmware lit on it.
    pub fn take(regs: R, bar_bytes: usize, device: u16) -> Result<(Self, LitPlane), Refusal> {
        let mut engine = Self {
            regs,
            gtt_offset: bar_bytes / 2,
            pipe: 0,
            mirrors: 0,
            display_11: is_display_11(device),
        };
        let lit = (0..MAX_PIPES)
            .find(|&pipe| engine.plane(PLANE_CTL, pipe) & PLANE_ENABLE != 0)
            .ok_or(Refusal::NothingLit)?;
        engine.pipe = lit;
        let control = engine.plane(PLANE_CTL, lit);
        let format = (control >> PLANE_FORMAT_SHIFT) & PLANE_FORMAT_MASK;
        let tiling = (control >> PLANE_TILING_SHIFT) & PLANE_TILING_MASK;
        if format != PLANE_FORMAT_XRGB_8888 || tiling != 0 || control & PLANE_ORDER_RGBX != 0 {
            return Err(Refusal::UnsupportedPlane);
        }
        let size = engine.plane(PLANE_SIZE, lit);
        let plane = LitPlane {
            pipe: lit,
            width: (size & 0xffff) + 1,
            height: (size >> 16) + 1,
            stride_bytes: (engine.plane(PLANE_STRIDE, lit) & PLANE_STRIDE_MASK) * STRIDE_UNIT,
            surface: engine.plane(PLANE_SURF, lit) & SURFACE_MASK,
        };
        Ok((engine, plane))
    }

    fn plane(&self, register: usize, pipe: usize) -> u32 {
        self.regs.read32(register + pipe * PIPE_STRIDE)
    }

    fn set_plane(&self, register: usize, value: u32) {
        self.regs.write32(register + self.pipe * PIPE_STRIDE, value);
    }

    /// Maps the device pages `pages` at global-GTT address `at` on, one entry each.
    pub fn map(&self, at: u64, pages: &mut dyn Iterator<Item = u64>) {
        let mut entry = self.gtt_offset + (at / GTT_PAGE) as usize * GTT_ENTRY_BYTES as usize;
        for page in pages {
            let value = gtt_entry(page);
            self.regs.write32(entry, value as u32);
            self.regs.write32(entry + 4, (value >> 32) as u32);
            entry += GTT_ENTRY_BYTES as usize;
        }
        let _ = self.regs.read32(entry - GTT_ENTRY_BYTES as usize);
        self.regs.write32(GTT_FLUSH, GTT_FLUSH_ENABLE);
    }

    /// Has the lit pipe interrupt at each vertical blank (D443): its interrupt unmasked and
    /// enabled, and the display's (and, on display 11 and later, the graphics') master switch
    /// on, for device `device`.
    pub fn interrupt_on_vblank(&self, device: u16) {
        let at = PIPE_INTERRUPT_STRIDE * self.pipe;
        self.regs.write32(PIPE_INTERRUPT_IDENTITY + at, PIPE_VBLANK);
        self.regs.write32(PIPE_INTERRUPT_MASK + at, !PIPE_VBLANK);
        self.regs.write32(PIPE_INTERRUPT_ENABLE + at, PIPE_VBLANK);
        self.regs.write32(MASTER_INTERRUPT, MASTER_ENABLE | 1 << (MASTER_PIPE_SHIFT + self.pipe as u32));
        if DISPLAY_11_AND_LATER.contains(&((device >> 8) as u8)) {
            self.regs.write32(GFX_MASTER_INTERRUPT, MASTER_ENABLE | GFX_MASTER_DISPLAY);
        }
    }

    /// Clears the lit pipe's vertical-blank cause, so the next one interrupts again: the master
    /// switch off, the cause written back, the switch on.
    pub fn acknowledge_vblank(&self) {
        let master = self.regs.read32(MASTER_INTERRUPT);
        self.regs.write32(MASTER_INTERRUPT, master & !MASTER_ENABLE);
        let at = PIPE_INTERRUPT_STRIDE * self.pipe;
        let cause = self.regs.read32(PIPE_INTERRUPT_IDENTITY + at);
        self.regs.write32(PIPE_INTERRUPT_IDENTITY + at, cause);
        self.regs.write32(MASTER_INTERRUPT, master | MASTER_ENABLE);
    }

    /// What the sink on digital port `port` (0 is port A) says over its AUX channel (D445): its
    /// link capabilities and its EDID, each when it answers.
    pub fn probe_sink(
        &self,
        port: usize,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> (Option<super::intel_aux::LinkCapabilities>, Option<crate::edid::Edid>) {
        let mut dpcd = [0u8; super::intel_aux::MOST_READ];
        let link = super::intel_aux::read_dpcd(
            &self.regs,
            port,
            super::intel_aux::RECEIVER_CAPABILITIES,
            &mut dpcd,
            keep_waiting,
        )
        .ok()
        .and_then(|()| super::intel_aux::link_capabilities(&dpcd));
        let mut block = [0u8; crate::edid::BLOCK_BYTES];
        let edid = super::intel_aux::read_edid(&self.regs, port, &mut block, keep_waiting)
            .ok()
            .and_then(|()| crate::edid::Edid::parse(&block));
        (link, edid)
    }

    /// Trains the DisplayPort link on digital port `port` (D446): `lanes` lanes at link rate code
    /// `rate`, the port's training patterns and drive set through its `DP_TP_CTL` and
    /// `DDI_BUF_CTL`, the sink's side through its AUX channel. The drive it settled on.
    pub fn train_port(
        &self,
        port: usize,
        lanes: usize,
        rate: u8,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<crate::display_port::Drive, crate::display_port::TrainingError> {
        let mut link = PortLink {
            regs: &self.regs,
            port,
            lanes,
            keep_waiting,
        };
        let buffer = DDI_BUF_CTL + port * PORT_REGISTER_STRIDE;
        self.regs.write32(buffer, DDI_BUF_ENABLE | ((lanes.clamp(1, 4) as u32 - 1) << DDI_PORT_WIDTH_SHIFT));
        crate::display_port::train(&mut link, lanes, rate)
    }

    /// Lights pipe `pipe` on DisplayPort port `port` with `timing` over `lanes` lanes at link
    /// rate code `rate`, showing the surface at global-GTT address `surface`, rows
    /// `stride_bytes` long (D447): the port's PLL set to the rate and routed to it (display 9's
    /// shared PLL for ports B to D, or display 11's combo PLL for ports A and B, D465), the
    /// transcoder's timings, M/N, clock and function, the link trained, then the pipe and its
    /// plane on. Polls while `keep_waiting` says to.
    #[allow(clippy::too_many_arguments)]
    pub fn set_mode(
        &self,
        pipe: usize,
        port: usize,
        timing: &crate::edid::Timing,
        lanes: usize,
        rate: u8,
        surface: u32,
        stride_bytes: u32,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<(), ModeError> {
        use super::intel_modeset::{m_n, symbol_clock_khz, timing_registers, TRANSFER_UNIT};
        if pipe >= MAX_PIPES - 1 {
            return Err(ModeError::Unsupported);
        }
        let symbol_khz = symbol_clock_khz(rate).ok_or(ModeError::Unsupported)?;
        match self.display_11 {
            true => self.clock_combo_port(port, rate, keep_waiting)?,
            false => self.clock_display_9_port(port, rate, keep_waiting)?,
        }
        let transcoder = TRANSCODER + pipe * TRANSCODER_STRIDE;
        for (index, value) in timing_registers(timing).iter().enumerate() {
            let at = match index {
                6 => PIPE_SOURCE,
                index => 4 * index,
            };
            self.regs.write32(transcoder + at, *value);
        }
        let ((data_m, data_n), (link_m, link_n)) = m_n(timing.clock_khz, BITS_PER_PIXEL, symbol_khz, lanes as u32);
        self.regs.write32(transcoder + DATA_M, (TRANSFER_UNIT - 1) << TU_SHIFT | data_m);
        self.regs.write32(transcoder + DATA_N, data_n);
        self.regs.write32(transcoder + LINK_M, link_m);
        self.regs.write32(transcoder + LINK_N, link_n);
        self.regs.write32(TRANS_CLK_SEL + 4 * pipe, (port as u32 + 1) << TRANS_CLK_PORT_SHIFT);
        let mut function = DDI_FUNC_ENABLE
            | (port as u32) << DDI_PORT_SHIFT
            | DDI_MODE_DP_SST
            | ((lanes.clamp(1, 4) as u32 - 1) << DDI_PORT_WIDTH_SHIFT);
        if timing.vertical_sync_positive {
            function |= DDI_SYNC_VERTICAL_HIGH;
        }
        if timing.horizontal_sync_positive {
            function |= DDI_SYNC_HORIZONTAL_HIGH;
        }
        self.regs.write32(DDI_FUNC_CTL + pipe * TRANSCODER_STRIDE, function);
        self.train_port(port, lanes, rate, keep_waiting)
            .map_err(|_| ModeError::LinkTraining)?;
        self.regs.write32(PIPE_CONF + pipe * PIPE_STRIDE, PIPE_CONF_ENABLE);
        let at = |register: usize| register + pipe * PIPE_STRIDE;
        self.regs.write32(at(PLANE_SIZE), (timing.height - 1) << 16 | (timing.width - 1));
        self.regs.write32(at(PLANE_STRIDE), stride_bytes / STRIDE_UNIT);
        self.regs.write32(at(PLANE_CTL), PLANE_ENABLE | PLANE_FORMAT_XRGB_8888 << PLANE_FORMAT_SHIFT);
        self.regs.write32(at(PLANE_SURF), surface & SURFACE_MASK);
        Ok(())
    }

    /// The EDID of an HDMI sink on digital port `port` (B to D), read over GMBUS (D466), when
    /// one answers.
    pub fn probe_hdmi_sink(
        &self,
        port: usize,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Option<crate::edid::Edid> {
        let pins = super::intel_hdmi::gmbus_pins(port)?;
        let mut block = [0u8; crate::edid::BLOCK_BYTES];
        super::intel_hdmi::read_edid(&self.regs, pins, &mut block, keep_waiting).ok()?;
        crate::edid::Edid::parse(&block)
    }

    /// Lights pipe `pipe` on HDMI port `port` (B to D) with `timing`, showing the surface at
    /// global-GTT address `surface`, rows `stride_bytes` long (D466, display 9): a free HDMI PLL
    /// set for the pixel clock and routed to the port, the transcoder's timings, clock and
    /// HDMI function at 8 bits a colour, the port's buffer, then the pipe and its plane on. No
    /// link to train: TMDS runs at the pixel clock.
    #[allow(clippy::too_many_arguments)]
    pub fn set_mode_hdmi(
        &self,
        pipe: usize,
        port: usize,
        timing: &crate::edid::Timing,
        surface: u32,
        stride_bytes: u32,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<(), ModeError> {
        if self.display_11 || port == 0 || port > 3 || pipe >= MAX_PIPES - 1 {
            return Err(ModeError::Unsupported);
        }
        let settings =
            super::intel_hdmi::hdmi_pll(timing.clock_khz).ok_or(ModeError::Unsupported)?;
        let pll = HDMI_PLLS
            .into_iter()
            .find(|&pll| self.regs.read32(DPLL_ENABLES[pll]) & PLL_ENABLE == 0)
            .ok_or(ModeError::Unsupported)?;
        let field = 6 * pll as u32;
        let control = self.regs.read32(DPLL_CTRL1) & !(0x3f << field);
        self.regs.write32(DPLL_CTRL1, control | (DPLL_OVERRIDE | DPLL_HDMI_MODE) << field);
        let configuration = DPLL_CONFIGURATION + (pll - 1) * DPLL_CONFIGURATION_STRIDE;
        self.regs.write32(configuration, settings.configuration_1());
        self.regs.write32(configuration + 4, settings.configuration_2());
        self.regs.write32(DPLL_ENABLES[pll], PLL_ENABLE);
        while self.regs.read32(DPLL_STATUS) & 1 << (8 * pll) == 0 {
            if !keep_waiting() {
                return Err(ModeError::PllUnlocked);
            }
        }
        let route = 3 * port as u32;
        let routing = self.regs.read32(DPLL_CTRL2) & !(0x7 << route | 1 << (15 + port as u32));
        self.regs.write32(DPLL_CTRL2, routing | 1 << route | (pll as u32) << (route + 1));
        let transcoder = TRANSCODER + pipe * TRANSCODER_STRIDE;
        for (index, value) in super::intel_modeset::timing_registers(timing).iter().enumerate() {
            let at = match index {
                6 => PIPE_SOURCE,
                index => 4 * index,
            };
            self.regs.write32(transcoder + at, *value);
        }
        self.regs.write32(TRANS_CLK_SEL + 4 * pipe, (port as u32 + 1) << TRANS_CLK_PORT_SHIFT);
        let mut function = DDI_FUNC_ENABLE | (port as u32) << DDI_PORT_SHIFT | DDI_MODE_HDMI;
        if timing.vertical_sync_positive {
            function |= DDI_SYNC_VERTICAL_HIGH;
        }
        if timing.horizontal_sync_positive {
            function |= DDI_SYNC_HORIZONTAL_HIGH;
        }
        self.regs.write32(DDI_FUNC_CTL + pipe * TRANSCODER_STRIDE, function);
        self.regs.write32(DDI_BUF_CTL + port * PORT_REGISTER_STRIDE, DDI_BUF_ENABLE);
        self.regs.write32(PIPE_CONF + pipe * PIPE_STRIDE, PIPE_CONF_ENABLE);
        let at = |register: usize| register + pipe * PIPE_STRIDE;
        self.regs.write32(at(PLANE_SIZE), (timing.height - 1) << 16 | (timing.width - 1));
        self.regs.write32(at(PLANE_STRIDE), stride_bytes / STRIDE_UNIT);
        self.regs.write32(
            at(PLANE_CTL),
            PLANE_ENABLE | PLANE_FORMAT_XRGB_8888 << PLANE_FORMAT_SHIFT,
        );
        self.regs.write32(at(PLANE_SURF), surface & SURFACE_MASK);
        Ok(())
    }

    fn clock_display_9_port(
        &self,
        port: usize,
        rate: u8,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<(), ModeError> {
        if port == 0 || port > 3 {
            return Err(ModeError::Unsupported);
        }
        let code = super::intel_modeset::skylake_link_rate(rate).ok_or(ModeError::Unsupported)?;
        let pll = DPLL_FOR_PORTS;
        let field = 6 * pll as u32;
        let control = self.regs.read32(DPLL_CTRL1) & !(0x3f << field);
        self.regs.write32(DPLL_CTRL1, control | 1 << field | code << (field + 1));
        self.regs.write32(DPLL_ENABLES[pll], PLL_ENABLE);
        while self.regs.read32(DPLL_STATUS) & 1 << (8 * pll) == 0 {
            if !keep_waiting() {
                return Err(ModeError::PllUnlocked);
            }
        }
        let route = 3 * port as u32;
        let routing = self.regs.read32(DPLL_CTRL2) & !(0x7 << route | 1 << (15 + port as u32));
        self.regs.write32(DPLL_CTRL2, routing | 1 << route | (pll as u32) << (route + 1));
        Ok(())
    }

    /// Display 11's combo ports A and B (D465): a combo PLL the firmware left off is powered,
    /// set to the rate from the reference clock the straps name, enabled until it locks, then
    /// routed to the port and the port's clock turned on.
    fn clock_combo_port(
        &self,
        port: usize,
        rate: u8,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> Result<(), ModeError> {
        use super::intel_combo_phy::{for_display_port, reference_khz};
        if port >= COMBO_PORTS {
            return Err(ModeError::Unsupported);
        }
        let reference = reference_khz(self.regs.read32(STRAPS) >> REFERENCE_SHIFT & 0x7)
            .ok_or(ModeError::Unsupported)?;
        let settings = for_display_port(rate, reference).ok_or(ModeError::Unsupported)?;
        let pll = (0..COMBO_PLL_ENABLES.len())
            .find(|&pll| self.regs.read32(COMBO_PLL_ENABLES[pll]) & PLL_ENABLE == 0)
            .ok_or(ModeError::Unsupported)?;
        let enable = COMBO_PLL_ENABLES[pll];
        let wait_for = |bit: u32, keep_waiting: &mut dyn FnMut() -> bool| {
            while self.regs.read32(enable) & bit == 0 {
                if !keep_waiting() {
                    return Err(ModeError::PllUnlocked);
                }
            }
            Ok(())
        };
        self.regs.write32(enable, PLL_POWER_ENABLE);
        wait_for(PLL_POWER_STATE, keep_waiting)?;
        let configuration = COMBO_PLL_CONFIGURATION + pll * COMBO_PLL_STRIDE;
        self.regs.write32(configuration, settings.configuration_0());
        self.regs.write32(configuration + 4, settings.configuration_1());
        self.regs.write32(enable, PLL_POWER_ENABLE | PLL_ENABLE);
        wait_for(PLL_LOCKED, keep_waiting)?;
        let field = 2 * port as u32;
        let routing = self.regs.read32(PORT_CLOCK_ROUTING) & !(0x3 << field);
        self.regs.write32(PORT_CLOCK_ROUTING, routing | (pll as u32) << field);
        let routed = self.regs.read32(PORT_CLOCK_ROUTING);
        self.regs.write32(
            PORT_CLOCK_ROUTING,
            routed & !(1 << (PORT_CLOCK_OFF_SHIFT + port as u32)),
        );
        Ok(())
    }

    /// Loads the display microcontroller's firmware (D444): its program into the DMC's program
    /// memory, word by word, then the register writes that start it.
    pub fn load_dmc(&self, dmc: &super::intel_dmc::Dmc<'_>) {
        for (index, word) in dmc.program.chunks_exact(4).enumerate() {
            let value = u32::from_le_bytes([word[0], word[1], word[2], word[3]]);
            self.regs.write32(dmc.program_base as usize + 4 * index, value);
        }
        for &(register, value) in &dmc.writes[..dmc.count] {
            self.regs.write32(register as usize, value);
        }
    }

    /// Shows the surface at global-GTT address `surface`, rows `stride_bytes` long, from the next
    /// vertical blank, on the lit pipe and every pipe mirroring it (D464).
    pub fn flip(&self, surface: u32, stride_bytes: u32) {
        let shown = |pipe: &usize| *pipe == self.pipe || self.mirrors & 1 << *pipe != 0;
        for pipe in (0..MAX_PIPES).filter(shown) {
            let at = |register: usize| register + pipe * PIPE_STRIDE;
            self.regs.write32(at(PLANE_STRIDE), stride_bytes / STRIDE_UNIT);
            self.regs.write32(at(PLANE_SURF), surface & SURFACE_MASK);
        }
    }

    /// The digital ports the firmware lit, a bit each (port A is bit 0): their buffers on.
    pub fn lit_ports(&self) -> u8 {
        let on = |port: &usize| {
            self.regs.read32(DDI_BUF_CTL + port * PORT_REGISTER_STRIDE) & DDI_BUF_ENABLE != 0
        };
        (0..DIGITAL_PORTS)
            .filter(on)
            .fold(0, |ports, port| ports | 1 << port)
    }

    /// A pipe [`Self::set_mode`] may light: not the lit one and not on.
    pub fn free_pipe(&self) -> Option<usize> {
        (0..MAX_PIPES - 1).find(|&pipe| {
            pipe != self.pipe
                && self.regs.read32(PIPE_CONF + pipe * PIPE_STRIDE) & PIPE_CONF_ENABLE == 0
        })
    }

    /// Has pipe `pipe`, lit by [`Self::set_mode`] with `timing`, show the lit pipe's
    /// `source`-sized picture scaled to its screen, keeping its shape, through the pipe's
    /// scaler, and follow every flip from now on (D464): a mirror of the first screen.
    pub fn mirror(&mut self, pipe: usize, source: (u32, u32), timing: &crate::edid::Timing) {
        let (size, at) = super::intel_modeset::fitted(source, (timing.width, timing.height));
        let pair = |high: u32, low: u32| (high & 0x1fff) << 16 | (low & 0x1fff);
        let transcoder = TRANSCODER + pipe * TRANSCODER_STRIDE;
        self.regs.write32(transcoder + PIPE_SOURCE, pair(source.0 - 1, source.1 - 1));
        self.regs.write32(PLANE_SIZE + pipe * PIPE_STRIDE, pair(source.1 - 1, source.0 - 1));
        let scaler = pipe * SCALER_PIPE_STRIDE;
        self.regs.write32(SCALER_WINDOW_POSITION + scaler, at.0 << 16 | at.1);
        self.regs.write32(SCALER_WINDOW_SIZE + scaler, size.0 << 16 | size.1);
        self.regs.write32(SCALER_CONTROL + scaler, SCALER_ENABLE | SCALER_FILTER_BILINEAR);
        self.mirrors |= 1 << pipe;
    }

    /// Whether the plane scans out `surface` now.
    pub fn showing(&self, surface: u32) -> bool {
        self.plane(PLANE_SURFLIVE, self.pipe) & SURFACE_MASK == surface & SURFACE_MASK
    }

    /// Shows the cursor image at global-GTT address `image` with its top-left corner at
    /// (`x`, `y`), from the next vertical blank.
    pub fn show_cursor(&self, image: u32, x: i32, y: i32) {
        let pipe = self.pipe * PIPE_STRIDE;
        self.regs.write32(CUR_CTL + pipe, CURSOR_MODE_64_ARGB);
        self.regs.write32(CUR_POS + pipe, cursor_position(x, y));
        self.regs.write32(CUR_BASE + pipe, image & SURFACE_MASK);
    }

    /// Hides the cursor from the next vertical blank.
    pub fn hide_cursor(&self, image: u32) {
        let pipe = self.pipe * PIPE_STRIDE;
        self.regs.write32(CUR_CTL + pipe, 0);
        self.regs.write32(CUR_BASE + pipe, image & SURFACE_MASK);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::cell::RefCell;
    use std::collections::BTreeMap;

    #[derive(Default)]
    struct Registers(RefCell<BTreeMap<usize, u32>>);

    impl RegisterIo for Registers {
        fn read8(&self, _offset: usize) -> u8 {
            0
        }
        fn write8(&self, _offset: usize, _value: u8) {}
        fn read16(&self, _offset: usize) -> u16 {
            0
        }
        fn write16(&self, _offset: usize, _value: u16) {}
        fn read32(&self, offset: usize) -> u32 {
            self.0.borrow().get(&offset).copied().unwrap_or(0)
        }
        fn write32(&self, offset: usize, value: u32) {
            self.0.borrow_mut().insert(offset, value);
        }
    }

    const SKYLAKE: u16 = 0x1912;

    fn lit_on_pipe_b(control: u32) -> Registers {
        let regs = Registers::default();
        regs.write32(PLANE_CTL + PIPE_STRIDE, control);
        regs.write32(PLANE_SIZE + PIPE_STRIDE, (1079 << 16) | 1919);
        regs.write32(PLANE_STRIDE + PIPE_STRIDE, 7680 / 64);
        regs.write32(PLANE_SURF + PIPE_STRIDE, 0x0010_0000);
        regs
    }

    #[test]
    fn the_lit_plane_is_found_with_its_size_stride_and_surface() {
        let control = PLANE_ENABLE | (PLANE_FORMAT_XRGB_8888 << PLANE_FORMAT_SHIFT);
        let (_, plane) = DisplayEngine::take(lit_on_pipe_b(control), 16 << 20, SKYLAKE).ok().unwrap();
        assert_eq!(
            plane,
            LitPlane {
                pipe: 1,
                width: 1920,
                height: 1080,
                stride_bytes: 7680,
                surface: 0x0010_0000,
            }
        );
    }

    #[test]
    fn tiled_or_rgb_ordered_planes_and_dark_engines_are_refused() {
        let xrgb = PLANE_ENABLE | (PLANE_FORMAT_XRGB_8888 << PLANE_FORMAT_SHIFT);
        assert_eq!(
            DisplayEngine::take(lit_on_pipe_b(xrgb | (1 << PLANE_TILING_SHIFT)), 16 << 20, SKYLAKE).err(),
            Some(Refusal::UnsupportedPlane)
        );
        assert_eq!(
            DisplayEngine::take(lit_on_pipe_b(xrgb | PLANE_ORDER_RGBX), 16 << 20, SKYLAKE).err(),
            Some(Refusal::UnsupportedPlane)
        );
        assert_eq!(
            DisplayEngine::take(Registers::default(), 16 << 20, SKYLAKE).err(),
            Some(Refusal::NothingLit)
        );
    }

    #[test]
    fn a_flip_writes_the_stride_before_the_surface_and_maps_pages_into_the_gtt() {
        let control = PLANE_ENABLE | (PLANE_FORMAT_XRGB_8888 << PLANE_FORMAT_SHIFT);
        let (engine, _) = DisplayEngine::take(lit_on_pipe_b(control), 16 << 20, SKYLAKE).ok().unwrap();
        engine.flip(0x3fc0_0000, stride_for(1366));
        assert_eq!(engine.regs.read32(PLANE_STRIDE + PIPE_STRIDE), 5504 / 64);
        assert_eq!(engine.regs.read32(PLANE_SURF + PIPE_STRIDE), 0x3fc0_0000);
        engine.map(0x2000, &mut [0x1_2345_6000u64, 0x7000].into_iter());
        let first = (8 << 20) + 2 * 8;
        assert_eq!(engine.regs.read32(first), 0x2345_6001);
        assert_eq!(engine.regs.read32(first + 4), 1);
        assert_eq!(engine.regs.read32(first + 8), 0x7001);
        assert_eq!(engine.regs.read32(GTT_FLUSH), GTT_FLUSH_ENABLE);
    }

    #[test]
    fn the_cursor_position_carries_signs_and_the_gtt_span_its_size_field() {
        assert_eq!(cursor_position(10, 20), (20 << 16) | 10);
        assert_eq!(cursor_position(-3, 5), (5 << 16) | 3 | CURSOR_NEGATIVE);
        assert_eq!(gtt_span(0), 0);
        assert_eq!(gtt_span(2 << GTT_SIZE_SHIFT), 2 << 30);
        assert_eq!(gtt_span(3 << GTT_SIZE_SHIFT), 4 << 30);
        assert!(is_supported(0x9a49));
        assert!(!is_supported(0x1234));
    }
}

struct PortLink<'a, R: RegisterIo> {
    regs: &'a R,
    port: usize,
    lanes: usize,
    keep_waiting: &'a mut dyn FnMut() -> bool,
}

impl<R: RegisterIo> crate::display_port::Link for PortLink<'_, R> {
    fn read(&mut self, address: u32, out: &mut [u8]) -> bool {
        super::intel_aux::read_dpcd(self.regs, self.port, address, out, self.keep_waiting).is_ok()
    }

    fn write(&mut self, address: u32, bytes: &[u8]) -> bool {
        super::intel_aux::write_dpcd(self.regs, self.port, address, bytes, self.keep_waiting).is_ok()
    }

    fn send_pattern(&mut self, pattern: u8) {
        let code = match pattern {
            1 => DP_TP_PATTERN_1,
            2 => DP_TP_PATTERN_2,
            _ => DP_TP_NORMAL,
        };
        self.regs.write32(
            DP_TP_CTL + self.port * PORT_REGISTER_STRIDE,
            DP_TP_ENABLE | code << DP_TP_PATTERN_SHIFT,
        );
    }

    fn set_drive(&mut self, drive: crate::display_port::Drive) {
        let level = u32::from(drive.swing) * 4 + u32::from(drive.pre_emphasis);
        let width = (self.lanes.clamp(1, 4) as u32 - 1) << DDI_PORT_WIDTH_SHIFT;
        self.regs.write32(
            DDI_BUF_CTL + self.port * PORT_REGISTER_STRIDE,
            DDI_BUF_ENABLE | width | level << DDI_BUF_SWING_SHIFT,
        );
    }

    fn pause(&mut self, microseconds: u32) {
        for _ in 0..microseconds {
            if !(self.keep_waiting)() {
                break;
            }
            core::hint::spin_loop();
        }
    }
}

/// Why a mode could not be set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModeError {
    /// A port, pipe or link rate this driver does not light yet.
    Unsupported,
    /// The port's PLL did not lock.
    PllUnlocked,
    /// The sink's link did not train.
    LinkTraining,
}
