use crate::bytes::le16;
use crate::lzw::{self, Flavour, Table};
use crate::{Animation, Error, Frame, Image, Limits, OPAQUE};
use alloc::vec::Vec;

const HEADER: usize = 13;
const EXTENSION: u8 = 0x21;
const IMAGE: u8 = 0x2c;
const TRAILER: u8 = 0x3b;
const GRAPHIC_CONTROL: u8 = 0xf9;
const APPLICATION: u8 = 0xff;
const LOOPING_NAMES: [&[u8]; 2] = [b"NETSCAPE2.0", b"ANIMEXTS1.0"];
const LOOPING_BLOCK: u8 = 1;
const TABLE_FLAG: u8 = 0x80;
const INTERLACED_FLAG: u8 = 0x40;
const TRANSPARENT_FLAG: u8 = 0x01;
const DISPOSAL_SHIFT: u8 = 2;
const DISPOSAL_MASK: u8 = 0x07;
const MILLISECONDS_PER_UNIT: u32 = 10;
const INTERLACE_PASSES: [(usize, usize); 4] = [(0, 8), (4, 8), (2, 4), (1, 2)];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Disposal {
    Keep,
    Background,
    Previous,
}

#[derive(Debug, Clone, Copy)]
struct Control {
    disposal: Disposal,
    delay_ms: u32,
    transparent: Option<u8>,
}

impl Control {
    const NONE: Control = Control {
        disposal: Disposal::Keep,
        delay_ms: 0,
        transparent: None,
    };
}

struct Palette {
    colours: [u32; 256],
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn byte(&mut self) -> Result<u8, Error> {
        let value = *self.bytes.get(self.at).ok_or(Error::Truncated)?;
        self.at += 1;
        Ok(value)
    }

    fn word(&mut self) -> Result<u16, Error> {
        let value = le16(self.bytes, self.at)?;
        self.at += 2;
        Ok(value)
    }

    fn take(&mut self, count: usize) -> Result<&'a [u8], Error> {
        let end = self.at.checked_add(count).ok_or(Error::Truncated)?;
        let taken = self.bytes.get(self.at..end).ok_or(Error::Truncated)?;
        self.at = end;
        Ok(taken)
    }

    fn block(&mut self) -> Result<Option<&'a [u8]>, Error> {
        let size = usize::from(self.byte()?);
        match size {
            0 => Ok(None),
            size => self.take(size).map(Some),
        }
    }

    fn skip_blocks(&mut self) -> Result<(), Error> {
        while self.block()?.is_some() {}
        Ok(())
    }

    fn palette(&mut self, flags: u8) -> Result<Palette, Error> {
        let count = 2usize << (flags & 0x07);
        let table = self.take(3 * count)?;
        let mut colours = [OPAQUE; 256];
        for (colour, rgb) in colours.iter_mut().zip(table.as_chunks::<3>().0) {
            *colour = crate::rgb(rgb[0], rgb[1], rgb[2]);
        }
        Ok(Palette { colours })
    }
}

fn control(reader: &mut Reader<'_>) -> Result<Control, Error> {
    let Some(fields) = reader.block()? else {
        return Ok(Control::NONE);
    };
    let found = match fields {
        [flags, low, high, transparent, ..] => Control {
            disposal: match flags >> DISPOSAL_SHIFT & DISPOSAL_MASK {
                2 => Disposal::Background,
                3 => Disposal::Previous,
                _ => Disposal::Keep,
            },
            delay_ms: u32::from(u16::from_le_bytes([*low, *high])) * MILLISECONDS_PER_UNIT,
            transparent: (flags & TRANSPARENT_FLAG != 0).then_some(*transparent),
        },
        _ => Control::NONE,
    };
    reader.skip_blocks()?;
    Ok(found)
}

fn loop_count(reader: &mut Reader<'_>) -> Result<Option<u16>, Error> {
    let looping = reader
        .block()?
        .is_some_and(|name| LOOPING_NAMES.contains(&name));
    let mut count = None;
    while let Some(data) = reader.block()? {
        if let [LOOPING_BLOCK, low, high, ..] = data
            && looping
        {
            count = Some(u16::from_le_bytes([*low, *high]));
        }
    }
    Ok(count)
}

fn image_data(reader: &mut Reader<'_>, data: &mut Vec<u8>) {
    data.clear();
    while let Ok(Some(block)) = reader.block() {
        if data.try_reserve(block.len()).is_err() {
            return;
        }
        data.extend_from_slice(block);
    }
}

fn stored_row(index: usize, height: usize, interlaced: bool) -> usize {
    if !interlaced {
        return index;
    }
    let mut left = index;
    for (start, step) in INTERLACE_PASSES {
        let rows = height.saturating_sub(start).div_ceil(step);
        if left < rows {
            return start + left * step;
        }
        left -= rows;
    }
    height
}

struct Placement {
    left: usize,
    top: usize,
    width: usize,
    height: usize,
}

struct Canvas {
    width: usize,
    height: usize,
    pixels: Vec<u32>,
}

impl Canvas {
    fn draw(
        &mut self,
        place: &Placement,
        indices: &[u8],
        palette: &Palette,
        control: Control,
        interlaced: bool,
    ) {
        if place.width == 0 {
            return;
        }
        for (index, row) in indices.chunks(place.width).enumerate() {
            let y = place.top + stored_row(index, place.height, interlaced);
            if y >= self.height {
                continue;
            }
            for (x, &colour) in row.iter().enumerate() {
                let x = place.left + x;
                if x >= self.width || control.transparent == Some(colour) {
                    continue;
                }
                self.pixels[y * self.width + x] = palette.colours[usize::from(colour)];
            }
        }
    }

    fn clear(&mut self, place: &Placement) {
        let right = (place.left + place.width).min(self.width);
        for y in place.top..(place.top + place.height).min(self.height) {
            let start = y * self.width;
            if let Some(row) = self.pixels.get_mut(start + place.left..start + right) {
                row.fill(0);
            }
        }
    }

    fn snapshot(&self) -> Result<Vec<u32>, Error> {
        let mut copy = crate::buffer(self.pixels.len(), 0)?;
        copy.copy_from_slice(&self.pixels);
        Ok(copy)
    }
}

pub(crate) fn decode(bytes: &[u8], limits: &Limits, every_frame: bool) -> Result<Animation, Error> {
    if bytes.len() < HEADER {
        return Err(Error::Truncated);
    }
    let width = u32::from(le16(bytes, 6)?);
    let height = u32::from(le16(bytes, 8)?);
    let flags = bytes[10];
    let image = crate::canvas(width, height, limits)?;
    let mut canvas = Canvas {
        width: width as usize,
        height: height as usize,
        pixels: image.pixels,
    };
    let mut reader = Reader { bytes, at: HEADER };
    let global = match flags & TABLE_FLAG {
        0 => None,
        _ => Some(reader.palette(flags)?),
    };
    let per_frame = u64::from(width) * u64::from(height);
    let mut frames: Vec<Frame> = Vec::new();
    let mut loops = None;
    let mut pending = Control::NONE;
    let mut table = Table::new()?;
    let mut data = Vec::new();
    let mut indices = Vec::new();
    while let Ok(introducer) = reader.byte() {
        match introducer {
            EXTENSION => match reader.byte()? {
                GRAPHIC_CONTROL => pending = control(&mut reader)?,
                APPLICATION => {
                    if let Some(count) = loop_count(&mut reader)? {
                        loops = Some(count);
                    }
                }
                _ => reader.skip_blocks()?,
            },
            IMAGE => {
                let kept = (frames.len() as u64).saturating_mul(per_frame);
                let full = frames.len() >= limits.max_frames
                    || kept.saturating_add(per_frame) > limits.max_pixels;
                if !frames.is_empty() && full {
                    break;
                }
                let place = Placement {
                    left: usize::from(reader.word()?),
                    top: usize::from(reader.word()?),
                    width: usize::from(reader.word()?),
                    height: usize::from(reader.word()?),
                };
                let flags = reader.byte()?;
                let local = match flags & TABLE_FLAG {
                    0 => None,
                    _ => Some(reader.palette(flags)?),
                };
                let palette = local.as_ref().or(global.as_ref()).ok_or(Error::Corrupt)?;
                let minimum = u32::from(reader.byte()?);
                let count = place.width * place.height;
                if count as u64 > limits.max_pixels {
                    return Err(Error::TooLarge);
                }
                image_data(&mut reader, &mut data);
                indices.clear();
                indices
                    .try_reserve_exact(count)
                    .map_err(|_| Error::OutOfMemory)?;
                indices.resize(count, 0);
                let written = match count {
                    0 => 0,
                    _ => lzw::decode(&data, minimum, Flavour::Gif, &mut table, &mut indices)?,
                };
                let control = pending;
                pending = Control::NONE;
                let before = match control.disposal {
                    Disposal::Previous => Some(canvas.snapshot()?),
                    _ => None,
                };
                let interlaced = flags & INTERLACED_FLAG != 0;
                canvas.draw(&place, &indices[..written], palette, control, interlaced);
                frames.try_reserve(1).map_err(|_| Error::OutOfMemory)?;
                let pixels = match every_frame {
                    true => canvas.snapshot()?,
                    false => core::mem::take(&mut canvas.pixels),
                };
                frames.push(Frame {
                    image: Image {
                        width,
                        height,
                        pixels,
                    },
                    delay_ms: control.delay_ms,
                });
                if !every_frame {
                    break;
                }
                match (control.disposal, before) {
                    (Disposal::Background, _) => canvas.clear(&place),
                    (Disposal::Previous, Some(before)) => canvas.pixels = before,
                    _ => {}
                }
            }
            TRAILER => break,
            _ if frames.is_empty() => return Err(Error::Corrupt),
            _ => break,
        }
    }
    if frames.is_empty() {
        return Err(Error::Truncated);
    }
    Ok(Animation {
        frames,
        loop_count: loops,
    })
}
