//! A compiled layout, `res/layout/*.xml` in binary XML (D500): its elements as a tree of views,
//! each with the attributes the Android layer shows (an id, a text given literally or as a
//! string resource, a click handler's method name, a linear layout's orientation, an image
//! view's drawable).

use crate::manifest::{half, strings, word};
use alloc::string::String;
use alloc::vec::Vec;

const XML_CHUNK: u16 = 0x0003;
const STRING_POOL: u16 = 0x0001;
const START_ELEMENT: u16 = 0x0102;
const END_ELEMENT: u16 = 0x0103;
const NO_STRING: u32 = u32::MAX;
const TYPE_REFERENCE: u8 = 0x01;
const TYPE_STRING: u8 = 0x03;
const TYPE_INT_DEC: u8 = 0x10;
const TYPE_INT_HEX: u8 = 0x11;
const ATTRIBUTE_BYTES: usize = 20;
const VERTICAL: u32 = 1;
const DEEPEST: usize = 32;

/// A text attribute: literal, or a string resource's id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Text {
    Literal(String),
    Resource(u32),
}

/// One view of a layout and the views inside it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ViewNode {
    /// The element's name: `TextView`, `Button`, `LinearLayout`, or a full class name.
    pub element: String,
    /// `android:id`, a resource id.
    pub id: Option<u32>,
    pub text: Option<Text>,
    /// `android:hint`.
    pub hint: Option<Text>,
    /// `android:onClick`: the activity's method a click calls.
    pub on_click: Option<String>,
    /// `android:orientation`: whether a linear layout stacks its children.
    pub vertical: bool,
    /// `android:src` or `app:srcCompat`: an image view's drawable resource (D509).
    pub image: Option<u32>,
    /// `android:inputType`'s flags (`TYPE_CLASS_NUMBER`, `TYPE_TEXT_VARIATION_PASSWORD`, ...),
    /// D762.
    pub input_type: Option<u32>,
    /// `android:imeOptions`' action (`actionDone` is 6), D762.
    pub ime_action: Option<u32>,
    /// `android:layout_width` and `android:layout_height` (D763).
    pub layout_width: Option<Extent>,
    pub layout_height: Option<Extent>,
    /// `android:scaleType`, as `ImageView.ScaleType`'s ordinal (`fitCenter` is 3), D763.
    pub scale_type: Option<u32>,
    pub children: Vec<ViewNode>,
}

/// A view's width or height in its layout (D763).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Extent {
    MatchParent,
    WrapContent,
    /// A dimension, as binary XML encodes it (see [`dimension_pixels`]).
    Dimension(u32),
}

const TYPE_DIMENSION: u8 = 0x05;
const MATCH_PARENT: u32 = u32::MAX;
const WRAP_CONTENT: u32 = u32::MAX - 1;
const BASE_DENSITY: i64 = 160;
const SCALE_TYPE_NAMES: [&str; 8] = [
    "matrix",
    "fitXY",
    "fitStart",
    "fitCenter",
    "fitEnd",
    "center",
    "centerCrop",
    "centerInside",
];

/// A dimension's size in pixels on a screen of `dpi` dots per inch: its mantissa and radix
/// (`23p0`, `16p7`, `8p15`, `0p23`) and its unit (`px`, `dp`, `sp`, `pt`, `in`, `mm`).
pub fn dimension_pixels(data: u32, dpi: u32) -> i32 {
    let mantissa = i64::from((data as i32) >> 8);
    let fraction_bits = match (data >> 4) & 3 {
        0 => 0,
        1 => 7,
        2 => 15,
        _ => 23,
    };
    let dpi = i64::from(dpi);
    let scaled = match data & 0xf {
        0 => mantissa,
        1 | 2 => mantissa * dpi / BASE_DENSITY,
        3 => mantissa * dpi / 72,
        4 => mantissa * dpi,
        5 => mantissa * dpi * 10 / 254,
        _ => mantissa,
    };
    (scaled >> fraction_bits) as i32
}

fn extent(attribute: &Attribute) -> Option<Extent> {
    match (attribute.kind, attribute.data, attribute.text.as_deref()) {
        (TYPE_DIMENSION, data, _) => Some(Extent::Dimension(data)),
        (TYPE_INT_DEC | TYPE_INT_HEX, MATCH_PARENT, _) => Some(Extent::MatchParent),
        (TYPE_INT_DEC | TYPE_INT_HEX, WRAP_CONTENT, _) => Some(Extent::WrapContent),
        (_, _, Some("match_parent" | "fill_parent")) => Some(Extent::MatchParent),
        (_, _, Some("wrap_content")) => Some(Extent::WrapContent),
        _ => None,
    }
}

const INPUT_TYPE_NAMES: [(&str, u32); 10] = [
    ("text", 0x1),
    ("textPassword", 0x81),
    ("textVisiblePassword", 0x91),
    ("textWebPassword", 0xe1),
    ("textEmailAddress", 0x21),
    ("textMultiLine", 0x2_0001),
    ("number", 0x2),
    ("numberSigned", 0x1002),
    ("numberDecimal", 0x2002),
    ("phone", 0x3),
];

const IME_ACTION_NAMES: [(&str, u32); 6] = [
    ("actionGo", 2),
    ("actionSearch", 3),
    ("actionSend", 4),
    ("actionNext", 5),
    ("actionDone", 6),
    ("actionPrevious", 7),
];

fn named_flags(text: Option<&str>, names: &[(&str, u32)]) -> Option<u32> {
    let mut flags = 0;
    for part in text?.split('|') {
        flags |= names.iter().find(|(name, _)| *name == part)?.1;
    }
    Some(flags)
}

struct Attribute {
    name: String,
    text: Option<String>,
    kind: u8,
    data: u32,
}

fn attributes(chunk: &[u8], pool: &[String]) -> Option<Vec<Attribute>> {
    let start = usize::from(half(chunk, 24)?);
    let size = usize::from(half(chunk, 26)?).max(ATTRIBUTE_BYTES);
    let count = usize::from(half(chunk, 28)?);
    let mut found = Vec::with_capacity(count.min(64));
    for index in 0..count {
        let at = 16 + start + index * size;
        let raw = word(chunk, at + 8)?;
        let kind = *chunk.get(at + 15)?;
        let data = word(chunk, at + 16)?;
        let text = match (raw, kind) {
            (raw, _) if raw != NO_STRING => pool.get(raw as usize).cloned(),
            (_, TYPE_STRING) => pool.get(data as usize).cloned(),
            _ => None,
        };
        found.push(Attribute {
            name: pool
                .get(word(chunk, at + 4)? as usize)
                .cloned()
                .unwrap_or_default(),
            text,
            kind,
            data,
        });
    }
    Some(found)
}

fn text_of(attribute: &Attribute) -> Option<Text> {
    match attribute.kind {
        TYPE_REFERENCE => Some(Text::Resource(attribute.data)),
        _ => attribute.text.clone().map(Text::Literal),
    }
}

fn node(name: String, attributes: &[Attribute]) -> ViewNode {
    let mut view = ViewNode {
        element: name,
        ..ViewNode::default()
    };
    for attribute in attributes {
        match attribute.name.as_str() {
            "id" if attribute.kind == TYPE_REFERENCE => view.id = Some(attribute.data),
            "text" => view.text = text_of(attribute),
            "hint" => view.hint = text_of(attribute),
            "onClick" => view.on_click = attribute.text.clone(),
            "src" | "srcCompat" if attribute.kind == TYPE_REFERENCE => {
                view.image = Some(attribute.data)
            }
            "layout_width" => view.layout_width = extent(attribute),
            "layout_height" => view.layout_height = extent(attribute),
            "scaleType" => {
                view.scale_type = match attribute.kind {
                    TYPE_INT_DEC | TYPE_INT_HEX => Some(attribute.data),
                    _ => attribute
                        .text
                        .as_deref()
                        .and_then(|name| SCALE_TYPE_NAMES.iter().position(|known| *known == name))
                        .map(|ordinal| ordinal as u32),
                }
            }
            "inputType" => {
                view.input_type = match attribute.kind {
                    TYPE_INT_DEC | TYPE_INT_HEX => Some(attribute.data),
                    _ => named_flags(attribute.text.as_deref(), &INPUT_TYPE_NAMES),
                }
            }
            "imeOptions" => {
                view.ime_action = match attribute.kind {
                    TYPE_INT_DEC | TYPE_INT_HEX => Some(attribute.data & 0xff),
                    _ => named_flags(attribute.text.as_deref(), &IME_ACTION_NAMES),
                }
            }
            "orientation" => {
                view.vertical = match attribute.kind {
                    TYPE_INT_DEC | TYPE_INT_HEX => attribute.data == VERTICAL,
                    _ => attribute.text.as_deref() == Some("vertical"),
                }
            }
            _ => {}
        }
    }
    view
}

/// The root view of the binary XML layout `xml`; `None` when it is not one.
pub fn parse(xml: &[u8]) -> Option<ViewNode> {
    if half(xml, 0)? != XML_CHUNK {
        return None;
    }
    let mut at = usize::from(half(xml, 2)?);
    let mut pool = Vec::new();
    let mut open: Vec<ViewNode> = Vec::new();
    let mut root = None;
    while at + 8 <= xml.len() {
        let kind = half(xml, at)?;
        let size = word(xml, at + 4)? as usize;
        if size < 8 {
            return None;
        }
        let chunk = xml.get(at..at + size)?;
        match kind {
            STRING_POOL => pool = strings(chunk)?,
            START_ELEMENT if open.len() < DEEPEST => {
                let name = pool.get(word(chunk, 20)? as usize)?.clone();
                open.push(node(name, &attributes(chunk, &pool)?));
            }
            END_ELEMENT => {
                let finished = open.pop()?;
                match open.last_mut() {
                    Some(parent) => parent.children.push(finished),
                    None => root = Some(finished),
                }
            }
            _ => {}
        }
        at += size;
    }
    root
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn pool(strings: &[&str]) -> Vec<u8> {
        let mut data = Vec::new();
        let mut offsets = Vec::new();
        for text in strings {
            offsets.push(data.len() as u32);
            data.push(text.len() as u8);
            data.push(text.len() as u8);
            data.extend_from_slice(text.as_bytes());
            data.push(0);
        }
        while data.len() % 4 != 0 {
            data.push(0);
        }
        let header = 28;
        let start = header + 4 * strings.len();
        let mut chunk = Vec::new();
        chunk.extend_from_slice(&STRING_POOL.to_le_bytes());
        chunk.extend_from_slice(&(header as u16).to_le_bytes());
        chunk.extend_from_slice(&((start + data.len()) as u32).to_le_bytes());
        chunk.extend_from_slice(&(strings.len() as u32).to_le_bytes());
        chunk.extend_from_slice(&0u32.to_le_bytes());
        chunk.extend_from_slice(&(1u32 << 8).to_le_bytes());
        chunk.extend_from_slice(&(start as u32).to_le_bytes());
        chunk.extend_from_slice(&0u32.to_le_bytes());
        for offset in offsets {
            chunk.extend_from_slice(&offset.to_le_bytes());
        }
        chunk.extend_from_slice(&data);
        chunk
    }

    fn element(kind: u16, name: u32, attributes: &[(u32, u32, u8, u32)]) -> Vec<u8> {
        let mut chunk = Vec::new();
        chunk.extend_from_slice(&kind.to_le_bytes());
        chunk.extend_from_slice(&16u16.to_le_bytes());
        let size = 36 + attributes.len() * 20;
        chunk.extend_from_slice(&(size as u32).to_le_bytes());
        chunk.extend_from_slice(&[0; 8]);
        chunk.extend_from_slice(&NO_STRING.to_le_bytes());
        chunk.extend_from_slice(&name.to_le_bytes());
        chunk.extend_from_slice(&20u16.to_le_bytes());
        chunk.extend_from_slice(&20u16.to_le_bytes());
        chunk.extend_from_slice(&(attributes.len() as u16).to_le_bytes());
        chunk.extend_from_slice(&[0; 6]);
        for &(attribute, raw, kind, data) in attributes {
            chunk.extend_from_slice(&NO_STRING.to_le_bytes());
            chunk.extend_from_slice(&attribute.to_le_bytes());
            chunk.extend_from_slice(&raw.to_le_bytes());
            chunk.extend_from_slice(&[8, 0, 0, kind]);
            chunk.extend_from_slice(&data.to_le_bytes());
        }
        chunk
    }

    #[test]
    fn a_linear_layout_with_a_text_and_a_button_reads_back_as_a_tree() {
        let strings = pool(&[
            "LinearLayout",
            "orientation",
            "TextView",
            "text",
            "Button",
            "id",
            "onClick",
            "greet",
        ]);
        let mut body = strings;
        body.extend(element(
            START_ELEMENT,
            0,
            &[(1, NO_STRING, TYPE_INT_DEC, 1)],
        ));
        body.extend(element(
            START_ELEMENT,
            2,
            &[(3, NO_STRING, TYPE_REFERENCE, 0x7f0f_0001)],
        ));
        body.extend(element(END_ELEMENT, 2, &[]));
        body.extend(element(
            START_ELEMENT,
            4,
            &[
                (5, NO_STRING, TYPE_REFERENCE, 0x7f08_0002),
                (6, 7, TYPE_STRING, 7),
            ],
        ));
        body.extend(element(END_ELEMENT, 4, &[]));
        body.extend(element(END_ELEMENT, 0, &[]));
        let mut xml = vec![0x03, 0x00, 0x08, 0x00];
        xml.extend_from_slice(&((8 + body.len()) as u32).to_le_bytes());
        xml.extend(body);
        let root = parse(&xml).unwrap();
        assert_eq!(root.element, "LinearLayout");
        assert!(root.vertical);
        assert_eq!(root.children.len(), 2);
        assert_eq!(root.children[0].text, Some(Text::Resource(0x7f0f_0001)));
        assert_eq!(root.children[1].id, Some(0x7f08_0002));
        assert_eq!(root.children[1].on_click.as_deref(), Some("greet"));
    }

    #[test]
    fn dimensions_become_pixels_at_the_screen_s_density() {
        let dp = |value: i32| ((value as u32) << 8) | 1;
        assert_eq!(dimension_pixels(dp(48), 160), 48);
        assert_eq!(dimension_pixels(dp(48), 320), 96);
        assert_eq!(dimension_pixels(10 << 8, 320), 10);
        assert_eq!(dimension_pixels(((3 << 7) << 8) | (1 << 4) | 1, 160), 3);
    }
}
