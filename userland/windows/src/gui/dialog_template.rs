//! Dialog templates (D178, `docs/ux/windows-controls-v0.3.md` section 11.4): `DLGTEMPLATE` and
//! `DLGTEMPLATEEX`, from an `RT_DIALOG` resource or from the program's memory, read field by
//! field from any [`TemplateBytes`]: the dialog's styles, size and texts, then each control's,
//! every control starting on a 4-byte boundary. A name or text is absent, a 16-bit ordinal
//! (`0xffff` then the number) or a NUL-terminated UTF-16 string.

/// Where a template's bytes are read from: a slice, or the program's memory.
pub trait TemplateBytes {
    /// The byte at offset `at`, `None` past the end.
    fn byte(&self, at: usize) -> Option<u8>;
}

impl TemplateBytes for [u8] {
    fn byte(&self, at: usize) -> Option<u8> {
        self.get(at).copied()
    }
}

/// The dialog style that says a font follows the title.
pub const DS_SETFONT: u32 = 0x40;
const EXTENDED_SIGNATURE: u16 = 0xffff;
const ORDINAL: u16 = 0xffff;
const LONGEST_TEXT: usize = 0x1_0000;

/// A name or text of a template.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Absent,
    Ordinal(u16),
    /// A string of `length` units at offset `at`.
    Text {
        at: usize,
        length: usize,
    },
}

/// A template's dialog: its styles, its control count, its rectangle in dialog units, its
/// menu, class and title, and where its first control starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dialog {
    pub extended: bool,
    pub style: u32,
    pub ex_style: u32,
    pub count: u16,
    pub x: i16,
    pub y: i16,
    pub width: i16,
    pub height: i16,
    pub menu: Field,
    pub class: Field,
    pub title: Field,
    pub first_item: usize,
}

/// One control of a template, in dialog units, with where the next one starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Item {
    pub style: u32,
    pub ex_style: u32,
    pub x: i16,
    pub y: i16,
    pub width: i16,
    pub height: i16,
    pub id: u32,
    pub class: Field,
    pub title: Field,
    /// Where the creation data starts (its size word, which `WM_CREATE`'s `lpCreateParams`
    /// points at) and how many bytes follow that word.
    pub data: Option<(usize, usize)>,
    pub next: usize,
}

struct Reader<'a, B: TemplateBytes + ?Sized> {
    bytes: &'a B,
    at: usize,
}

impl<B: TemplateBytes + ?Sized> Reader<'_, B> {
    fn word(&mut self) -> Option<u16> {
        let low = self.bytes.byte(self.at)?;
        let high = self.bytes.byte(self.at + 1)?;
        self.at += 2;
        Some(u16::from_le_bytes([low, high]))
    }

    fn double(&mut self) -> Option<u32> {
        let low = self.word()?;
        let high = self.word()?;
        Some(u32::from(low) | (u32::from(high) << 16))
    }

    fn short(&mut self) -> Option<i16> {
        self.word().map(|word| word as i16)
    }

    fn align(&mut self, to: usize) {
        self.at = self.at.next_multiple_of(to);
    }

    fn text(&mut self) -> Option<Field> {
        let at = self.at;
        for length in 0..LONGEST_TEXT {
            if self.word()? == 0 {
                return Some(Field::Text { at, length });
            }
        }
        None
    }

    fn field(&mut self) -> Option<Field> {
        let at = self.at;
        match self.word()? {
            0 => Some(Field::Absent),
            ORDINAL => self.word().map(Field::Ordinal),
            _ => {
                self.at = at;
                self.text()
            }
        }
    }
}

/// The dialog of the template in `bytes`.
pub fn dialog<B: TemplateBytes + ?Sized>(bytes: &B) -> Option<Dialog> {
    let mut reader = Reader { bytes, at: 0 };
    let version = reader.word()?;
    let signature = reader.word()?;
    let extended = version == 1 && signature == EXTENDED_SIGNATURE;
    let (style, ex_style) = match extended {
        true => {
            reader.double()?;
            let ex_style = reader.double()?;
            (reader.double()?, ex_style)
        }
        false => {
            reader.at = 0;
            (reader.double()?, reader.double()?)
        }
    };
    let count = reader.word()?;
    let (x, y, width, height) = (
        reader.short()?,
        reader.short()?,
        reader.short()?,
        reader.short()?,
    );
    let menu = reader.field()?;
    let class = reader.field()?;
    let title = reader.text()?;
    if style & DS_SETFONT != 0 {
        reader.word()?;
        if extended {
            reader.word()?;
            reader.word()?;
        }
        reader.text()?;
    }
    reader.align(4);
    Some(Dialog {
        extended,
        style,
        ex_style,
        count,
        x,
        y,
        width,
        height,
        menu,
        class,
        title,
        first_item: reader.at,
    })
}

/// The control of an extended (`DLGTEMPLATEEX`) or plain template that starts at `at`.
pub fn item<B: TemplateBytes + ?Sized>(bytes: &B, at: usize, extended: bool) -> Option<Item> {
    let mut reader = Reader { bytes, at };
    let (style, ex_style) = match extended {
        true => {
            reader.double()?;
            let ex_style = reader.double()?;
            (reader.double()?, ex_style)
        }
        false => (reader.double()?, reader.double()?),
    };
    let (x, y, width, height) = (
        reader.short()?,
        reader.short()?,
        reader.short()?,
        reader.short()?,
    );
    let id = match extended {
        true => reader.double()?,
        false => u32::from(reader.word()?),
    };
    let class = reader.field()?;
    let title = reader.field()?;
    let size_at = reader.at;
    let size = usize::from(reader.word()?);
    let data = (size > 0).then_some((size_at, size));
    reader.at += size;
    reader.align(4);
    Some(Item {
        style,
        ex_style,
        x,
        y,
        width,
        height,
        id,
        class,
        title,
        data,
        next: reader.at,
    })
}

/// Copies text `field` into `out`, cut to fit; the units copied (0 for an absent field or an
/// ordinal).
pub fn copy_text<B: TemplateBytes + ?Sized>(bytes: &B, field: Field, out: &mut [u16]) -> usize {
    let Field::Text { at, length } = field else {
        return 0;
    };
    let mut copied = 0;
    for (index, slot) in out.iter_mut().take(length).enumerate() {
        let Some(low) = bytes.byte(at + 2 * index) else {
            break;
        };
        let Some(high) = bytes.byte(at + 2 * index + 1) else {
            break;
        };
        *slot = u16::from_le_bytes([low, high]);
        copied += 1;
    }
    copied
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Builder(Vec<u8>);

    impl Builder {
        fn word(&mut self, value: u16) -> &mut Self {
            self.0.extend_from_slice(&value.to_le_bytes());
            self
        }

        fn double(&mut self, value: u32) -> &mut Self {
            self.0.extend_from_slice(&value.to_le_bytes());
            self
        }

        fn text(&mut self, text: &str) -> &mut Self {
            for unit in text.encode_utf16() {
                self.word(unit);
            }
            self.word(0)
        }

        fn align(&mut self) -> &mut Self {
            while !self.0.len().is_multiple_of(4) {
                self.0.push(0);
            }
            self
        }
    }

    fn text_of(bytes: &[u8], field: Field) -> String {
        let mut out = [0u16; 64];
        let length = copy_text(bytes, field, &mut out);
        String::from_utf16(&out[..length]).unwrap()
    }

    #[test]
    fn a_plain_template_gives_its_dialog_and_controls() {
        let mut template = Builder(Vec::new());
        template
            .double(0x80c8_0000 | DS_SETFONT)
            .double(0)
            .word(2)
            .word(10)
            .word(20)
            .word(200)
            .word(100)
            .word(0)
            .word(0)
            .text("Find")
            .word(8)
            .text("MS Shell Dlg")
            .align();
        let first = template.0.len();
        template
            .double(0x5001_0000)
            .double(0)
            .word(7)
            .word(9)
            .word(50)
            .word(14)
            .word(1)
            .word(0xffff)
            .word(0x80)
            .text("OK")
            .word(0)
            .align();
        let second = template.0.len();
        template
            .double(0x5081_0080)
            .double(0x200)
            .word(60)
            .word(9)
            .word(80)
            .word(12)
            .word(1001)
            .text("MyEdit")
            .word(0xffff)
            .word(5)
            .word(2)
            .word(0xabcd)
            .align();
        let bytes = &template.0[..];
        let dialog = dialog(bytes).unwrap();
        assert!(!dialog.extended);
        assert_eq!(dialog.style, 0x80c8_0040);
        assert_eq!(dialog.count, 2);
        assert_eq!(
            (dialog.x, dialog.y, dialog.width, dialog.height),
            (10, 20, 200, 100)
        );
        assert_eq!((dialog.menu, dialog.class), (Field::Absent, Field::Absent));
        assert_eq!(text_of(bytes, dialog.title), "Find");
        assert_eq!(dialog.first_item, first);
        let ok = item(bytes, dialog.first_item, false).unwrap();
        assert_eq!(ok.id, 1);
        assert_eq!(ok.class, Field::Ordinal(0x80));
        assert_eq!(text_of(bytes, ok.title), "OK");
        assert_eq!(ok.data, None);
        assert_eq!(ok.next, second);
        let edit = item(bytes, ok.next, false).unwrap();
        assert_eq!(
            (edit.style, edit.ex_style, edit.id),
            (0x5081_0080, 0x200, 1001)
        );
        assert_eq!(text_of(bytes, edit.class), "MyEdit");
        assert_eq!(edit.title, Field::Ordinal(5));
        assert_eq!(edit.data.map(|(_, size)| size), Some(2));
        assert_eq!(edit.next, bytes.len());
        assert_eq!(item(bytes, edit.next, false), None);
    }

    #[test]
    fn an_extended_template_has_help_ids_wide_ids_and_a_longer_font() {
        let mut template = Builder(Vec::new());
        template
            .word(1)
            .word(0xffff)
            .double(0)
            .double(0x8)
            .double(0x80cc_0a48)
            .word(1)
            .word(0)
            .word(0)
            .word(210)
            .word(83)
            .word(0xffff)
            .word(300)
            .word(0)
            .text("Dialog")
            .word(9)
            .word(400)
            .word(0x0001)
            .text("Segoe UI")
            .align();
        template
            .double(0)
            .double(0)
            .double(0x5000_0000)
            .word(5)
            .word(5)
            .word(40)
            .word(8)
            .double(70_000)
            .word(0xffff)
            .word(0x82)
            .text("Prompt")
            .word(0)
            .align();
        let bytes = &template.0[..];
        let dialog = dialog(bytes).unwrap();
        assert!(dialog.extended);
        assert_eq!((dialog.style, dialog.ex_style), (0x80cc_0a48, 0x8));
        assert_eq!(dialog.menu, Field::Ordinal(300));
        assert_eq!(text_of(bytes, dialog.title), "Dialog");
        let label = item(bytes, dialog.first_item, true).unwrap();
        assert_eq!(label.id, 70_000);
        assert_eq!(label.class, Field::Ordinal(0x82));
        assert_eq!(text_of(bytes, label.title), "Prompt");
        let mut short = [0u16; 3];
        assert_eq!(copy_text(bytes, label.title, &mut short), 3);
        assert_eq!(copy_text(bytes, label.class, &mut short), 0);
    }

    #[test]
    fn a_truncated_template_is_refused() {
        assert_eq!(dialog(&[1u8, 0, 0xff][..]), None);
        let mut template = Builder(Vec::new());
        template
            .double(0)
            .double(0)
            .word(1)
            .word(0)
            .word(0)
            .word(9)
            .word(9)
            .word(0)
            .word(0);
        template.word(0x41);
        assert_eq!(dialog(&template.0[..]), None);
    }
}
