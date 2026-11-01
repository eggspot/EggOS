#![allow(clippy::panic, clippy::unwrap_used, clippy::indexing_slicing)]

use super::*;

fn value(text: &str) -> Value<'_> {
    parse(text.as_bytes()).unwrap().value
}

fn written(write: impl FnOnce(&mut JsonWriter<'_>)) -> String {
    let mut buffer = vec![0u8; 4096];
    let mut writer = JsonWriter::new(&mut buffer);
    write(&mut writer);
    assert!(!writer.overflowed());
    String::from_utf8(writer.as_bytes().to_vec()).unwrap()
}

#[test]
fn every_kind_of_value_parses_and_reads_back() {
    let parsed = value(
        r#" {"a": null, "b": true, "c": false, "d": -12.5e+3, "e": "x\ty", "f": [1, [2], {}],
            "g": {"h": 7}} "#,
    );
    assert_eq!(parsed.get("a"), Some(Value::Null));
    assert_eq!(parsed.get("b"), Some(Value::Bool(true)));
    assert_eq!(parsed.get("c"), Some(Value::Bool(false)));
    assert_eq!(parsed.get("d").unwrap().raw(), b"-12.5e+3");
    let mut buffer = [0u8; 8];
    assert_eq!(
        parsed
            .get("e")
            .unwrap()
            .as_str()
            .unwrap()
            .decode_into(&mut buffer),
        Some("x\ty")
    );
    let Some(Value::Array(array)) = parsed.get("f") else {
        panic!("f is an array");
    };
    let elements: Vec<&[u8]> = array.elements().map(Value::raw).collect();
    assert_eq!(elements, [&b"1"[..], b"[2]", b"{}"]);
    let Some(Value::Number(seven)) = parsed.get("g").unwrap().get("h") else {
        panic!("h is a number");
    };
    assert_eq!(seven.as_u64(), Some(7));
    assert_eq!(parsed.get("missing"), None);
}

#[test]
fn the_grammar_is_rfc_8259_s() {
    for good in [
        "0",
        "-0",
        "1.5",
        "1e5",
        "1E-5",
        "\"\"",
        "[]",
        "{}",
        " [ 1 , 2 ] ",
        "\"\\u00e9\"",
        "\"\\/\"",
    ] {
        assert!(parse(good.as_bytes()).is_ok(), "{good}");
    }
    for bad in [
        "",
        " ",
        "01",
        "1.",
        ".5",
        "-",
        "1e",
        "+1",
        "[1,]",
        "{\"a\":1,}",
        "[1 2]",
        "{\"a\" 1}",
        "{a:1}",
        "'a'",
        "\"\\x\"",
        "\"\\u12\"",
        "\"\t\"",
        "nul",
        "true false",
        "[",
        "{",
        "\"",
        "{\"a\":}",
        "[,]",
        "NaN",
        "Infinity",
    ] {
        assert!(parse(bad.as_bytes()).is_err(), "{bad}");
    }
    assert_eq!(parse(b"\xff"), Err(JsonError::InvalidUtf8));
    assert_eq!(parse(b"[1, x]"), Err(JsonError::Syntax(4)));
}

#[test]
fn nesting_values_and_members_are_bounded() {
    let deep = "[".repeat(MAX_DEPTH) + &"]".repeat(MAX_DEPTH);
    assert!(parse(deep.as_bytes()).is_ok());
    let deeper = "[".repeat(MAX_DEPTH + 1) + &"]".repeat(MAX_DEPTH + 1);
    assert_eq!(parse(deeper.as_bytes()), Err(JsonError::TooDeep));
    let unclosed = "[".repeat(100_000);
    assert_eq!(parse(unclosed.as_bytes()), Err(JsonError::TooDeep));
    let many = format!("[{}0]", "0,".repeat(MAX_VALUES));
    assert_eq!(parse(many.as_bytes()), Err(JsonError::TooManyValues));
    let members: Vec<String> = (0..=MAX_MEMBERS)
        .map(|index| format!("\"k{index}\":0"))
        .collect();
    let wide = format!("{{{}}}", members.join(","));
    assert_eq!(parse(wide.as_bytes()), Err(JsonError::TooManyMembers));
}

#[test]
fn duplicate_keys_are_refused_near_the_top_and_reported_deeper() {
    assert_eq!(parse(br#"{"a":1,"a":2}"#), Err(JsonError::DuplicateKey));
    assert_eq!(
        parse(br#"{"a":1,"\u0061":2}"#),
        Err(JsonError::DuplicateKey)
    );
    assert_eq!(
        parse(br#"{"p":{"x":1,"x":2}}"#),
        Err(JsonError::DuplicateKey)
    );
    let nested = parse(br#"{"p":{"q":{"x":1,"\u0078":2}}}"#).unwrap();
    assert!(nested.nested_duplicate);
    let fine = parse(br#"{"p":{"q":{"x":1,"y":2}},"x":3}"#).unwrap();
    assert!(!fine.nested_duplicate);
}

#[test]
fn strings_decode_escapes_and_surrogates() {
    let mut buffer = [0u8; 64];
    let decode = |text: &str, buffer: &mut [u8; 64]| -> String {
        let value = parse(text.as_bytes()).unwrap().value;
        value
            .as_str()
            .unwrap()
            .decode_into(buffer)
            .unwrap()
            .to_owned()
    };
    assert_eq!(
        decode(r#""a\"b\\c\/d\b\f\n\r\t""#, &mut buffer),
        "a\"b\\c/d\u{8}\u{c}\n\r\t"
    );
    assert_eq!(decode(r#""\u0041\u00e9\u20ac""#, &mut buffer), "Aé€");
    assert_eq!(decode(r#""\ud83d\ude00""#, &mut buffer), "😀");
    assert_eq!(decode(r#""\ud83d x""#, &mut buffer), "\u{fffd} x");
    assert_eq!(decode(r#""\ude00""#, &mut buffer), "\u{fffd}");
    assert_eq!(decode(r#""\u002e\u002e\u0000""#, &mut buffer), "..\0");
    assert_eq!(decode("\"é😀\"", &mut buffer), "é😀");
    let long = value("\"abcdef\"").as_str().unwrap();
    assert_eq!(long.decode_into(&mut [0u8; 3]), None);
    assert_eq!(long.decode_prefix_into(&mut [0u8; 3]), ("abc", false));
    assert_eq!(long.decode_prefix_into(&mut [0u8; 8]), ("abcdef", true));
    assert_eq!(value(r#""\u00e9""#).as_str().unwrap().decoded_len(), 2);
    assert!(
        value(r#""tools\/list""#)
            .as_str()
            .unwrap()
            .equals("tools/list")
    );
    assert_eq!(value(r#""x""#).raw(), br#""x""#);
}

#[test]
fn numbers_are_read_as_unsigned_integers_only_when_they_are() {
    for (text, expected) in [
        ("0", Some(0)),
        ("42", Some(42)),
        ("18446744073709551615", Some(u64::MAX)),
        ("18446744073709551616", None),
        ("-1", None),
        ("1.0", None),
        ("1e3", None),
    ] {
        let Value::Number(number) = value(text) else {
            panic!("{text} is a number");
        };
        assert_eq!(number.as_u64(), expected, "{text}");
    }
}

#[test]
fn numbers_are_read_as_signed_integers_only_when_they_fit() {
    for (text, expected) in [
        ("0", Some(0)),
        ("-0", Some(0)),
        ("-16", Some(-16)),
        ("9223372036854775807", Some(i64::MAX)),
        ("9223372036854775808", None),
        ("-9223372036854775808", Some(i64::MIN)),
        ("-9223372036854775809", None),
        ("-1.5", None),
        ("-1e3", None),
    ] {
        let Value::Number(number) = value(text) else {
            panic!("{text} is a number");
        };
        assert_eq!(number.as_i64(), expected, "{text}");
    }
}

#[test]
fn the_writer_places_commas_and_escapes() {
    let text = written(|writer| {
        writer.begin_object();
        writer.key("a");
        writer.u64(1);
        writer.key("b");
        writer.begin_array();
        writer.string("x\"\\\n\u{1}");
        writer.bool(true);
        writer.null();
        writer.i64(-5);
        writer.begin_object();
        writer.end_object();
        writer.end_array();
        writer.key("c");
        writer.raw(b"{\"d\":[]}");
        writer.key("e");
        writer.string_lossy(b"ok\xffend");
        writer.end_object();
    });
    assert_eq!(
        text,
        r#"{"a":1,"b":["x\"\\\n\u0001",true,null,-5,{}],"c":{"d":[]},"e":"ok�end"}"#
    );
    assert!(parse(text.as_bytes()).is_ok());
}

#[test]
fn base64_is_rfc_4648_s() {
    for (bytes, encoded) in [
        (&b""[..], "\"\""),
        (b"f", "\"Zg==\""),
        (b"fo", "\"Zm8=\""),
        (b"foo", "\"Zm9v\""),
        (b"foob", "\"Zm9vYg==\""),
        (b"\xff\x00\xfe", "\"/wD+\""),
    ] {
        assert_eq!(written(|writer| writer.string_base64(bytes)), encoded);
    }
}

#[test]
fn base64_in_pieces_of_any_length_is_the_text_of_one_piece() {
    let bytes: Vec<u8> = (0..=255u8).cycle().take(1000).collect();
    let whole = written(|writer| writer.string_base64(&bytes));
    let mut buffer = vec![0u8; 4096];
    for piece in [1, 2, 3, 4, 5, 7, 131, 999, 1000] {
        let mut writer = JsonWriter::new(&mut buffer);
        writer.begin_string();
        let mut carry = Base64Carry::default();
        for chunk in bytes.chunks(piece) {
            writer.base64_piece(&mut carry, chunk);
        }
        writer.base64_piece(&mut carry, &[]);
        writer.base64_end(carry);
        writer.end_string();
        assert_eq!(writer.as_bytes(), whole.as_bytes(), "pieces of {piece}");
    }
    let uneven = [1usize, 2, 4, 1, 3, 989];
    let mut writer = JsonWriter::new(&mut buffer);
    writer.begin_string();
    let mut carry = Base64Carry::default();
    let mut start = 0;
    for length in uneven {
        writer.base64_piece(&mut carry, &bytes[start..start + length]);
        start += length;
    }
    writer.base64_end(carry);
    writer.end_string();
    assert_eq!(writer.as_bytes(), whole.as_bytes());
}

#[test]
fn a_full_writer_says_so_and_rewinds_cleanly() {
    let mut buffer = [0u8; 16];
    let mut writer = JsonWriter::new(&mut buffer);
    writer.begin_object();
    let mark = writer.mark();
    writer.key("long");
    writer.string("this does not fit at all");
    assert!(writer.overflowed());
    writer.rewind(mark);
    assert!(!writer.overflowed());
    writer.key("k");
    writer.u64(1);
    writer.end_object();
    assert_eq!(writer.as_bytes(), br#"{"k":1}"#);
    assert_eq!(writer.remaining(), 16 - 7);
    writer.clear();
    assert!(writer.is_empty());
}

#[test]
fn every_control_byte_is_escaped_and_the_text_round_trips() {
    let all: String = (0u8..0x80).map(char::from).collect::<String>() + "é😀\u{2028}";
    let text = written(|writer| writer.string(&all));
    let parsed = parse(text.as_bytes()).unwrap().value.as_str().unwrap();
    let mut buffer = vec![0u8; 512];
    assert_eq!(parsed.decode_into(&mut buffer), Some(all.as_str()));
    assert!(text.bytes().all(|byte| byte >= 0x20));
}

#[test]
fn an_object_written_last_reopens_for_more_members() {
    let text = written(|out| {
        out.begin_object();
        out.key("result");
        out.begin_object();
        out.key("a");
        out.u64(1);
        out.end_object();
        out.end_object();
        assert!(out.reopen_object());
        assert!(out.reopen_object());
        out.key("b");
        out.u64(2);
        out.end_object();
        out.key("c");
        out.bool(true);
        out.end_object();
    });
    assert_eq!(text, r#"{"result":{"a":1,"b":2},"c":true}"#);
    let empty = written(|out| {
        out.begin_object();
        out.end_object();
        assert!(out.reopen_object());
        out.key("only");
        out.null();
        out.end_object();
        out.begin_array();
        out.end_array();
        assert!(!out.reopen_object());
    });
    assert_eq!(empty, r#"{"only":null}[]"#);
}
