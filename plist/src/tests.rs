use super::*;

fn ascii(text: &str) -> Vec<u8> {
    let mut object = match text.len() {
        short @ 0..15 => vec![0x50 | short as u8],
        long => vec![0x5f, 0x10, long as u8],
    };
    object.extend_from_slice(text.as_bytes());
    object
}

fn utf16(text: &str) -> Vec<u8> {
    let units: Vec<u16> = text.encode_utf16().collect();
    let mut object = vec![0x6f, 0x10, units.len() as u8];
    for unit in units {
        object.extend_from_slice(&unit.to_be_bytes());
    }
    object
}

fn binary(objects: &[Vec<u8>], top: u8) -> Vec<u8> {
    let mut bytes = binary::MAGIC.to_vec();
    let mut offsets = Vec::new();
    for object in objects {
        offsets.push(bytes.len() as u8);
        bytes.extend_from_slice(object);
    }
    let table = bytes.len();
    bytes.extend_from_slice(&offsets);
    let mut trailer = [0u8; 32];
    trailer[6] = 1;
    trailer[7] = 1;
    trailer[8..16].copy_from_slice(&(objects.len() as u64).to_be_bytes());
    trailer[16..24].copy_from_slice(&u64::from(top).to_be_bytes());
    trailer[24..32].copy_from_slice(&(table as u64).to_be_bytes());
    bytes.extend_from_slice(&trailer);
    bytes
}

fn dictionary_plist(pairs: &[(Vec<u8>, Vec<u8>)]) -> Vec<u8> {
    let count = pairs.len();
    let mut objects = vec![{
        let mut dictionary = vec![0xd0 | count as u8];
        dictionary.extend((0..count).map(|index| (1 + 2 * index) as u8));
        dictionary.extend((0..count).map(|index| (2 + 2 * index) as u8));
        dictionary
    }];
    for (key, value) in pairs {
        objects.push(key.clone());
        objects.push(value.clone());
    }
    binary(&objects, 0)
}

fn string_of(document: &[u8], key: &[u8]) -> Option<String> {
    let mut out = [0u8; 64];
    let top = Document::parse(document).ok()?.top();
    let text = top.get(key)?.text()?;
    Some(String::from_utf8(text.write_utf8(&mut out)?.to_vec()).unwrap())
}

#[test]
fn a_binary_top_level_string_is_found_by_its_key_in_ascii_or_utf16() {
    let bytes = dictionary_plist(&[
        (ascii("CFBundleExecutable"), ascii("Paint")),
        (ascii("CFBundleIconFile"), utf16("Ic\u{f4}ne \u{1f95a}")),
    ]);
    assert_eq!(Document::parse(&bytes).unwrap().format(), Format::Binary);
    assert_eq!(
        string_of(&bytes, b"CFBundleExecutable").as_deref(),
        Some("Paint")
    );
    assert_eq!(
        string_of(&bytes, b"CFBundleIconFile").as_deref(),
        Some("Ic\u{f4}ne \u{1f95a}")
    );
    assert_eq!(string_of(&bytes, b"CFBundleName"), None);
    let top = Document::parse(&bytes).unwrap().top();
    let mut short = [0u8; 3];
    assert_eq!(
        top.get(b"CFBundleExecutable")
            .unwrap()
            .text()
            .unwrap()
            .write_utf8(&mut short),
        None
    );
}

#[test]
fn binary_scalars_collections_and_uids_read_back() {
    let objects = vec![
        vec![0xa7, 1, 2, 3, 4, 5, 6, 7],
        vec![0x09],
        vec![0x13, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xfe],
        {
            let mut real = vec![0x23];
            real.extend_from_slice(&1.5f64.to_be_bytes());
            real
        },
        {
            let mut date = vec![0x33];
            date.extend_from_slice(&86_400f64.to_be_bytes());
            date
        },
        vec![0x43, 1, 2, 3],
        vec![0x81, 0x01, 0x02],
        vec![0xc1, 1],
    ];
    let bytes = binary(&objects, 0);
    let top = Document::parse(&bytes).unwrap().top();
    assert_eq!(top.kind(), Some(Kind::Array));
    assert_eq!(top.count(), Some(7));
    let items: Vec<Object<'_>> = top.items().unwrap().collect();
    assert_eq!(items[0].boolean(), Some(true));
    assert_eq!(items[1].integer(), Some(-2));
    assert_eq!(items[2].real(), Some(1.5));
    assert_eq!(items[3].date(), Some(86_400.0));
    assert_eq!(items[4].data().unwrap().as_raw(), Some(&[1u8, 2, 3][..]));
    assert_eq!(items[5].uid(), Some(0x0102));
    assert_eq!(items[6].kind(), Some(Kind::Set));
    assert_eq!(items[6].item(0).unwrap().boolean(), Some(true));
    assert_eq!(top.item(4).unwrap().index(), Some(5));
    assert!(top.item(7).is_none());
    assert!(top.get(b"x").is_none());
}

#[test]
fn a_dictionary_count_that_reaches_the_address_space_s_end_is_none() {
    let keys_at = binary::MAGIC.len() + 10;
    let count = (usize::MAX - keys_at) as u64;
    let mut dictionary = vec![0xdf, 0x13];
    dictionary.extend_from_slice(&count.to_be_bytes());
    dictionary.extend_from_slice(&[1, 2]);
    let bytes = binary(&[dictionary, ascii("x"), ascii("CFBundleIconFile")], 0);
    assert_eq!(string_of(&bytes, b"CFBundleIconFile"), None);
    assert_eq!(Document::parse(&bytes).unwrap().top().count(), None);
}

#[test]
fn a_broken_binary_list_is_an_error_or_none_never_a_panic() {
    let bytes = dictionary_plist(&[(ascii("CFBundleIconFile"), ascii("AppIcon"))]);
    for cut in 0..bytes.len() {
        if let Ok(document) = Document::parse(&bytes[..cut]) {
            let _ = document.top().get(b"CFBundleIconFile");
        }
    }
    let mut wrong = bytes.clone();
    let table_at = wrong.len() - 8;
    wrong[table_at..].copy_from_slice(&u64::MAX.to_be_bytes());
    assert_eq!(Document::parse(&wrong).err(), Some(Error::Damaged));
    let mut looping = bytes.clone();
    let offsets_at = looping.len() - 32 - 3;
    looping[offsets_at] = 200;
    let document = Document::parse(&looping).unwrap();
    assert!(document.top().kind().is_none());
    for flip in 0..bytes.len() {
        let mut damaged = bytes.clone();
        damaged[flip] ^= 0xff;
        if let Ok(document) = Document::parse(&damaged) {
            let top = document.top();
            let _ = top.kind();
            let _ = top.count();
            let _ = top.get(b"CFBundleIconFile").and_then(|value| value.text());
        }
    }
    assert_eq!(
        Document::parse(b"hello").err(),
        Some(Error::NotAPropertyList)
    );
}

const XML: &[u8] = br#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<!-- a comment -->
	<key>CFBundleName</key>
	<string>Paint &amp; Draw &#x1F95A; &#233;</string>
	<key>Count</key>
	<integer>-42</integer>
	<key>Scale</key>
	<real>2.5</real>
	<key>Enabled</key>
	<true/>
	<key>Built</key>
	<date>2001-01-02T00:00:01Z</date>
	<key>Bytes</key>
	<data>
	AQID
	BA==
	</data>
	<key>Kinds</key>
	<array>
		<string>a</string>
		<dict/>
		<array><string>nested</string></array>
		<false/>
		<string/>
	</array>
	<key>Reference</key>
	<dict><key>CF$UID</key><integer>7</integer></dict>
</dict>
</plist>
"#;

#[test]
fn xml_values_of_every_kind_read_back() {
    let document = Document::parse(XML).unwrap();
    assert_eq!(document.format(), Format::Xml);
    let top = document.top();
    assert_eq!(top.kind(), Some(Kind::Dictionary));
    assert_eq!(top.count(), Some(8));
    assert_eq!(
        string_of(XML, b"CFBundleName").as_deref(),
        Some("Paint & Draw \u{1f95a} \u{e9}")
    );
    assert_eq!(top.get(b"Count").unwrap().integer(), Some(-42));
    assert_eq!(top.get(b"Scale").unwrap().real(), Some(2.5));
    assert_eq!(top.get(b"Enabled").unwrap().boolean(), Some(true));
    assert_eq!(top.get(b"Built").unwrap().date(), Some(86_401.0));
    let data = top.get(b"Bytes").unwrap().data().unwrap();
    assert_eq!(data.len(), 4);
    let mut out = [0u8; 4];
    assert_eq!(data.write(&mut out), Some(&[1u8, 2, 3, 4][..]));
    let kinds = top.get(b"Kinds").unwrap();
    assert_eq!(kinds.count(), Some(5));
    let items: Vec<Option<Kind>> = kinds.items().unwrap().map(|item| item.kind()).collect();
    assert_eq!(
        items,
        [
            Some(Kind::String),
            Some(Kind::Dictionary),
            Some(Kind::Array),
            Some(Kind::Boolean),
            Some(Kind::String)
        ]
    );
    assert_eq!(kinds.item(1).unwrap().count(), Some(0));
    assert!(kinds.item(4).unwrap().text().unwrap().equals(b""));
    assert!(
        kinds
            .item(2)
            .unwrap()
            .item(0)
            .unwrap()
            .text()
            .unwrap()
            .equals(b"nested")
    );
    assert_eq!(top.get(b"Reference").unwrap().uid(), Some(7));
    assert_eq!(top.get(b"Kinds").unwrap().uid(), None);
    assert_eq!(top.index(), None);
}

#[test]
fn damaged_xml_is_an_error_or_none_never_a_panic() {
    assert_eq!(
        Document::parse(b"<plist version=\"1.0\"></plist>").err(),
        Some(Error::Damaged)
    );
    assert_eq!(
        Document::parse(b"<html></html>").err(),
        Some(Error::NotAPropertyList)
    );
    for cut in 0..XML.len() {
        if let Ok(document) = Document::parse(&XML[..cut]) {
            let top = document.top();
            let _ = top.count();
            let _ = top
                .get(b"Bytes")
                .and_then(|data| data.data())
                .map(|data| data.len());
            let _ = top
                .get(b"Kinds")
                .map(|kinds| kinds.items().map(Iterator::count));
        }
    }
    let bad = b"<plist><dict><key>A</key><data>A*B=</data><key>B</key><integer>x</integer></dict></plist>";
    let top = Document::parse(bad).unwrap().top();
    let data = top.get(b"A").unwrap().data().unwrap();
    assert!(!data.is_valid());
    assert_eq!(data.write(&mut [0u8; 8]), None);
    assert_eq!(top.get(b"B").unwrap().integer(), None);
    let unknown = b"<plist><string>&bogus; &#xD800;</string></plist>";
    let mut out = [0u8; 32];
    let text = Document::parse(unknown).unwrap().top().text().unwrap();
    assert_eq!(text.write_utf8(&mut out), Some(&b"&bogus; &#xD800;"[..]));
}

fn xml_integer(text: &str) -> Option<i128> {
    let document = format!("<plist><integer>{text}</integer></plist>");
    Document::parse(document.as_bytes())
        .unwrap()
        .top()
        .integer()
}

#[test]
fn an_xml_integer_takes_one_leading_sign_and_every_value_of_128_bits() {
    assert_eq!(xml_integer("5"), Some(5));
    assert_eq!(xml_integer("+5"), Some(5));
    assert_eq!(xml_integer("-0x1F"), Some(-31));
    assert_eq!(
        xml_integer("-170141183460469231731687303465884105728"),
        Some(i128::MIN)
    );
    assert_eq!(
        xml_integer("-0x80000000000000000000000000000000"),
        Some(i128::MIN)
    );
    assert_eq!(
        xml_integer("170141183460469231731687303465884105727"),
        Some(i128::MAX)
    );
}

#[test]
fn an_xml_integer_with_a_second_sign_or_out_of_range_is_none_never_a_panic() {
    for damaged in [
        "--5",
        "-+5",
        "+-5",
        "++5",
        "-0x-5",
        "0x-5",
        "0x+5",
        "-0x+5",
        "--170141183460469231731687303465884105728",
        "-0x-80000000000000000000000000000000",
        "0x-80000000000000000000000000000000",
        "170141183460469231731687303465884105728",
        "-170141183460469231731687303465884105729",
        "0x80000000000000000000000000000000",
        "-",
        "0x",
        "-0x",
        "5 5",
    ] {
        assert_eq!(xml_integer(damaged), None, "{damaged}");
    }
}
