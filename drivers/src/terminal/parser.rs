use super::{Attributes, Terminal, cell_flags, colours};
use crate::font;

pub(super) const MAX_PARAMETERS: usize = 16;
pub(super) const MAX_STRING_BYTES: u16 = 1024;

const BEL: u8 = 0x07;
const BS: u8 = 0x08;
const HT: u8 = 0x09;
const LF: u8 = 0x0a;
const VT: u8 = 0x0b;
const FF: u8 = 0x0c;
const CR: u8 = 0x0d;
const CAN: u8 = 0x18;
const SUB: u8 = 0x1a;
const ESC: u8 = 0x1b;
const DEL: u8 = 0x7f;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Ground,
    Escape,
    EscapeIntermediate,
    Csi,
    CsiIgnore,
    String,
    StringEscape,
}

#[derive(Debug, Clone, Copy)]
struct Parameters {
    values: [u16; MAX_PARAMETERS],
    count: usize,
}

impl Parameters {
    const NONE: Parameters = Parameters {
        values: [0; MAX_PARAMETERS],
        count: 0,
    };

    fn raw(&self, index: usize) -> u16 {
        if index < self.count {
            self.values[index]
        } else {
            0
        }
    }

    fn or(&self, index: usize, default: u16) -> u16 {
        match self.raw(index) {
            0 => default,
            value => value,
        }
    }
}

#[derive(Debug)]
pub(super) struct Parser {
    state: State,
    parameters: Parameters,
    private: u8,
    intermediate: u8,
    string_length: u16,
    utf8_codepoint: u32,
    utf8_length: u8,
    utf8_remaining: u8,
}

impl Parser {
    pub(super) const fn new() -> Self {
        Self {
            state: State::Ground,
            parameters: Parameters::NONE,
            private: 0,
            intermediate: 0,
            string_length: 0,
            utf8_codepoint: 0,
            utf8_length: 0,
            utf8_remaining: 0,
        }
    }
}

#[inline]
pub(super) fn feed(terminal: &mut Terminal, byte: u8) {
    match terminal.parser.state {
        State::Ground => ground(terminal, byte),
        State::Escape => escape(terminal, byte),
        State::EscapeIntermediate => escape_intermediate(terminal, byte),
        State::Csi => csi(terminal, byte),
        State::CsiIgnore => csi_ignore(terminal, byte),
        State::String => string(terminal, byte),
        State::StringEscape => string_escape(terminal, byte),
    }
}

#[inline]
fn ground(terminal: &mut Terminal, byte: u8) {
    if terminal.parser.utf8_remaining == 0 {
        match byte {
            0x20..=0x7e => return terminal.print(byte),
            DEL => return,
            0x00..=0x1f => return control(terminal, byte),
            _ => {}
        }
    }
    utf8(terminal, byte);
}

fn control(terminal: &mut Terminal, byte: u8) {
    match byte {
        BS => terminal.backspace(),
        HT => terminal.tab(),
        LF => terminal.line_feed(),
        VT | FF => terminal.index_down(),
        CR => terminal.carriage_return(),
        ESC => {
            terminal.parser.state = State::Escape;
            terminal.parser.intermediate = 0;
        }
        CAN | SUB => terminal.parser.state = State::Ground,
        _ => {}
    }
}

fn replace_bad_bytes(terminal: &mut Terminal, count: u8) {
    for _ in 0..count {
        terminal.print(font::REPLACEMENT);
    }
}

fn utf8(terminal: &mut Terminal, byte: u8) {
    let parser = &mut terminal.parser;
    if byte & 0xc0 == 0x80 && parser.utf8_remaining > 0 {
        parser.utf8_codepoint = parser.utf8_codepoint << 6 | u32::from(byte & 0x3f);
        parser.utf8_remaining -= 1;
        if parser.utf8_remaining == 0 {
            let (codepoint, length) = (parser.utf8_codepoint, parser.utf8_length);
            let shortest = match length {
                2 => 0x80,
                3 => 0x800,
                _ => 0x1_0000,
            };
            let valid = codepoint >= shortest
                && !(0xd800..=0xdfff).contains(&codepoint)
                && codepoint <= 0x10_ffff;
            if valid {
                terminal.print(font::glyph_id(codepoint));
            } else {
                replace_bad_bytes(terminal, length);
            }
        }
        return;
    }
    if parser.utf8_remaining > 0 {
        let consumed = parser.utf8_length - parser.utf8_remaining;
        parser.utf8_remaining = 0;
        replace_bad_bytes(terminal, consumed);
    }
    let (length, bits) = match byte {
        0x00..=0x7f => return ground(terminal, byte),
        0xc2..=0xdf => (2, byte & 0x1f),
        0xe0..=0xef => (3, byte & 0x0f),
        0xf0..=0xf4 => (4, byte & 0x07),
        _ => return terminal.print(font::REPLACEMENT),
    };
    let parser = &mut terminal.parser;
    parser.utf8_length = length;
    parser.utf8_remaining = length - 1;
    parser.utf8_codepoint = u32::from(bits);
}

fn escape(terminal: &mut Terminal, byte: u8) {
    if byte < 0x20 {
        return control(terminal, byte);
    }
    let parser = &mut terminal.parser;
    parser.state = State::Ground;
    match byte {
        0x20..=0x2f => parser.state = State::EscapeIntermediate,
        b'[' => {
            parser.state = State::Csi;
            parser.parameters = Parameters::NONE;
            parser.private = 0;
            parser.intermediate = 0;
        }
        b']' | b'P' | b'X' | b'^' | b'_' => {
            parser.state = State::String;
            parser.string_length = 0;
        }
        b'7' => terminal.save_cursor(),
        b'8' => terminal.restore_cursor(),
        b'D' => terminal.index_down(),
        b'M' => terminal.reverse_index(),
        b'E' => {
            terminal.carriage_return();
            terminal.index_down();
        }
        b'c' => terminal.reset(),
        _ => {}
    }
}

fn escape_intermediate(terminal: &mut Terminal, byte: u8) {
    match byte {
        0x00..=0x1f => control(terminal, byte),
        0x20..=0x2f | DEL => {}
        _ => terminal.parser.state = State::Ground,
    }
}

fn csi(terminal: &mut Terminal, byte: u8) {
    let parser = &mut terminal.parser;
    let parameters = &mut parser.parameters;
    match byte {
        0x00..=0x1f => control(terminal, byte),
        b'0'..=b'9' if parser.intermediate == 0 => {
            parameters.count = parameters.count.max(1);
            let value = &mut parameters.values[parameters.count - 1];
            *value = value
                .saturating_mul(10)
                .saturating_add(u16::from(byte - b'0'));
        }
        b';' if parser.intermediate == 0 => {
            parameters.count = parameters.count.max(1);
            if parameters.count == MAX_PARAMETERS {
                parser.state = State::CsiIgnore;
            } else {
                parameters.count += 1;
            }
        }
        b'<'..=b'?' if parameters.count == 0 && parser.private == 0 && parser.intermediate == 0 => {
            parser.private = byte;
        }
        0x20..=0x2f => parser.intermediate = byte,
        0x40..=0x7e => {
            parser.state = State::Ground;
            dispatch_csi(terminal, byte);
        }
        DEL => {}
        _ => parser.state = State::CsiIgnore,
    }
}

fn csi_ignore(terminal: &mut Terminal, byte: u8) {
    match byte {
        0x00..=0x1f => control(terminal, byte),
        0x40..=0x7e => terminal.parser.state = State::Ground,
        _ => {}
    }
}

fn string(terminal: &mut Terminal, byte: u8) {
    let parser = &mut terminal.parser;
    match byte {
        BEL | CAN | SUB => parser.state = State::Ground,
        ESC => parser.state = State::StringEscape,
        _ => {
            parser.string_length += 1;
            if parser.string_length > MAX_STRING_BYTES {
                parser.state = State::Ground;
            }
        }
    }
}

fn string_escape(terminal: &mut Terminal, byte: u8) {
    if byte == b'\\' {
        terminal.parser.state = State::Ground;
    } else {
        terminal.parser.state = State::Escape;
        escape(terminal, byte);
    }
}

const PRIMARY_ATTRIBUTES: &[u8] = b"\x1b[?1;2c";
const SECONDARY_ATTRIBUTES: &[u8] = b"\x1b[>0;10;0c";

fn dispatch_csi(terminal: &mut Terminal, final_byte: u8) {
    let parameters = terminal.parser.parameters;
    let private = terminal.parser.private;
    if terminal.parser.intermediate != 0 {
        return;
    }
    let count = parameters.or(0, 1);
    match (private, final_byte) {
        (0, b'A') => terminal.cursor_up(count),
        (0, b'B') => terminal.cursor_down(count),
        (0, b'C') => terminal.cursor_right(count),
        (0, b'D') => terminal.cursor_left(count),
        (0, b'E') => {
            terminal.cursor_down(count);
            terminal.carriage_return();
        }
        (0, b'F') => {
            terminal.cursor_up(count);
            terminal.carriage_return();
        }
        (0, b'G') => terminal.set_column(count),
        (0, b'H' | b'f') => terminal.set_position(count, parameters.or(1, 1)),
        (0, b'd') => terminal.set_row(count),
        (0, b'J') => terminal.erase_in_display(parameters.raw(0)),
        (0, b'K') => terminal.erase_in_line(parameters.raw(0)),
        (0, b'L') => terminal.insert_lines(count),
        (0, b'M') => terminal.delete_lines(count),
        (0, b'@') => terminal.insert_characters(count),
        (0, b'P') => terminal.delete_characters(count),
        (0, b'X') => terminal.erase_characters(count),
        (0, b'S') => terminal.scroll_up(terminal.top, terminal.bottom, count, true),
        (0, b'T') if parameters.count <= 1 => {
            terminal.scroll_down(terminal.top, terminal.bottom, count)
        }
        (0, b'r') => terminal.set_scroll_region(parameters.raw(0), parameters.raw(1)),
        (0, b's') => terminal.position_saved = (terminal.row, terminal.column),
        (0, b'u') => {
            let (row, column) = terminal.position_saved;
            terminal.row = row.min(terminal.rows - 1);
            terminal.column = column.min(terminal.columns - 1);
            terminal.pending_wrap = false;
        }
        (0, b'n') if parameters.raw(0) == 6 => terminal.report_position(),
        (0, b'c') if parameters.raw(0) == 0 => terminal.reply(PRIMARY_ATTRIBUTES),
        (b'>', b'c') if parameters.raw(0) == 0 => terminal.reply(SECONDARY_ATTRIBUTES),
        (0, b'm') => select_graphic_rendition(terminal, &parameters),
        (b'?', b'h' | b'l') => {
            for index in 0..parameters.count {
                set_private_mode(terminal, parameters.raw(index), final_byte == b'h');
            }
        }
        _ => {}
    }
}

fn set_private_mode(terminal: &mut Terminal, mode: u16, on: bool) {
    match mode {
        1 => terminal.modes.application_cursor_keys = on,
        6 => {
            terminal.modes.origin = on;
            terminal.set_position(1, 1);
        }
        7 => {
            terminal.modes.autowrap = on;
            terminal.pending_wrap &= on;
        }
        12 => terminal.modes.cursor_blinking = on,
        25 => terminal.modes.cursor_visible = on,
        47 | 1047 => terminal.use_alternate_screen(on, false),
        1049 => terminal.use_alternate_screen(on, true),
        _ => {}
    }
}

fn select_graphic_rendition(terminal: &mut Terminal, parameters: &Parameters) {
    let attributes = &mut terminal.attributes;
    let count = parameters.count.max(1);
    let mut index = 0;
    while index < count {
        match parameters.raw(index) {
            0 => *attributes = Attributes::DEFAULT,
            1 => attributes.flags |= cell_flags::BOLD,
            4 => attributes.flags |= cell_flags::UNDERLINE,
            7 => attributes.flags |= cell_flags::INVERSE,
            22 => attributes.flags &= !cell_flags::BOLD,
            24 => attributes.flags &= !cell_flags::UNDERLINE,
            27 => attributes.flags &= !cell_flags::INVERSE,
            value @ 30..=37 => attributes.foreground = (value - 30) as u8,
            39 => attributes.foreground = colours::DEFAULT_COLOUR,
            value @ 40..=47 => attributes.background = (value - 40) as u8,
            49 => attributes.background = colours::DEFAULT_COLOUR,
            value @ 90..=97 => attributes.foreground = (value - 90 + 8) as u8,
            value @ 100..=107 => attributes.background = (value - 100 + 8) as u8,
            selector @ (38 | 48) => {
                let colour = match parameters.raw(index + 1) {
                    5 => {
                        let colour = colours::from_256(parameters.raw(index + 2));
                        index += 2;
                        Some(colour)
                    }
                    2 => {
                        let colour = colours::from_rgb(
                            parameters.raw(index + 2),
                            parameters.raw(index + 3),
                            parameters.raw(index + 4),
                        );
                        index += 4;
                        Some(colour)
                    }
                    _ => None,
                };
                match (selector, colour) {
                    (38, Some(colour)) => attributes.foreground = colour,
                    (_, Some(colour)) => attributes.background = colour,
                    _ => {}
                }
            }
            _ => {}
        }
        index += 1;
    }
}
