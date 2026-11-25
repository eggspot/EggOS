//! The sound output (D773): the one output device a driver installed (virtio sound in QEMU,
//! Intel HD Audio on PCs), which the sound server (`soundd`) feeds through `/dev/sound` with
//! interleaved 16-bit stereo frames at the device's rate, and which `/proc/driver/sound`
//! describes: one line per device, its id, kind, rate and name (`hda0-out output 48000
//! Built-in speakers`), or `none`.

use crate::sync::SpinLock;
use alloc::boxed::Box;
use alloc::string::String;
use core::fmt::Write;
use eggos_drivers::sound::PcmFormat;

/// A sound output a driver drives.
pub trait SoundOutput: Send {
    /// Its id, `<driver><index>-out` (`hda0-out`, `virtio0-out`).
    fn id(&self) -> &'static str;
    /// What the person calls it (`Built-in speakers`, `Headphones`, `virtio sound`).
    fn name(&self) -> &'static str;
    /// The format it plays.
    fn format(&self) -> PcmFormat;
    /// Queues the start of `samples`; how many bytes it took (0 while it is full).
    fn write(&mut self, samples: &[u8]) -> usize;
}

static OUTPUT: SpinLock<Option<Box<dyn SoundOutput>>> = SpinLock::new(None);

/// Installs `output` as the sound output; it comes back when one is installed already.
pub fn install(output: Box<dyn SoundOutput>) -> Result<(), Box<dyn SoundOutput>> {
    let mut installed = OUTPUT.lock();
    if installed.is_some() {
        return Err(output);
    }
    *installed = Some(output);
    Ok(())
}

/// Whether a sound output is installed.
pub fn is_present() -> bool {
    OUTPUT.lock().is_some()
}

/// Queues the start of `samples` to the output; how many bytes it took, 0 with no output.
pub fn write(samples: &[u8]) -> usize {
    OUTPUT
        .lock()
        .as_mut()
        .map_or(0, |output| output.write(samples))
}

/// `/proc/driver/sound`'s text, without its final newline.
pub fn status_text() -> String {
    let output = OUTPUT.lock();
    let Some(output) = output.as_ref() else {
        return String::from("none");
    };
    let mut text = String::new();
    let _ = write!(
        text,
        "{} output {} {}",
        output.id(),
        output.format().rate,
        output.name()
    );
    text
}
