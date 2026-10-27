//! What Settings reports about the system and the choices it shows, gathered by the program
//! from the kernel (or made up by a test), so the window and `settings` show the same values.

use eggos_toolkit::text::Text;

/// The system's facts and the current choices.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Facts {
    /// EggOS's version.
    pub version: &'static str,
    /// The machine's architecture, as `uname -m` says it.
    pub architecture: Text<16>,
    /// CPUs online.
    pub cpus: u32,
    /// Memory in whole MB.
    pub memory_mb: u64,
    /// Seconds since boot.
    pub uptime_s: u64,
    /// The screen's size in pixels.
    pub resolution: (u32, u32),
    /// The display's scale.
    pub scale: u32,
    /// The desktop's layout profile: its index in the settings file's `PROFILES`.
    pub profile: usize,
    /// Whether `/home` is on a disk, so a choice survives a power-off.
    pub on_disk: bool,
}

impl Facts {
    /// The facts of a 1280x800 aarch64 machine with 4 CPUs and 1 GiB, up 1:02:05, in the
    /// macOS profile with a disk: what tests show.
    pub fn example() -> Self {
        Self {
            version: "0.2.0",
            architecture: Text::from("aarch64"),
            cpus: 4,
            memory_mb: 1024,
            uptime_s: 3725,
            resolution: (1280, 800),
            scale: 1,
            profile: 1,
            on_disk: true,
        }
    }
}
