use super::*;

const REFUSED: &[u8] =
    b"5\t-\tdevicetree\tbus\tvirtio_mmio@a000400\tvirtio,mmio\tvirtio-mmio\trefused\t-\ta legacy device\n";

#[test]
fn a_line_has_ten_fields_with_dashes_for_none() {
    let device = Device::parse(
        b"4\t2\tvirtio\tinput\tinput\tvirtio input\tvirtio-input\tbound\tinterrupts\t-\n",
    )
    .unwrap();
    assert_eq!(device.id, 4);
    assert_eq!(device.parent, Some(2));
    assert_eq!(device.driver, Some(&b"virtio-input"[..]));
    assert_eq!(device.reason, None);
    assert_eq!(device.kind(), "Input");
    assert_eq!(Device::parse(b"4\t2\tvirtio\n"), None);
    assert_eq!(Device::parse(b"x\t-\ta\tb\tc\td\te\tf\tg\th\n"), None);
    assert_eq!(Device::parse(b"1\t-\ta\tb\tc\td\te\tf\tg\th\ti\n"), None);
}

#[test]
fn the_default_list_hides_parents_buses_and_controllers_but_never_a_refusal() {
    let text = [EXAMPLE, REFUSED].concat();
    let mut devices = [Device::EMPTY; MAX_DEVICES];
    let (count, cut) = parse_all(&text, &mut devices);
    assert_eq!((count, cut), (6, false));
    let mut order = [0; MAX_DEVICES];
    let shown = rows(&devices[..count], false, &mut order);
    let ids: Vec<u64> = order[..shown].iter().map(|&at| devices[at].id).collect();
    assert_eq!(ids, [4, 0, 1, 5]);
    let shown = rows(&devices[..count], true, &mut order);
    let ids: Vec<u64> = order[..shown].iter().map(|&at| devices[at].id).collect();
    assert_eq!(ids, [4, 0, 1, 2, 3, 5]);
}

#[test]
fn the_columns_use_the_words_people_read() {
    let mut words = Vec::new();
    let refused = Device::parse(REFUSED).unwrap();
    refused.write_driver(&mut |bytes| words.extend_from_slice(bytes));
    assert_eq!(words, b"Refused: a legacy device");
    words.clear();
    refused.write_device(&mut |bytes| words.extend_from_slice(bytes));
    assert_eq!(words, "virtio_mmio@a000400 · virtio,mmio".as_bytes());
    words.clear();
    let unbound = Device::parse(EXAMPLE.split(|&byte| byte == b'\n').next().unwrap()).unwrap();
    unbound.write_driver(&mut |bytes| words.extend_from_slice(bytes));
    assert_eq!(words, b"No driver");
}

#[test]
fn short_buffers_keep_what_fits_and_say_the_rest_was_left_out() {
    let mut devices = [Device::EMPTY; 2];
    assert_eq!(parse_all(EXAMPLE, &mut devices), (2, true));
    let mut all = [Device::EMPTY; MAX_DEVICES];
    let (count, _) = parse_all(EXAMPLE, &mut all);
    let mut order = [0; 2];
    assert_eq!(rows(&all[..count], true, &mut order), 2);
}
