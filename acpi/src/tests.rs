use super::*;
use alloc::collections::BTreeMap;
use alloc::vec;
use alloc::vec::Vec;

#[derive(Debug, Default)]
struct FakeHost {
    memory: BTreeMap<u64, u8>,
    configuration: BTreeMap<(u8, u8, u8, u16), u8>,
    configuration_reads: Vec<(PciFunction, u16, u32)>,
    notified: Vec<(Path, u64)>,
    slept: u64,
    ticks: u64,
}

impl Host for FakeHost {
    fn read(&mut self, _space: RegionSpace, address: u64, width: u32) -> Result<u64, HostError> {
        Ok((0..u64::from(width / 8)).rev().fold(0, |value, byte| {
            value << 8 | u64::from(self.memory.get(&(address + byte)).copied().unwrap_or(0))
        }))
    }

    fn write(
        &mut self,
        _space: RegionSpace,
        address: u64,
        width: u32,
        value: u64,
    ) -> Result<(), HostError> {
        for byte in 0..u64::from(width / 8) {
            self.memory.insert(address + byte, (value >> (byte * 8)) as u8);
        }
        Ok(())
    }

    fn read_pci(
        &mut self,
        function: PciFunction,
        offset: u16,
        width: u32,
    ) -> Result<u64, HostError> {
        self.configuration_reads.push((function, offset, width));
        let key = |byte: u16| (function.bus, function.device, function.function, offset + byte);
        Ok((0..(width / 8) as u16).rev().fold(0, |value, byte| {
            value << 8 | u64::from(self.configuration.get(&key(byte)).copied().unwrap_or(0xff))
        }))
    }

    fn write_pci(
        &mut self,
        _function: PciFunction,
        _offset: u16,
        _width: u32,
        _value: u64,
    ) -> Result<(), HostError> {
        Ok(())
    }

    fn sleep(&mut self, milliseconds: u64) {
        self.slept += milliseconds;
    }

    fn stall(&mut self, _microseconds: u64) {}

    fn timer(&mut self) -> u64 {
        self.ticks += 1;
        self.ticks
    }

    fn notify(&mut self, path: &Path, value: u64) {
        self.notified.push((path.clone(), value));
    }
}

fn cat(parts: &[&[u8]]) -> Vec<u8> {
    parts.concat()
}

fn package_length(content: usize) -> Vec<u8> {
    match content + 1 <= 0x3f {
        true => vec![(content + 1) as u8],
        false => {
            let total = content + 2;
            vec![0x40 | (total & 0x0f) as u8, (total >> 4) as u8]
        }
    }
}

fn package(lead: &[u8], body: &[u8]) -> Vec<u8> {
    cat(&[lead, &package_length(body.len()), body])
}

fn method(name: &[u8; 4], flags: u8, body: &[u8]) -> Vec<u8> {
    package(&[0x14], &cat(&[name, &[flags], body]))
}

fn device(name: &[u8; 4], body: &[u8]) -> Vec<u8> {
    package(&[0x5b, 0x82], &cat(&[name, body]))
}

fn table(revision: u8, body: &[u8]) -> Vec<u8> {
    let mut table = vec![0u8; 36];
    table[..4].copy_from_slice(b"DSDT");
    table[4..8].copy_from_slice(&((36 + body.len()) as u32).to_le_bytes());
    table[8] = revision;
    table.extend_from_slice(body);
    table
}

fn loaded(body: &[u8]) -> Interpreter<FakeHost> {
    let mut interpreter = Interpreter::new(FakeHost::default());
    interpreter.load_table(&table(2, body)).unwrap();
    interpreter
}

fn path(text: &[u8]) -> Path {
    Path::parse(text).unwrap()
}

#[test]
fn a_method_adds_one_to_its_argument() {
    let mut interpreter = loaded(&method(b"ADD1", 1, &[0xa4, 0x72, 0x68, 0x01, 0x00]));
    let sum = interpreter.evaluate(&path(b"\\ADD1"), &[Object::Integer(41)]);
    assert_eq!(sum, Ok(Object::Integer(42)));
}

#[test]
fn a_while_loop_sums_with_its_locals() {
    let looped = package(&[0xa2], &[0x95, 0x61, 0x68, 0x75, 0x61, 0x72, 0x60, 0x61, 0x60]);
    let body = cat(&[&[0x70, 0x00, 0x60, 0x70, 0x00, 0x61], &looped, &[0xa4, 0x60]]);
    let mut interpreter = loaded(&method(b"SUMN", 1, &body));
    let sum = interpreter.evaluate_integer(&path(b"\\SUMN"), &[Object::Integer(4)]);
    assert_eq!(sum, Ok(10));
}

#[test]
fn a_package_is_indexed_measured_and_changed_through_a_reference() {
    let buffer = package(&[0x11], &[0x0a, 0x02, 0x05, 0x06]);
    let elements = package(&[0x12], &cat(&[&[0x03, 0x01, 0x0d, b'A', b'B', 0x00], &buffer]));
    let body = cat(&[
        &cat(&[&[0x08], b"PKG0", &elements]),
        &method(b"GETE", 1, &cat(&[&[0xa4, 0x83, 0x88], b"PKG0", &[0x68, 0x00]])),
        &method(b"SIZE", 0, &cat(&[&[0xa4, 0x87], b"PKG0"])),
        &method(b"SETE", 0, &cat(&[&[0x70, 0x0a, 0x07, 0x88], b"PKG0", &[0x00, 0x00]])),
    ]);
    let mut interpreter = loaded(&body);
    let second = interpreter.evaluate(&path(b"\\GETE"), &[Object::Integer(1)]);
    assert_eq!(second, Ok(Object::String(b"AB".to_vec())));
    let third = interpreter.evaluate(&path(b"\\GETE"), &[Object::Integer(2)]);
    assert_eq!(third, Ok(Object::Buffer(vec![5, 6])));
    assert_eq!(interpreter.evaluate_integer(&path(b"\\SIZE"), &[]), Ok(3));
    assert!(interpreter.evaluate(&path(b"\\GETE"), &[Object::Integer(3)]).is_err());
    interpreter.evaluate(&path(b"\\SETE"), &[]).unwrap();
    let Ok(Object::Package(changed)) = interpreter.evaluate(&path(b"\\PKG0"), &[]) else {
        panic!("PKG0 is not a package");
    };
    assert_eq!(changed[0], Object::Integer(7));
}

#[test]
fn fields_write_their_bits_and_preserve_the_rest_of_the_unit() {
    let region = cat(&[&[0x5b, 0x80], b"REG0", &[0x00, 0x0b, 0x00, 0x10, 0x0a, 0x10]]);
    let field = package(
        &[0x5b, 0x81],
        &cat(&[b"REG0", &[0x01], b"FLD0", &[0x08, 0x00, 0x04], b"FLD1", &[0x04]]),
    );
    let set = method(
        b"SETF",
        0,
        &cat(&[&[0x70, 0x0a, 0xab], b"FLD0", &[0x70, 0x0a, 0x05], b"FLD1"]),
    );
    let mut interpreter = loaded(&cat(&[&region, &field, &set]));
    interpreter.host().memory.insert(0x1001, 0x03);
    interpreter.evaluate(&path(b"\\SETF"), &[]).unwrap();
    assert_eq!(interpreter.host().memory.get(&0x1000), Some(&0xab));
    assert_eq!(interpreter.host().memory.get(&0x1001), Some(&0x53));
    assert_eq!(interpreter.evaluate_integer(&path(b"\\FLD1"), &[]), Ok(5));
    let outside = Field {
        kind: FieldKind::Region(interpreter.namespace().find(&path(b"\\REG0")).unwrap()),
        bit_offset: 0x10 * 8,
        bit_length: 8,
        access: Access::Byte,
        update: UpdateRule::Preserve,
    };
    assert_eq!(interpreter.read_field(&outside), Err(Error::OutsideRegion));
}

#[test]
fn osi_answers_windows_versions_and_not_others() {
    let mut interpreter = loaded(&method(b"OSIT", 1, &cat(&[&[0xa4, 0x5c], b"_OSI", &[0x68]])));
    let windows = Object::String(b"Windows 2015".to_vec());
    let linux = Object::String(b"Linux".to_vec());
    assert_eq!(interpreter.evaluate_integer(&path(b"\\OSIT"), &[windows]), Ok(u64::MAX));
    assert_eq!(interpreter.evaluate_integer(&path(b"\\OSIT"), &[linux]), Ok(0));
}

#[test]
fn present_devices_run_their_ini_and_absent_ones_hide_their_children() {
    let set_flag = |value: &[u8]| cat(&[&[0x70], value, &[0x5c], b"FLAG"]);
    let body = cat(&[
        &cat(&[&[0x08], b"FLAG", &[0x00]]),
        &device(
            b"DEV0",
            &cat(&[
                &cat(&[&[0x08], b"_STA", &[0x0a, 0x0f]]),
                &method(b"_INI", 0, &set_flag(&[0x01])),
            ]),
        ),
        &device(
            b"DEV1",
            &cat(&[
                &cat(&[&[0x08], b"_STA", &[0x00]]),
                &device(b"SUB0", &method(b"_INI", 0, &set_flag(&[0x0a, 0x02]))),
            ]),
        ),
    ]);
    let mut interpreter = loaded(&body);
    let statuses = interpreter.initialize_devices();
    assert_eq!(interpreter.evaluate_integer(&path(b"\\FLAG"), &[]), Ok(1));
    let status_of = |wanted: &[u8]| {
        statuses
            .iter()
            .find(|status| status.path == path(wanted))
            .map(|status| status.status)
    };
    assert_eq!(status_of(b"\\DEV0"), Some(0x0f));
    assert_eq!(status_of(b"\\DEV1"), Some(0));
    assert_eq!(status_of(b"\\DEV1.SUB0"), None);
}

#[test]
fn a_method_builds_a_buffer_through_a_buffer_field_and_its_names_go_with_it() {
    let body = cat(&[
        &cat(&[&[0x08], b"RBUF", &package(&[0x11], &[0x0a, 0x08])]),
        &cat(&[&[0x8a], b"RBUF", &[0x0a, 0x04], b"VAL0"]),
        &cat(&[&[0x70, 0x0c, 0x78, 0x56, 0x34, 0x12], b"VAL0"]),
        &cat(&[&[0xa4], b"RBUF"]),
    ]);
    let mut interpreter = loaded(&method(b"CRS0", 0, &body));
    let built = interpreter.evaluate(&path(b"\\CRS0"), &[]);
    assert_eq!(built, Ok(Object::Buffer(vec![0, 0, 0, 0, 0x78, 0x56, 0x34, 0x12])));
    assert_eq!(interpreter.namespace().find(&path(b"\\CRS0.RBUF")), None);
    assert_eq!(interpreter.namespace().find(&path(b"\\CRS0.VAL0")), None);
}

#[test]
fn a_pci_config_region_reads_the_function_its_device_and_bridge_name() {
    let configuration_field = package(
        &[0x5b, 0x81],
        &cat(&[b"PCFG", &[0x03], b"VID0", &[0x10], b"DID0", &[0x10]]),
    );
    let function = device(
        b"DEV0",
        &cat(&[
            &cat(&[&[0x08], b"_ADR", &[0x0c, 0x00, 0x00, 0x03, 0x00]]),
            &cat(&[&[0x5b, 0x80], b"PCFG", &[0x02, 0x00, 0x0b, 0x00, 0x01]]),
            &configuration_field,
        ]),
    );
    let bridge = device(
        b"PCI0",
        &cat(&[
            &cat(&[&[0x08], b"_HID", &[0x0c, 0x41, 0xd0, 0x0a, 0x03]]),
            &cat(&[&[0x08], b"_BBN", &[0x0a, 0x02]]),
            &function,
        ]),
    );
    let mut interpreter = loaded(&bridge);
    for (offset, byte) in [0x86u8, 0x80, 0x34, 0x12].into_iter().enumerate() {
        interpreter
            .host()
            .configuration
            .insert((2, 3, 0, offset as u16), byte);
    }
    assert_eq!(interpreter.evaluate_integer(&path(b"\\PCI0.DEV0.VID0"), &[]), Ok(0x8086));
    assert_eq!(interpreter.evaluate_integer(&path(b"\\PCI0.DEV0.DID0"), &[]), Ok(0x1234));
    let expected = PciFunction {
        segment: 0,
        bus: 2,
        device: 3,
        function: 0,
    };
    assert_eq!(interpreter.host().configuration_reads[0], (expected, 0, 32));
}

#[test]
fn if_and_else_choose_by_a_string_comparison() {
    let then = package(&[0xa0], &cat(&[&[0x93, 0x68, 0x0d], b"ABC\0", &[0xa4, 0x01]]));
    let otherwise = package(&[0xa1], &[0xa4, 0x0a, 0x02]);
    let mut interpreter = loaded(&method(b"CMPS", 1, &cat(&[&then, &otherwise])));
    let same = Object::String(b"ABC".to_vec());
    let other = Object::String(b"XYZ".to_vec());
    assert_eq!(interpreter.evaluate_integer(&path(b"\\CMPS"), &[same]), Ok(1));
    assert_eq!(interpreter.evaluate_integer(&path(b"\\CMPS"), &[other]), Ok(2));
}

#[test]
fn divide_gives_its_remainder_and_quotient() {
    let body = [
        0x78, 0x0a, 0x11, 0x0a, 0x05, 0x60, 0x61, 0xa4, 0x72, 0x60, 0x79, 0x61, 0x0a, 0x04, 0x00,
        0x00,
    ];
    let mut interpreter = loaded(&method(b"DIVM", 0, &body));
    assert_eq!(interpreter.evaluate_integer(&path(b"\\DIVM"), &[]), Ok(50));
}

#[test]
fn notify_tells_the_host_which_device() {
    let body = cat(&[
        &device(b"DEV0", &[]),
        &method(b"NTFY", 0, &cat(&[&[0x86], b"DEV0", &[0x0a, 0x80]])),
    ]);
    let mut interpreter = loaded(&body);
    interpreter.evaluate(&path(b"\\NTFY"), &[]).unwrap();
    assert_eq!(interpreter.host().notified, vec![(path(b"\\DEV0"), 0x80)]);
}

#[test]
fn a_revision_1_dsdt_makes_integers_32_bits_wide() {
    let mut interpreter = Interpreter::new(FakeHost::default());
    interpreter
        .load_table(&table(1, &method(b"ONES", 0, &[0xa4, 0xff])))
        .unwrap();
    assert!(interpreter.has_32_bit_integers());
    assert_eq!(interpreter.evaluate_integer(&path(b"\\ONES"), &[]), Ok(0xffff_ffff));
}

#[test]
fn a_table_shorter_than_its_header_says_is_refused() {
    let mut interpreter = Interpreter::new(FakeHost::default());
    let mut short = table(2, &[]);
    short[4] = 0xff;
    assert_eq!(interpreter.load_table(&short), Err(Error::NotATable));
}

#[test]
fn eisa_ids_compress_as_hid_gives_them() {
    assert_eq!(eisa_id(b"PNP0A03"), 0x030a_d041);
    assert_eq!(eisa_id(b"PNP0A08"), 0x080a_d041);
}

#[test]
fn paths_parse_and_write_back() {
    let parsed = path(b"\\_SB.PCI0._PRT");
    assert_eq!(parsed.depth(), 3);
    assert_eq!(parsed.last(), Some(NameSeg::new(b"_PRT")));
    let mut written = Vec::new();
    parsed.write(&mut written);
    assert_eq!(written, b"\\_SB_.PCI0._PRT");
    assert_eq!(Path::parse(b"_SB.PCI0"), None);
    assert_eq!(Path::parse(b"\\"), Some(Path::root()));
}
