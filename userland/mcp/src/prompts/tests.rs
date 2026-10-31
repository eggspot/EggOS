use super::*;
use crate::json;

fn get(name: &str, arguments: &str) -> Result<String, PromptError> {
    let prompt = find(name).ok_or(PromptError::Unknown)?;
    let parsed = json::parse(arguments.as_bytes()).unwrap();
    let mut out = [0u8; 2048];
    let mut writer = JsonWriter::new(&mut out);
    write_get(prompt, parsed.value.as_object(), &mut writer)?;
    let length = writer.len();
    Ok(String::from_utf8(out[..length].to_vec()).unwrap())
}

#[test]
fn every_prompt_is_listed_with_its_required_arguments() {
    let mut out = [0u8; 4096];
    let mut writer = JsonWriter::new(&mut out);
    write_list(&mut writer);
    let length = writer.len();
    let text = core::str::from_utf8(&out[..length]).unwrap();
    assert!(json::parse(text.as_bytes()).is_ok(), "{text}");
    for prompt in &PROMPTS {
        assert!(text.contains(&format!("\"name\":\"{}\"", prompt.name)));
        for argument in prompt.arguments {
            assert!(prompt.text.contains(&format!("{{{}}}", argument.name)));
        }
        assert!(prompt.arguments.len() <= 2);
    }
    assert_eq!(text.matches("\"required\":true").count(), 3);
}

#[test]
fn a_prompt_fills_in_its_arguments_as_a_user_message() {
    let text = get("look-at-window", r#"{"window":"7"}"#).unwrap();
    assert!(
        text.contains(
            "\"messages\":[{\"role\":\"user\",\"content\":{\"type\":\"text\",\"text\":\"Show me \
         window 7: call wm_show with window 7,"
        ),
        "{text}"
    );
    let quoted = get("tidy-folder", r#"{"folder":"/home/root/a \"b\""}"#).unwrap();
    assert!(
        quoted.contains("List /home/root/a \\\"b\\\" with ls"),
        "{quoted}"
    );
    assert!(
        get("check-the-machine", "{}")
            .unwrap()
            .contains("device_list")
    );
}

#[test]
fn missing_or_wrong_arguments_and_unknown_prompts_are_refused() {
    assert_eq!(get("nothing", "{}"), Err(PromptError::Unknown));
    assert_eq!(get("install-app", "{}"), Err(PromptError::Argument("path")));
    assert_eq!(
        get("install-app", r#"{"path":5}"#),
        Err(PromptError::Argument("path"))
    );
    let long = format!(r#"{{"path":"{}"}}"#, "a".repeat(MAX_ARGUMENT_BYTES + 1));
    assert_eq!(
        get("install-app", &long),
        Err(PromptError::Argument("path"))
    );
}
