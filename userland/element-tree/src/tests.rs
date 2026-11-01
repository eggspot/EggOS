use super::*;

const fn encode(direction: u32, number: u32, size: usize) -> u32 {
    direction << 30 | (size as u32) << 16 | (b'w' as u32) << 8 | number
}

#[test]
fn the_requests_have_linux_s_encoding_with_their_sizes() {
    assert_eq!(request::GET_TREE, encode(3, 24, size::GET_TREE));
    assert_eq!(request::ACT_ON_ELEMENT, encode(3, 25, size::ACT_ON_ELEMENT));
    assert_eq!(request::READ_QUERY, encode(3, 26, size::READ_QUERY));
    assert_eq!(request::ANSWER_QUERY, encode(1, 27, size::ANSWER_QUERY));
    assert_eq!(act_on_element::TEXT + MAX_TEXT_BYTES, size::ACT_ON_ELEMENT);
    assert_eq!(read_query::TEXT + MAX_TEXT_BYTES, size::READ_QUERY);
}

#[test]
fn a_written_tree_reads_back_element_by_element() {
    let mut buffer = [0u8; 512];
    let mut tree = TreeWriter::new(&mut buffer);
    let root = Element::new(1, 0, Role::Window, Bounds::new(0, 0, 640, 480)).named(b"Settings");
    let field = Element::new(7, 1, Role::TextField, Bounds::new(-4, 10, 200, 28))
        .named(b"Name")
        .valued("caf\u{e9}".as_bytes())
        .flagged(flags::FOCUSABLE | flags::FOCUSED);
    assert!(tree.push(&root));
    assert!(tree.push(&field));
    assert_eq!(tree.count(), 2);
    assert_eq!(tree.bytes().len(), 2 * HEADER_BYTES + 8 + 4 + 5);
    let read: Vec<_> = elements(tree.bytes()).collect();
    assert_eq!(read, [Ok(root), Ok(field)]);
}

#[test]
fn a_tree_stops_at_its_buffer_and_long_names_are_cut_at_a_character() {
    let mut buffer = [0u8; HEADER_BYTES + 260];
    let mut tree = TreeWriter::new(&mut buffer);
    let mut long = [b'a'; 300];
    long[254] = 0xc3;
    long[255] = 0xa9;
    let element = Element::new(1, 0, Role::Label, Bounds::default()).named(&long);
    assert!(tree.push(&element));
    let first = elements(tree.bytes()).next().unwrap().unwrap();
    assert_eq!(
        first.name.len(),
        254,
        "the two-byte character is left out whole"
    );
    assert!(!tree.push(&Element::new(2, 1, Role::Button, Bounds::default())));
    assert!(tree.is_full());
    assert_eq!(tree.count(), 1);
}

#[test]
fn a_malformed_tree_ends_with_an_error_and_unknown_roles_stay_readable() {
    let mut buffer = [0u8; 128];
    let mut tree = TreeWriter::new(&mut buffer);
    tree.push(&Element::new(3, 0, Role::Button, Bounds::default()).named(b"OK"));
    let mut bytes = tree.bytes().to_vec();
    bytes[8..10].copy_from_slice(&999u16.to_ne_bytes());
    let read: Vec<_> = elements(&bytes).collect();
    assert_eq!(read.len(), 1);
    assert_eq!(read[0].unwrap().role, Role::Unknown);
    bytes.extend_from_slice(&[1, 2, 3]);
    let read: Vec<_> = elements(&bytes).collect();
    assert_eq!(read.len(), 2);
    assert_eq!(read[1], Err(Malformed));
    let mut lying = bytes[..HEADER_BYTES + 2].to_vec();
    lying[28..30].copy_from_slice(&40u16.to_ne_bytes());
    assert_eq!(elements(&lying).next(), Some(Err(Malformed)));
}

#[test]
fn actions_and_outcomes_have_their_numbers_and_names() {
    for action in Action::ALL {
        assert_eq!(Action::from_value(action.value()), Some(action));
        assert_eq!(Action::from_name(action.name().as_bytes()), Some(action));
    }
    assert_eq!(Action::from_value(0), None);
    assert_eq!(Action::from_value(5), None);
    assert_eq!(Action::from_name(b"set-text"), Some(Action::SetText));
    for outcome in [
        Outcome::Done,
        Outcome::NoSuchElement,
        Outcome::NotSupported,
        Outcome::Disabled,
    ] {
        assert_eq!(Outcome::from_value(outcome.value()), outcome);
    }
    for role in Role::ALL {
        assert_eq!(Role::from_value(role.value()), role);
    }
}

#[test]
fn a_query_record_carries_the_serial_window_kind_element_and_action() {
    let mut record = [0u8; 32];
    record[0..2].copy_from_slice(&QUERY_EVENT.to_ne_bytes());
    record[2..4].copy_from_slice(&129u16.to_ne_bytes());
    put_word(&mut record, 8, 42);
    put_word(&mut record, 12, query_kind::ACT);
    put_word(&mut record, 16, 301);
    put_word(&mut record, 20, Action::Select.value());
    let query = Query::from_record(&record).unwrap();
    assert_eq!((query.serial, query.window, query.element), (42, 129, 301));
    assert_eq!(query.action(), Some(Action::Select));
    record[0] = 14;
    assert_eq!(Query::from_record(&record), None);
}
