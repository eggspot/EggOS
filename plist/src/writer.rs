//! Writing the binary format (`bplist00`, D723) without allocating: the caller numbers its
//! objects and gives each one as a [`Node`] on request, collections naming their members by
//! number; [`write_binary`] sizes every object first, then hands the header, the objects, the
//! offset table and the trailer to the caller's output in order. Integers take the fewest bytes
//! that hold them (negative ones eight, as Apple writes them), strings stay ASCII when they can
//! and are UTF-16 otherwise, and object references and offsets take the fewest bytes the list
//! needs.

use crate::binary::MAGIC;

const LONG_COUNT: usize = 15;
const TRAILER_UNUSED: usize = 6;

mod marker {
    pub const FALSE: u8 = 0x08;
    pub const TRUE: u8 = 0x09;
    pub const INTEGER: u8 = 0x10;
    pub const REAL_EIGHT: u8 = 0x23;
    pub const DATE: u8 = 0x33;
    pub const DATA: u8 = 0x40;
    pub const ASCII_STRING: u8 = 0x50;
    pub const UTF16_STRING: u8 = 0x60;
    pub const UID: u8 = 0x80;
    pub const ARRAY: u8 = 0xa0;
    pub const DICTIONARY: u8 = 0xd0;
}

/// One object of a list being written; an array or dictionary names its members by their
/// numbers among the list's objects.
#[derive(Debug, Clone, Copy)]
pub enum Node<'n> {
    Boolean(bool),
    Integer(i64),
    Real(f64),
    /// Seconds since 2001-01-01.
    Date(f64),
    String(&'n str),
    Data(&'n [u8]),
    Uid(u64),
    Array(&'n [u32]),
    /// The keys' numbers, then the values' numbers, as many of each.
    Dictionary(&'n [u32], &'n [u32]),
}

fn width_for(largest: u64) -> usize {
    match largest {
        0..=0xff => 1,
        0x100..=0xffff => 2,
        0x1_0000..=0xffff_ffff => 4,
        _ => 8,
    }
}

fn width_exponent(width: usize) -> u8 {
    width.trailing_zeros() as u8
}

fn integer_width(value: i64) -> usize {
    match u64::try_from(value) {
        Ok(value) => width_for(value),
        Err(_) => 8,
    }
}

fn counted_header_size(count: usize) -> usize {
    match count < LONG_COUNT {
        true => 1,
        false => 2 + width_for(count as u64),
    }
}

fn utf16_units(text: &str) -> usize {
    text.chars().map(char::len_utf16).sum()
}

fn size(node: Node<'_>, reference_width: usize) -> usize {
    match node {
        Node::Boolean(_) => 1,
        Node::Integer(value) => 1 + integer_width(value),
        Node::Real(_) | Node::Date(_) => 9,
        Node::String(text) if text.is_ascii() => counted_header_size(text.len()) + text.len(),
        Node::String(text) => {
            let units = utf16_units(text);
            counted_header_size(units) + 2 * units
        }
        Node::Data(bytes) => counted_header_size(bytes.len()) + bytes.len(),
        Node::Uid(value) => 1 + width_for(value),
        Node::Array(members) => {
            counted_header_size(members.len()) + members.len() * reference_width
        }
        Node::Dictionary(keys, values) => {
            let count = keys.len().min(values.len());
            counted_header_size(count) + 2 * count * reference_width
        }
    }
}

fn put_big_endian(out: &mut impl FnMut(&[u8]), value: u64, width: usize) {
    out(&value.to_be_bytes()[8 - width..]);
}

fn put_counted(out: &mut impl FnMut(&[u8]), kind: u8, count: usize) {
    if count < LONG_COUNT {
        out(&[kind | count as u8]);
        return;
    }
    let width = width_for(count as u64);
    out(&[kind | LONG_COUNT as u8, marker::INTEGER | width_exponent(width)]);
    put_big_endian(out, count as u64, width);
}

fn put_references(out: &mut impl FnMut(&[u8]), members: &[u32], width: usize) {
    for &member in members {
        put_big_endian(out, u64::from(member), width);
    }
}

fn put(out: &mut impl FnMut(&[u8]), node: Node<'_>, reference_width: usize) {
    match node {
        Node::Boolean(true) => out(&[marker::TRUE]),
        Node::Boolean(false) => out(&[marker::FALSE]),
        Node::Integer(value) => {
            let width = integer_width(value);
            out(&[marker::INTEGER | width_exponent(width)]);
            put_big_endian(out, value as u64, width);
        }
        Node::Real(value) => {
            out(&[marker::REAL_EIGHT]);
            out(&value.to_bits().to_be_bytes());
        }
        Node::Date(seconds) => {
            out(&[marker::DATE]);
            out(&seconds.to_bits().to_be_bytes());
        }
        Node::String(text) if text.is_ascii() => {
            put_counted(out, marker::ASCII_STRING, text.len());
            out(text.as_bytes());
        }
        Node::String(text) => {
            put_counted(out, marker::UTF16_STRING, utf16_units(text));
            let mut units = [0u16; 2];
            for character in text.chars() {
                for unit in character.encode_utf16(&mut units) {
                    out(&unit.to_be_bytes());
                }
            }
        }
        Node::Data(bytes) => {
            put_counted(out, marker::DATA, bytes.len());
            out(bytes);
        }
        Node::Uid(value) => {
            let width = width_for(value);
            out(&[marker::UID | (width as u8 - 1)]);
            put_big_endian(out, value, width);
        }
        Node::Array(members) => {
            put_counted(out, marker::ARRAY, members.len());
            put_references(out, members, reference_width);
        }
        Node::Dictionary(keys, values) => {
            let count = keys.len().min(values.len());
            put_counted(out, marker::DICTIONARY, count);
            put_references(out, &keys[..count], reference_width);
            put_references(out, &values[..count], reference_width);
        }
    }
}

/// Writes the `count` objects `node` gives (numbered from 0) as a binary property list whose
/// top object is number `top`, handing its bytes to `out` in order.
pub fn write_binary<'n>(
    count: usize,
    top: usize,
    node: impl Fn(usize) -> Node<'n>,
    out: &mut impl FnMut(&[u8]),
) {
    let reference_width = width_for(count.saturating_sub(1) as u64);
    let objects_end = (0..count).fold(MAGIC.len(), |end, index| {
        end + size(node(index), reference_width)
    });
    let offset_width = width_for(objects_end as u64);
    out(MAGIC);
    for index in 0..count {
        put(out, node(index), reference_width);
    }
    let mut offset = MAGIC.len();
    for index in 0..count {
        put_big_endian(out, offset as u64, offset_width);
        offset += size(node(index), reference_width);
    }
    out(&[0; TRAILER_UNUSED]);
    out(&[offset_width as u8, reference_width as u8]);
    out(&(count as u64).to_be_bytes());
    out(&(top as u64).to_be_bytes());
    out(&(objects_end as u64).to_be_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Document, Format, Kind};

    fn written(nodes: &[Node<'_>], top: usize) -> Vec<u8> {
        let mut bytes = Vec::new();
        write_binary(nodes.len(), top, |index| nodes[index], &mut |part: &[u8]| {
            bytes.extend_from_slice(part)
        });
        bytes
    }

    #[test]
    fn a_written_dictionary_reads_back_with_every_kind_of_value() {
        let long = "a string longer than fifteen characters";
        let nodes = [
            Node::Dictionary(&[1, 2, 3, 4], &[5, 6, 7, 8]),
            Node::String("ascii"),
            Node::String("wide"),
            Node::String("long"),
            Node::String("list"),
            Node::Integer(-5),
            Node::String("héllo"),
            Node::String(long),
            Node::Array(&[9, 10, 11]),
            Node::Uid(300),
            Node::Real(1.5),
            Node::Boolean(true),
        ];
        let bytes = written(&nodes, 0);
        let document = Document::parse(&bytes).unwrap();
        assert_eq!(document.format(), Format::Binary);
        let top = document.top();
        assert_eq!(top.get(b"ascii").unwrap().integer(), Some(-5));
        assert!(top.get(b"wide").unwrap().text().unwrap().equals("héllo".as_bytes()));
        assert!(top.get(b"long").unwrap().text().unwrap().equals(long.as_bytes()));
        let list = top.get(b"list").unwrap();
        assert_eq!(list.kind(), Some(Kind::Array));
        assert_eq!(list.item(0).unwrap().uid(), Some(300));
        assert_eq!(list.item(1).unwrap().real(), Some(1.5));
        assert_eq!(list.item(2).unwrap().boolean(), Some(true));
    }

    #[test]
    fn integers_take_the_fewest_bytes_and_negative_ones_eight() {
        assert_eq!(written(&[Node::Integer(200)], 0)[8..10], [0x10, 200]);
        assert_eq!(written(&[Node::Integer(-1)], 0)[8], 0x13);
        assert_eq!(written(&[Node::Integer(70_000)], 0)[8], 0x12);
    }

    #[test]
    fn many_objects_widen_the_references() {
        let mut nodes = vec![Node::Integer(0); 300];
        let members: Vec<u32> = (1..300).collect();
        nodes[0] = Node::Array(&members);
        let bytes = written(&nodes, 0);
        let document = Document::parse(&bytes).unwrap();
        let top = document.top();
        assert_eq!(top.count(), Some(299));
        assert_eq!(top.item(298).unwrap().integer(), Some(0));
    }
}
