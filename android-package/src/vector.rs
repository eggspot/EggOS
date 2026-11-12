//! A compiled vector drawable, `res/drawable/*.xml` in binary XML (D518): its size in
//! density-independent pixels, its viewport, and its paths, each a path's data in SVG's path
//! syntax with its fill colour (literal, or a colour resource to look up) and fill type.

use crate::manifest::{half, strings, word};
use alloc::string::String;
use alloc::vec::Vec;

const XML_CHUNK: u16 = 0x0003;
const STRING_POOL: u16 = 0x0001;
const START_ELEMENT: u16 = 0x0102;
const NO_STRING: u32 = u32::MAX;
const TYPE_REFERENCE: u8 = 0x01;
const TYPE_STRING: u8 = 0x03;
const TYPE_FLOAT: u8 = 0x04;
const TYPE_DIMENSION: u8 = 0x05;
const FIRST_COLOUR_TYPE: u8 = 0x1c;
const LAST_COLOUR_TYPE: u8 = 0x1f;
const ATTRIBUTE_BYTES: usize = 20;
const EVEN_ODD: u32 = 1;
const MOST_PATHS: usize = 256;

/// A colour: `0xAARRGGBB`, or a colour resource's id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Colour {
    Value(u32),
    Resource(u32),
}

/// One path of a vector drawable.
#[derive(Debug, Clone, PartialEq)]
pub struct VectorPath {
    pub data: String,
    pub fill: Option<Colour>,
    pub even_odd: bool,
}

/// A vector drawable.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Vector {
    /// Its size, in density-independent pixels.
    pub width: f32,
    pub height: f32,
    /// The coordinate space its paths are drawn in.
    pub viewport_width: f32,
    pub viewport_height: f32,
    pub paths: Vec<VectorPath>,
}

/// A dimension value's number (`TYPE_DIMENSION`): its mantissa scaled by its radix.
fn dimension(data: u32) -> f32 {
    let mantissa = (data as i32 >> 8) as f32;
    let shift = match data >> 4 & 3 {
        0 => 0,
        1 => 7,
        2 => 15,
        _ => 23,
    };
    mantissa / (1u32 << shift) as f32
}

/// The vector drawable in binary XML `xml`; `None` when it is not one.
pub fn parse(xml: &[u8]) -> Option<Vector> {
    if half(xml, 0)? != XML_CHUNK {
        return None;
    }
    let mut at = usize::from(half(xml, 2)?);
    let mut pool = Vec::new();
    let mut vector = Vector::default();
    let mut found = false;
    while at + 8 <= xml.len() {
        let kind = half(xml, at)?;
        let size = word(xml, at + 4)? as usize;
        if size < 8 {
            return None;
        }
        let chunk = xml.get(at..at + size)?;
        match kind {
            STRING_POOL => pool = strings(chunk)?,
            START_ELEMENT => {
                let name = pool.get(word(chunk, 20)? as usize)?.as_str();
                let start = usize::from(half(chunk, 24)?);
                let attribute_size = usize::from(half(chunk, 26)?).max(ATTRIBUTE_BYTES);
                let count = usize::from(half(chunk, 28)?);
                let mut path = VectorPath {
                    data: String::new(),
                    fill: None,
                    even_odd: false,
                };
                for index in 0..count {
                    let place = 16 + start + index * attribute_size;
                    let attribute = pool
                        .get(word(chunk, place + 4)? as usize)
                        .map_or("", String::as_str);
                    let raw = word(chunk, place + 8)?;
                    let kind = *chunk.get(place + 15)?;
                    let data = word(chunk, place + 16)?;
                    let text = match (raw, kind) {
                        (raw, _) if raw != NO_STRING => pool.get(raw as usize).cloned(),
                        (_, TYPE_STRING) => pool.get(data as usize).cloned(),
                        _ => None,
                    };
                    let number = match kind {
                        TYPE_DIMENSION => dimension(data),
                        TYPE_FLOAT => f32::from_bits(data),
                        _ => text.as_deref().and_then(|text| text.parse().ok()).unwrap_or(0.0),
                    };
                    match (name, attribute) {
                        ("vector", "width") => vector.width = number,
                        ("vector", "height") => vector.height = number,
                        ("vector", "viewportWidth") => vector.viewport_width = number,
                        ("vector", "viewportHeight") => vector.viewport_height = number,
                        ("path", "pathData") => path.data = text.unwrap_or_default(),
                        ("path", "fillColor") => {
                            path.fill = match kind {
                                TYPE_REFERENCE => Some(Colour::Resource(data)),
                                FIRST_COLOUR_TYPE..=LAST_COLOUR_TYPE => Some(Colour::Value(data)),
                                _ => None,
                            }
                        }
                        ("path", "fillType") => path.even_odd = data == EVEN_ODD,
                        _ => {}
                    }
                }
                match name {
                    "vector" => found = true,
                    "path" if vector.paths.len() < MOST_PATHS => vector.paths.push(path),
                    _ => {}
                }
            }
            _ => {}
        }
        at += size;
    }
    found.then_some(vector)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dimensions_scale_their_mantissa_by_their_radix() {
        assert_eq!(dimension(24 << 8 | 1), 24.0);
        assert_eq!(dimension(3 << 8 | 1 << 4), 3.0 / 128.0);
    }
}
