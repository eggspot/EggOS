use super::*;
use crate::fake::Fake;

pub const EXAMPLE: &[u8] =
    b"0\t-\tdevicetree\tserial\tpl011@9000000\tarm,pl011\t-\tunbound\t-\t-\n\
1\t-\tdevicetree\tclock\tpl031@9010000\tarm,pl031\tpl031\tbound\tpolled\t-\n\
2\t-\tdevicetree\tbus\tvirtio_mmio@a000000\tvirtio,mmio\tvirtio-mmio\tbound\t-\t-\n\
3\t-\tdevicetree\tbus\tvirtio_mmio@a000200\tvirtio,mmio\t-\tunbound\t-\t-\n\
4\t2\tvirtio\tinput\tinput\tvirtio input\tvirtio-input\tbound\tinterrupts\t-\n";

fn run(fake: Fake, arguments: &[&CStr]) -> (i32, String, String) {
    let mut output = Output::new(fake, b"device");
    let status = device(&mut output, arguments);
    let status = output.finish(status);
    (status, output.system.output(), output.system.errors())
}

fn machine(devices: &[u8]) -> Fake {
    Fake::default()
        .with_directory("/proc", 0)
        .with_directory("/proc/driver", 0)
        .with_file("/proc/driver/devices", 0o444, devices, 0)
}

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
    let refused = b"5\t-\tdevicetree\tbus\tvirtio_mmio@a000400\tvirtio,mmio\tvirtio-mmio\trefused\t-\ta legacy device\n";
    let text = [EXAMPLE, &refused[..]].concat();
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
    let refused = Device::parse(
        b"5\t-\tdevicetree\tbus\tvirtio_mmio@a000400\tvirtio,mmio\tvirtio-mmio\trefused\t-\ta legacy device\n",
    )
    .unwrap();
    refused.write_driver(&mut |bytes| words.extend_from_slice(bytes));
    assert_eq!(words, b"Refused: a legacy device");
    words.clear();
    refused.write_device(&mut |bytes| words.extend_from_slice(bytes));
    assert_eq!(words, "virtio_mmio@a000400 · virtio,mmio".as_bytes());
}

#[test]
fn the_examples_rows_print_as_a_table_and_as_records() {
    let (status, output, errors) = run(machine(EXAMPLE), &[c"list"]);
    assert_eq!((status, errors.as_str()), (0, ""));
    assert_eq!(output.lines().count(), 4);
    assert!(output.starts_with("ID  PARENT  KIND"));
    let (status, output, _) = run(machine(EXAMPLE), &[c"list", c"--json"]);
    assert_eq!(status, 0);
    assert_eq!(output.lines().count(), 3);
    assert!(output.starts_with("{\"id\":4,\"parent\":2,\"bus\":\"virtio\""));
}

#[test]
fn a_missing_file_or_a_wrong_verb_fails() {
    let (status, _, errors) = run(Fake::default(), &[c"list"]);
    assert_eq!(status, 1);
    assert!(errors.contains("/proc/driver/devices"), "{errors}");
    let (status, _, errors) = run(machine(EXAMPLE), &[c"show"]);
    assert_eq!(status, 1);
    assert!(errors.contains("unknown verb 'show'"), "{errors}");
    let (status, _, _) = run(machine(EXAMPLE), &[c"list", c"extra"]);
    assert_eq!(status, 1);
}
