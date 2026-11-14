//! The pointing devices as `/proc/driver/pointers` lists them (D601): one kind word a line, the
//! line's place (from 1) being the device's number in `mouse.device.<n>.*`, the Mouse pane's
//! `Device <n>` and `mouse.devices`.

/// Where the kernel lists the pointing devices.
pub const POINTERS_FILE: &str = "/proc/driver/pointers";
/// The most pointing devices kept.
pub const MOST_POINTERS: usize = 16;

/// What a pointing device is, as far as the kernel knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerKind {
    /// Relative axes: a mouse, a touchpad or a trackpoint.
    Mouse,
    /// Absolute axes: a pen tablet, or a tablet a virtual machine offers.
    Tablet,
    /// Neither known.
    Pointer,
}

impl PointerKind {
    /// Its word in `mouse.devices`: `mouse`, `tablet` or `pointer`.
    pub fn word(self) -> &'static str {
        match self {
            Self::Mouse => "mouse",
            Self::Tablet => "tablet",
            Self::Pointer => "pointer",
        }
    }

    /// The Mouse pane's hint beside the picker.
    pub fn hint(self) -> &'static str {
        match self {
            Self::Mouse => "a mouse or touchpad",
            Self::Tablet => "a tablet or pen",
            Self::Pointer => "a pointing device",
        }
    }

    /// Whether the kernel scales its moves: not for one that points where it moves.
    pub fn has_speed(self) -> bool {
        self != Self::Tablet
    }
}

/// The pointing devices present, device `n` at `n - 1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PointingDevices {
    pub kinds: [Option<PointerKind>; MOST_POINTERS],
}

impl PointingDevices {
    /// The devices `text`, the file's contents, lists.
    pub fn parse(text: &[u8]) -> Self {
        let mut devices = Self::default();
        let lines = text
            .split(|&byte| byte == b'\n')
            .map(<[u8]>::trim_ascii)
            .filter(|line| !line.is_empty());
        for (slot, line) in devices.kinds.iter_mut().zip(lines) {
            *slot = Some(match line {
                b"mouse" => PointerKind::Mouse,
                b"tablet" => PointerKind::Tablet,
                _ => PointerKind::Pointer,
            });
        }
        devices
    }

    /// How many are present.
    pub fn count(&self) -> usize {
        self.kinds.iter().flatten().count()
    }

    /// Device `number`'s kind (from 1), `None` when it is not attached.
    pub fn kind(&self, number: usize) -> Option<PointerKind> {
        self.kinds.get(number.checked_sub(1)?).copied().flatten()
    }

    /// Writes `mouse.devices`' value: the kinds' words joined by commas.
    pub fn write_words(&self, out: &mut impl core::fmt::Write) -> core::fmt::Result {
        for (index, kind) in self.kinds.iter().flatten().enumerate() {
            if index > 0 {
                out.write_char(',')?;
            }
            out.write_str(kind.word())?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mouse_and_a_tablet_are_devices_1_and_2() {
        let devices = PointingDevices::parse(b"mouse\ntablet\n");
        assert_eq!(devices.count(), 2);
        assert_eq!(devices.kind(1), Some(PointerKind::Mouse));
        assert_eq!(devices.kind(2), Some(PointerKind::Tablet));
        assert_eq!(devices.kind(3), None);
        assert_eq!(devices.kind(0), None);
        let mut words = std::string::String::new();
        devices.write_words(&mut words).unwrap();
        assert_eq!(words, "mouse,tablet");
    }

    #[test]
    fn no_file_is_no_device() {
        assert_eq!(PointingDevices::parse(b"").count(), 0);
    }
}
