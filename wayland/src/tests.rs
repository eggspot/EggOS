use crate::blit::{OPAQUE_MASK, Rect, copy_opaque_rows};
use crate::keymap::{self, US_KEYMAP};
use crate::protocol::{self, Interface};
use crate::wire::{self, Arg, Header, MAX_ARGUMENTS, Value, WireError, decode, encode};

fn words(values: &[u32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_ne_bytes())
        .collect()
}

fn decoded<'a>(
    body: &'a [u8],
    signature: &[Arg],
) -> Result<([Value<'a>; MAX_ARGUMENTS], usize), WireError> {
    let mut values = [Value::None; MAX_ARGUMENTS];
    let descriptors = decode(body, signature, &mut values)?;
    Ok((values, descriptors))
}

#[test]
fn a_header_is_the_object_then_the_size_and_opcode() {
    let bytes = words(&[3, 12 << 16 | 2, 7]);
    assert_eq!(
        Header::read(&bytes),
        Ok(Some(Header {
            object: 3,
            opcode: 2,
            size: 12
        }))
    );
    assert_eq!(Header::read(&bytes[..7]), Ok(None));
    for size in [0u32, 4, 7, 10, 4100] {
        assert_eq!(
            Header::read(&words(&[1, size << 16])),
            Err(WireError::BadSize),
            "{size}"
        );
    }
    assert_eq!(
        Header::read(&words(&[1, 4096 << 16])).map(|header| header.map(|h| h.size)),
        Ok(Some(4096))
    );
}

#[test]
fn a_set_title_encodes_as_the_specification_lays_it_out() {
    let mut out = [0u8; 64];
    let size = encode(&mut out, 9, 2, &[Value::String(b"paint")]).unwrap();
    let expected = [
        9u32.to_ne_bytes(),
        (20u32 << 16 | 2).to_ne_bytes(),
        6u32.to_ne_bytes(),
        *b"pain",
        [b't', 0, 0, 0],
    ]
    .concat();
    assert_eq!(&out[..size], &expected[..]);
    let (values, descriptors) = decoded(&expected[8..], &[Arg::String]).unwrap();
    assert_eq!((values[0], descriptors), (Value::String(b"paint"), 0));
}

#[test]
fn a_string_of_four_bytes_takes_a_whole_extra_word_for_its_nul() {
    let mut out = [0u8; 64];
    let size = encode(&mut out, 1, 0, &[Value::String(b"seat")]).unwrap();
    assert_eq!(size, 8 + 4 + 8);
    assert_eq!(
        &out[8..20],
        &[&5u32.to_ne_bytes()[..], b"seat", &[0; 4]].concat()[..]
    );
}

#[test]
fn an_untyped_new_id_is_the_interface_its_version_and_the_id() {
    let body = [
        &words(&[5, 14])[..],
        b"wl_compositor\0\0\0",
        &words(&[4, 3])[..],
    ]
    .concat();
    let (values, _) = decoded(&body, &[Arg::Uint, Arg::UntypedNewId]).unwrap();
    assert_eq!(values[0], Value::Uint(5));
    assert_eq!(
        values[1],
        Value::UntypedNewId {
            interface: b"wl_compositor",
            version: 4,
            id: 3
        }
    );
    let mut out = [0u8; 64];
    let size = encode(&mut out, 2, 0, &values[..2]).unwrap();
    assert_eq!(&out[8..size], &body[..]);
}

#[test]
fn arrays_fixed_point_and_descriptors_round_trip() {
    let states = words(&[1, 4]);
    let mut out = [0u8; 64];
    let size = encode(
        &mut out,
        7,
        0,
        &[
            Value::Int(-5),
            Value::Array(&states),
            Value::Fd,
            Value::Fixed(wire::fixed(3)),
        ],
    )
    .unwrap();
    assert_eq!(size, 8 + 4 + 4 + 8 + 4);
    let (values, descriptors) =
        decoded(&out[8..size], &[Arg::Int, Arg::Array, Arg::Fd, Arg::Fixed]).unwrap();
    assert_eq!(descriptors, 1);
    assert_eq!(values[0].int(), -5);
    assert_eq!(values[1].bytes(), &states[..]);
    assert_eq!(values[2], Value::Fd);
    assert_eq!(wire::fixed_to_int(values[3].int()), 3);
    assert_eq!(wire::fixed(-2), -512);
    assert_eq!(wire::fixed(i32::MAX), i32::MAX);
}

#[test]
fn malformed_arguments_are_refused() {
    let short = words(&[1]);
    assert_eq!(
        decoded(&short, &[Arg::Uint, Arg::Uint]).err(),
        Some(WireError::Truncated)
    );
    assert_eq!(
        decoded(&words(&[1, 2]), &[Arg::Uint]).err(),
        Some(WireError::TrailingBytes)
    );
    assert_eq!(
        decoded(&[&words(&[4])[..], b"abcd"].concat(), &[Arg::String]).err(),
        Some(WireError::BadString)
    );
    assert_eq!(
        decoded(&words(&[0]), &[Arg::String]).err(),
        Some(WireError::BadString)
    );
    assert_eq!(
        decoded(&words(&[100, 0]), &[Arg::Array]).err(),
        Some(WireError::Truncated)
    );
    assert_eq!(
        decoded(&words(&[u32::MAX]), &[Arg::Array]).err(),
        Some(WireError::Truncated)
    );
    assert_eq!(
        decoded(&words(&[0]), &[Arg::Object]).err(),
        Some(WireError::NullObject)
    );
    assert_eq!(
        decoded(&words(&[0]), &[Arg::NewId]).err(),
        Some(WireError::NullObject)
    );
    assert_eq!(
        decoded(&words(&[0]), &[Arg::NullableObject]).map(|(values, _)| values[0]),
        Ok(Value::Object(0))
    );
    let mut small = [0u8; 12];
    assert_eq!(
        encode(&mut small, 1, 0, &[Value::String(b"too long")]),
        Err(WireError::TooLong)
    );
}

#[test]
fn random_bytes_never_panic_the_decoder() {
    let mut seed = 0x1234_5678_9abc_def0u64;
    let signatures: [&[Arg]; 4] = [
        &[Arg::Uint, Arg::UntypedNewId],
        &[Arg::NewId, Arg::Fd, Arg::Int],
        &[Arg::Int, Arg::Int, Arg::Array],
        &[Arg::String, Arg::NullableObject, Arg::Fixed],
    ];
    for round in 0..20_000 {
        let length = (round % 40) * 4;
        let bytes: Vec<u8> = (0..length)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                match seed % 5 {
                    0 => 0,
                    1 => 0xff,
                    _ => seed as u8 % 24,
                }
            })
            .collect();
        let _ = Header::read(&bytes);
        let _ = decoded(&bytes, signatures[round % signatures.len()]);
    }
}

#[test]
fn every_request_table_is_ordered_and_bounded() {
    for interface in [
        Interface::Display,
        Interface::Registry,
        Interface::Compositor,
        Interface::Region,
        Interface::Shm,
        Interface::ShmPool,
        Interface::Buffer,
        Interface::Surface,
        Interface::Seat,
        Interface::Pointer,
        Interface::Keyboard,
        Interface::Touch,
        Interface::Output,
        Interface::XdgWmBase,
        Interface::XdgPositioner,
        Interface::XdgSurface,
        Interface::XdgToplevel,
        Interface::XdgPopup,
    ] {
        for request in protocol::requests(interface) {
            assert!(request.signature.len() <= MAX_ARGUMENTS, "{}", request.name);
            assert!(!request.destructor || request.signature.is_empty());
        }
    }
    assert_eq!(
        protocol::request_of(Interface::Surface, 4, 9).map(|request| request.name),
        Some("damage_buffer")
    );
    assert_eq!(protocol::request_of(Interface::Surface, 3, 9), None);
    assert_eq!(
        protocol::request_of(Interface::XdgToplevel, 1, 5).map(|request| request.name),
        Some("move")
    );
    assert_eq!(
        protocol::request_of(Interface::XdgToplevel, 1, 13).map(|request| request.name),
        Some("set_minimized")
    );
    assert_eq!(protocol::request_of(Interface::Callback, 1, 0), None);
    assert_eq!(
        protocol::GLOBALS.map(|global| (global.interface.name(), global.version)),
        [
            ("wl_compositor", 4),
            ("wl_shm", 1),
            ("wl_seat", 5),
            ("wl_output", 3),
            ("xdg_wm_base", 3)
        ]
    );
}

#[test]
fn the_keymap_is_balanced_small_and_defines_every_key_it_maps() {
    assert!(US_KEYMAP.starts_with("xkb_keymap {\n"));
    assert!(US_KEYMAP.len() + 1 < 8192);
    let opens = US_KEYMAP.matches('{').count();
    let closes = US_KEYMAP.matches('}').count();
    assert_eq!(opens, closes);
    let (keycodes, rest) = US_KEYMAP.split_once("xkb_types").unwrap();
    let symbols = rest.split_once("xkb_symbols").unwrap().1;
    for line in symbols.lines().filter(|line| line.starts_with("key <")) {
        let name = &line[4..line.find('>').unwrap() + 1];
        assert!(keycodes.contains(&format!("{name} = ")), "{name}");
        let kind = line.split('"').nth(1).unwrap();
        assert!(rest.contains(&format!("type \"{kind}\" {{")), "{kind}");
    }
    for (name, code) in [
        ("<AC01>", 38),
        ("<ESC>", 9),
        ("<SPCE>", 65),
        ("<LWIN>", 133),
    ] {
        assert!(keycodes.contains(&format!("{name} = {code};")), "{name}");
    }
    assert!(US_KEYMAP.contains("modifier_map Mod4 { <LWIN>, <RWIN> };"));
}

#[test]
fn window_modifier_bits_become_xkb_masks() {
    assert_eq!(keymap::modifier_masks(0), (0, 0));
    assert_eq!(
        keymap::modifier_masks(1 | 2),
        (keymap::SHIFT | keymap::CONTROL, 0)
    );
    assert_eq!(
        keymap::modifier_masks(4 | 8),
        (keymap::MOD1 | keymap::MOD4, 0)
    );
    assert_eq!(keymap::modifier_masks(16), (0, keymap::LOCK));
}

#[test]
fn copying_rows_clips_to_both_sides_and_clears_the_top_byte() {
    let source: Vec<u32> = (0..8 * 4).map(|pixel| 0xff00_0000 | pixel).collect();
    let mut destination = vec![0u32; 6 * 3];
    copy_opaque_rows(&source, 8, &mut destination, 6, Rect::new(4, 1, 10, 10));
    assert_eq!(&destination[6..10], &[0; 4]);
    assert_eq!(&destination[6 + 4..6 + 6], &[12, 13]);
    assert_eq!(&destination[12 + 4..12 + 6], &[20, 21]);
    assert!(destination.iter().all(|pixel| pixel & !OPAQUE_MASK == 0));
    copy_opaque_rows(&source, 8, &mut destination, 6, Rect::new(100, 100, 5, 5));
}

#[test]
fn rectangles_from_clients_are_clipped_and_joined() {
    assert_eq!(Rect::from_signed(-5, -5, 10, 10), Rect::new(0, 0, 5, 5));
    assert!(Rect::from_signed(10, 10, -3, 4).is_empty());
    assert_eq!(
        Rect::from_signed(i32::MAX, 0, i32::MAX, 1).width,
        i32::MAX as u32
    );
    let joined = Rect::new(2, 2, 3, 3).union(&Rect::new(10, 0, 1, 1));
    assert_eq!(joined, Rect::new(2, 0, 9, 5));
    assert_eq!(Rect::default().union(&joined), joined);
    assert_eq!(joined.clipped(6, 4), Rect::new(2, 0, 4, 4));
    assert!(joined.clipped(1, 1).is_empty());
}
