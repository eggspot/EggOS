use crate::{Error, Image, Limits};
use alloc::alloc::{Layout, alloc_zeroed};
use alloc::boxed::Box;
use eggos_png::DecodeError;
use eggos_png::inflate::SliceSource;
use eggos_png::rows::{RowDecoder, RowWorkspace};

fn error(error: DecodeError) -> Error {
    match error {
        DecodeError::Unsupported => Error::Unsupported,
        DecodeError::Truncated => Error::Truncated,
        DecodeError::NotPng
        | DecodeError::BadChunk
        | DecodeError::Corrupt
        | DecodeError::BufferTooSmall => Error::Corrupt,
    }
}

fn workspace() -> Result<Box<RowWorkspace>, Error> {
    let layout = Layout::new::<RowWorkspace>();
    // SAFETY: a `RowWorkspace` is not zero-sized, so its layout is a valid request.
    let block = unsafe { alloc_zeroed(layout) }.cast::<RowWorkspace>();
    if block.is_null() {
        return Err(Error::OutOfMemory);
    }
    // SAFETY: a zeroed block of a `RowWorkspace`'s layout from the global allocator, owned by
    // nothing else, which the box frees with that layout; all zero is a workspace's starting
    // state and its arrays take any bytes.
    Ok(unsafe { Box::from_raw(block) })
}

pub(crate) fn decode(bytes: &[u8], limits: &Limits) -> Result<Image, Error> {
    let mut source = SliceSource::new(bytes);
    let decoder = RowDecoder::open(&mut source).map_err(error)?;
    let header = decoder.header();
    let mut image = crate::canvas(header.width, header.height, limits)?;
    let mut work = workspace()?;
    let width = header.width as usize;
    decoder
        .read_rows(&mut work, |row, pixels| {
            let start = row as usize * width;
            if let Some(target) = image.pixels.get_mut(start..start + width) {
                for (into, &pixel) in target.iter_mut().zip(pixels) {
                    *into = pixel;
                }
            }
        })
        .map_err(error)?;
    Ok(image)
}
