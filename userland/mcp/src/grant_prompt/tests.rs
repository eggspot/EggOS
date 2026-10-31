use super::*;

fn question(arguments: &[u8]) -> Question<'_> {
    Question {
        agent: "phone",
        name: "Claude on my phone",
        tool: "tee",
        title: "Write a file",
        class: Some(Grant::Write),
        destructive: false,
        arguments,
        timeout_s: GRANT_PROMPT_TIMEOUT_S,
    }
}

#[test]
fn a_question_is_one_json_line_the_prompt_reads_back() {
    let mut out = [0u8; QUESTION_BYTES];
    let length = write_question(
        &question(br#"{"path":"/home/a","content":"40 bytes"}"#),
        &mut out,
    );
    let line = &out[..length];
    assert_eq!(
        line,
        &br#"{"agent":"phone","name":"Claude on my phone","tool":"tee","title":"Write a file","class":"write","destructive":false,"arguments":{"path":"/home/a","content":"40 bytes"},"timeout_s":90}
"#[..]
    );
    let read = read_question(line).expect("a question");
    assert!(read.agent.equals("phone"));
    assert!(read.name.equals("Claude on my phone"));
    assert!(read.tool.equals("tee"));
    assert!(read.title.equals("Write a file"));
    assert_eq!(read.class, Some(Grant::Write));
    assert!(!read.destructive);
    assert_eq!(read.timeout_s, 90);
    let keys: Vec<String> = read
        .arguments
        .members()
        .map(|(key, _)| key.chars().collect())
        .collect();
    assert_eq!(keys, ["path", "content"]);
}

#[test]
fn a_question_that_does_not_fit_or_has_no_argument_object_is_not_written() {
    let mut small = [0u8; 32];
    assert_eq!(write_question(&question(b"{}"), &mut small), 0);
    let mut out = [0u8; QUESTION_BYTES];
    assert_eq!(write_question(&question(b"[1]"), &mut out), 0);
    assert_eq!(write_question(&question(b"{"), &mut out), 0);
}

#[test]
fn names_are_escaped_so_a_name_cannot_add_fields() {
    let mut out = [0u8; QUESTION_BYTES];
    let mut asked = question(b"{}");
    asked.name = "x\",\"class\":\"run";
    let length = write_question(&asked, &mut out);
    let read = read_question(&out[..length]).expect("a question");
    assert_eq!(read.class, Some(Grant::Write));
    assert_eq!(read.name.chars().collect::<String>(), "x\",\"class\":\"run");
}

#[test]
fn a_question_without_a_name_or_timeout_takes_the_id_and_90_s() {
    let line = br#"{"agent":"local","tool":"trash","title":"Move to the Trash","class":null,"arguments":{}}"#;
    let read = read_question(line).expect("a question");
    assert!(read.name.equals("local"));
    assert_eq!(read.class, None);
    assert!(!read.destructive);
    assert_eq!(read.timeout_s, GRANT_PROMPT_TIMEOUT_S);
}

#[test]
fn anything_else_is_not_a_question() {
    for line in [
        &b""[..],
        b"once",
        b"[]",
        br#"{"agent":"a","tool":"t","title":"T","class":"root","arguments":{}}"#,
        br#"{"agent":"a","tool":"t","title":"T","class":"write","arguments":[]}"#,
        br#"{"agent":"a","tool":"t","title":"T","class":"write","destructive":1,"arguments":{}}"#,
        br#"{"tool":"t","title":"T","class":"write","arguments":{}}"#,
    ] {
        assert_eq!(
            read_question(line),
            None,
            "{:?}",
            String::from_utf8_lossy(line)
        );
    }
}

#[test]
fn answers_are_one_word_with_an_optional_newline() {
    for answer in PromptAnswer::ALL {
        assert_eq!(
            PromptAnswer::from_line(answer.word().as_bytes()),
            Some(answer)
        );
        let mut line = answer.word().as_bytes().to_vec();
        line.push(b'\n');
        assert_eq!(PromptAnswer::from_line(&line), Some(answer));
    }
    for line in [&b""[..], b"\n", b"Once", b"once\n\n", b" deny", b"allow"] {
        assert_eq!(PromptAnswer::from_line(line), None);
    }
}

#[test]
fn always_is_offered_for_view_and_write_calls_that_need_no_yes() {
    assert!(offers_always(Some(Grant::View), false));
    assert!(offers_always(Some(Grant::Write), false));
    assert!(!offers_always(Some(Grant::Write), true));
    assert!(!offers_always(Some(Grant::Run), false));
    assert!(!offers_always(Some(Grant::Install), false));
    assert!(!offers_always(None, false));
}
