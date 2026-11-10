use super::*;
use crate::fake::Fake;

pub const EXAMPLE: &[u8] = eggos_device_list::EXAMPLE;

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
