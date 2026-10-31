//! `app icon` (D196, docs/ux/app-icons-v0.3.md section 7): an installed app's own icon, the
//! shell-ready `app.icon` the installer wrote, written as an 8-bit RGBA PNG at 48 or 32 pixels,
//! its pixels un-premultiplied. Nothing is decoded but that file.

use crate::files::{Files, Handle, Path};
use crate::installer::{self, APPLICATIONS};
use eggos_app_manifest::icon::{self as file, LARGE, LARGE_PIXELS, SMALL};
use eggos_png::{EncodeError, Encoder, EncoderWorkspace, PngSink};
use eggos_runtime::errno::Errno;

/// The sizes `app icon --size` takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconSize {
    /// 48 by 48, the default.
    Large,
    /// 32 by 32.
    Small,
}

impl IconSize {
    /// The size `--size` names: `48` or `32`.
    pub fn named(text: &[u8]) -> Option<Self> {
        match text {
            b"48" => Some(Self::Large),
            b"32" => Some(Self::Small),
            _ => None,
        }
    }

    /// Its width and height in pixels.
    pub const fn pixels(self) -> usize {
        match self {
            Self::Large => LARGE,
            Self::Small => SMALL,
        }
    }
}

/// Why an app's icon could not be written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportError {
    /// No app of that name is installed.
    NotInstalled,
    /// The app is drawn as the generic tile: a built-in, no `icon=` line, or a file that is
    /// not an icon.
    NoIcon,
    /// The PNG could not be created or written.
    Write(Errno),
}

/// A premultiplied `0xAARRGGBB` pixel as straight alpha: each channel `(c * 255 + a / 2) / a`,
/// and 0 where `a` is 0.
pub fn straight(pixel: u32) -> u32 {
    let alpha = pixel >> 24;
    if alpha == 0 {
        return 0;
    }
    let channel = |shift: u32| (((pixel >> shift & 0xff) * 255 + alpha / 2) / alpha).min(255);
    alpha << 24 | channel(16) << 16 | channel(8) << 8 | channel(0)
}

struct FileSink<'f, F: Files> {
    files: &'f mut F,
    handle: Handle,
}

impl<F: Files> PngSink for FileSink<'_, F> {
    type Error = Errno;

    fn write(&mut self, bytes: &[u8]) -> Result<(), Errno> {
        self.files.write(self.handle, bytes)
    }
}

/// Writes app `name`'s icon at `size` as a PNG to `output`, encoding in `work`.
pub fn export(
    files: &mut impl Files,
    name: &[u8],
    size: IconSize,
    output: &[u8],
    work: &mut EncoderWorkspace,
) -> Result<(), ExportError> {
    if crate::command::is_built_in(name) {
        return Err(ExportError::NoIcon);
    }
    let mut manifest = [0u8; eggos_app_manifest::LARGEST + 1];
    let length =
        installer::read_manifest(files, name, &mut manifest).ok_or(ExportError::NotInstalled)?;
    let app =
        eggos_app_manifest::parse(&manifest[..length], name).ok_or(ExportError::NotInstalled)?;
    let icon = app.icon.ok_or(ExportError::NoIcon)?;
    let path = Path::from(APPLICATIONS)
        .and_then(|folder| folder.join(app.name.as_bytes()))
        .and_then(|folder| folder.join(icon.as_bytes()))
        .ok_or(ExportError::NoIcon)?;
    let mut bytes = [0u8; file::FILE_BYTES + 1];
    let read = crate::icon::read_start(files, path.as_bytes(), &mut bytes);
    let mut pixels = [0u32; LARGE_PIXELS];
    let side = size.pixels();
    let skipped = match size {
        IconSize::Large => 0,
        IconSize::Small => LARGE_PIXELS,
    };
    let all = file::pixels(&bytes[..read]).ok_or(ExportError::NoIcon)?;
    for (place, pixel) in pixels.iter_mut().zip(all.skip(skipped).take(side * side)) {
        *place = straight(pixel);
    }
    let handle = files.create(output, 0o644).map_err(ExportError::Write)?;
    let sink = FileSink { files, handle };
    let written =
        Encoder::new_rgba(side as u32, side as u32, work, sink).and_then(|mut encoder| {
            for row in pixels[..side * side].chunks_exact(side) {
                encoder.write_row(row)?;
            }
            encoder.finish()
        });
    let sink = match written {
        Ok(sink) => sink,
        Err(EncodeError::Sink(errno)) => return Err(ExportError::Write(errno)),
        Err(_) => return Err(ExportError::Write(Errno::EIO)),
    };
    sink.files.close(handle);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::FakeFiles;
    use crate::fake::Node;
    use eggos_png::inflate::SliceSource;
    use eggos_png::rows::{RowDecoder, RowWorkspace};

    fn icon_file() -> Vec<u8> {
        let mut pixels = [0u32; file::PIXELS];
        for (index, pixel) in pixels.iter_mut().enumerate() {
            *pixel = match index {
                0 => 0xffff_0000,
                1 => 0x8000_8000,
                2 => 0,
                index if index >= LARGE_PIXELS => 0xff00_00ff,
                _ => 0xffff_ffff,
            };
        }
        let mut bytes = [0u8; file::FILE_BYTES];
        file::write(&pixels, &mut bytes);
        bytes.to_vec()
    }

    fn installed(icon: Option<&[u8]>) -> FakeFiles {
        let mut files = FakeFiles::new();
        files.directory("/home");
        files.directory("/Applications");
        files.directory("/Applications/Paint");
        let manifest = match icon {
            Some(_) => "eggos-app 1\nname=Paint\nsource=windows\nrun=paint.exe\nicon=app.icon\n",
            None => "eggos-app 1\nname=Paint\nsource=windows\nrun=paint.exe\n",
        };
        files.file("/Applications/Paint/app.manifest", manifest.as_bytes());
        if let Some(icon) = icon {
            files.file("/Applications/Paint/app.icon", icon);
        }
        files
    }

    fn png_at(files: &FakeFiles, path: &str) -> Vec<Vec<u32>> {
        let Some(Node::File(bytes, _)) = files.find(path) else {
            panic!("no {path}");
        };
        let mut source = SliceSource::new(bytes);
        let decoder = RowDecoder::open(&mut source).unwrap();
        let mut work = Box::new(RowWorkspace::new());
        let mut rows = Vec::new();
        decoder
            .read_rows(&mut work, |_, row| rows.push(row.to_vec()))
            .unwrap();
        rows
    }

    #[test]
    fn premultiplied_pixels_come_back_straight() {
        assert_eq!(straight(0xffff_0000), 0xffff_0000);
        assert_eq!(straight(0x8000_8000), 0x8000_ff00);
        assert_eq!(straight(0x0012_3456), 0);
        assert_eq!(straight(0x8040_2010), 0x8080_4020);
    }

    #[test]
    fn the_large_or_the_small_image_is_written_as_an_rgba_png() {
        let mut files = installed(Some(&icon_file()));
        let mut work = Box::new(EncoderWorkspace::new());
        export(
            &mut files,
            b"Paint",
            IconSize::Large,
            b"/home/paint.png",
            &mut work,
        )
        .unwrap();
        let rows = png_at(&files, "/home/paint.png");
        assert_eq!((rows.len(), rows[0].len()), (48, 48));
        assert_eq!(&rows[0][..4], [0xffff_0000, 0x8000_ff00, 0, 0xffff_ffff]);
        export(
            &mut files,
            b"Paint",
            IconSize::Small,
            b"/home/small.png",
            &mut work,
        )
        .unwrap();
        let rows = png_at(&files, "/home/small.png");
        assert_eq!((rows.len(), rows[31][31]), (32, 0xff00_00ff));
    }

    #[test]
    fn an_app_without_an_icon_or_not_installed_says_so() {
        let mut work = Box::new(EncoderWorkspace::new());
        let mut files = installed(None);
        assert_eq!(
            export(
                &mut files,
                b"Paint",
                IconSize::Large,
                b"/home/p.png",
                &mut work
            ),
            Err(ExportError::NoIcon)
        );
        let mut files = installed(Some(&icon_file()[1..]));
        assert_eq!(
            export(
                &mut files,
                b"Paint",
                IconSize::Large,
                b"/home/p.png",
                &mut work
            ),
            Err(ExportError::NoIcon)
        );
        assert_eq!(
            export(
                &mut files,
                b"Files",
                IconSize::Large,
                b"/home/p.png",
                &mut work
            ),
            Err(ExportError::NoIcon)
        );
        assert_eq!(
            export(
                &mut files,
                b"Gimp",
                IconSize::Large,
                b"/home/p.png",
                &mut work
            ),
            Err(ExportError::NotInstalled)
        );
    }
}
