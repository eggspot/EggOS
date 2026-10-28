//! JSON-RPC 2.0 messages as MCP uses them: requests, notifications and responses, each one
//! JSON object; what is not one is answered with the specification's error codes. Batches
//! (arrays) are refused: no protocol version this server speaks has them.

#![deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]

use crate::json::{self, JsonError, JsonWriter, Object, Str, Value};

/// Invalid JSON.
pub const PARSE_ERROR: i64 = -32700;
/// JSON that is not a valid request.
pub const INVALID_REQUEST: i64 = -32600;
/// A method the server does not have.
pub const METHOD_NOT_FOUND: i64 = -32601;
/// Parameters the method does not take.
pub const INVALID_PARAMS: i64 = -32602;
/// The server failed.
pub const INTERNAL_ERROR: i64 = -32603;

/// The longest request id echoed back, in bytes as written.
pub const MAX_ID_BYTES: usize = 256;

/// A message's kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Message<'a> {
    /// A request: an answer is due, echoing its id as written; its parameters object, if
    /// any, and whether an object inside them names a key twice.
    Request {
        id: &'a [u8],
        method: Str<'a>,
        params: Option<Object<'a>>,
        nested_duplicate: bool,
    },
    /// A notification (its method and parameters): no answer.
    Notification {
        method: Str<'a>,
        params: Option<Object<'a>>,
    },
    /// An answer to a request of ours (`elicitation/create`, D137): its id as written, and
    /// what it said.
    Response { id: &'a [u8], answer: Answer },
}

/// What a client answered to `elicitation/create`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    /// The user accepted; whether the form's `confirm` was `true`.
    Accept { confirm: bool },
    /// The user declined.
    Decline,
    /// The user dismissed the question.
    Cancel,
    /// The client answered with an error (its code).
    Error(i64),
    /// The answer is neither.
    Malformed,
}

fn answer_of(object: Object<'_>) -> Answer {
    if let Some(error) = object.get("error") {
        let code = error
            .get("code")
            .and_then(|code| core::str::from_utf8(code.raw()).ok())
            .and_then(|code| code.parse().ok())
            .unwrap_or(0);
        return Answer::Error(code);
    }
    let Some(result) = object.get("result") else {
        return Answer::Malformed;
    };
    let action = result.get("action").and_then(Value::as_str);
    match action {
        Some(action) if action.equals("accept") => Answer::Accept {
            confirm: result
                .get("content")
                .and_then(|content| content.get("confirm"))
                == Some(Value::Bool(true)),
        },
        Some(action) if action.equals("decline") => Answer::Decline,
        Some(action) if action.equals("cancel") => Answer::Cancel,
        _ => Answer::Malformed,
    }
}

/// Why a message is refused, and the id to answer with if it had a usable one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Refusal<'a> {
    /// The id to answer with; `null` without one.
    pub id: Option<&'a [u8]>,
    /// The error code.
    pub code: i64,
    /// What is wrong.
    pub message: &'static str,
}

fn refuse<'a>(id: Option<&'a [u8]>, code: i64, message: &'static str) -> Refusal<'a> {
    Refusal { id, code, message }
}

/// Reads `line` as one JSON-RPC message.
pub fn classify(line: &[u8]) -> Result<Message<'_>, Refusal<'_>> {
    let parsed = match json::parse(line) {
        Ok(parsed) => parsed,
        Err(JsonError::DuplicateKey) => {
            return Err(refuse(
                None,
                INVALID_REQUEST,
                "a key appears twice in an object",
            ));
        }
        Err(JsonError::TooDeep | JsonError::TooManyValues | JsonError::TooManyMembers) => {
            return Err(refuse(
                None,
                INVALID_REQUEST,
                "the message is too large or too deep",
            ));
        }
        Err(JsonError::InvalidUtf8 | JsonError::Syntax(_)) => {
            return Err(refuse(None, PARSE_ERROR, "the message is not valid JSON"));
        }
    };
    let object = match parsed.value {
        Value::Object(object) => object,
        Value::Array(_) => {
            return Err(refuse(None, INVALID_REQUEST, "batches are not supported"));
        }
        _ => return Err(refuse(None, INVALID_REQUEST, "a message is a JSON object")),
    };
    let id = match object.get("id") {
        None => None,
        Some(value @ (Value::Number(_) | Value::Str(_))) if value.raw().len() <= MAX_ID_BYTES => {
            Some(value.raw())
        }
        Some(Value::Number(_) | Value::Str(_)) => {
            return Err(refuse(None, INVALID_REQUEST, "the id is too long"));
        }
        Some(_) => {
            return Err(refuse(
                None,
                INVALID_REQUEST,
                "an id is a string or a number",
            ));
        }
    };
    if !object
        .get("jsonrpc")
        .and_then(Value::as_str)
        .is_some_and(|version| version.equals("2.0"))
    {
        return Err(refuse(id, INVALID_REQUEST, "jsonrpc must be \"2.0\""));
    }
    let method = match object.get("method") {
        Some(Value::Str(method)) => method,
        Some(_) => return Err(refuse(id, INVALID_REQUEST, "a method is a string")),
        None if object.get("result").is_some() || object.get("error").is_some() => {
            return match id {
                Some(id) => Ok(Message::Response {
                    id,
                    answer: answer_of(object),
                }),
                None => Err(refuse(None, INVALID_REQUEST, "the message has no method")),
            };
        }
        None => return Err(refuse(id, INVALID_REQUEST, "the message has no method")),
    };
    let params = match object.get("params") {
        None => None,
        Some(Value::Object(params)) => Some(params),
        Some(_) => return Err(refuse(id, INVALID_PARAMS, "params must be an object")),
    };
    match id {
        Some(id) => Ok(Message::Request {
            id,
            method,
            params,
            nested_duplicate: parsed.nested_duplicate,
        }),
        None => Ok(Message::Notification { method, params }),
    }
}

/// Writes a response with `id` (or `null`) whose result `result` writes.
pub fn write_result(out: &mut JsonWriter<'_>, id: &[u8], result: impl FnOnce(&mut JsonWriter<'_>)) {
    out.begin_object();
    out.key("jsonrpc");
    out.string("2.0");
    out.key("id");
    out.raw(id);
    out.key("result");
    result(out);
    out.end_object();
}

/// Writes an error response with `id` (or `null` without one).
pub fn write_error(out: &mut JsonWriter<'_>, id: Option<&[u8]>, code: i64, message: &str) {
    out.begin_object();
    out.key("jsonrpc");
    out.string("2.0");
    out.key("id");
    match id {
        Some(id) => out.raw(id),
        None => out.null(),
    }
    out.key("error");
    out.begin_object();
    out.key("code");
    out.i64(code);
    out.key("message");
    out.string(message);
    out.end_object();
    out.end_object();
}

#[cfg(test)]
mod tests {
    #![allow(clippy::indexing_slicing, clippy::unwrap_used, clippy::panic)]

    use super::*;

    fn error_of(line: &str) -> (Option<Vec<u8>>, i64) {
        let refusal = classify(line.as_bytes()).unwrap_err();
        (refusal.id.map(<[u8]>::to_vec), refusal.code)
    }

    #[test]
    fn requests_notifications_and_responses_are_told_apart() {
        let Ok(Message::Request {
            id,
            method,
            params,
            nested_duplicate,
        }) = classify(br#"{"jsonrpc":"2.0","id":"a-1","method":"tools/list","params":{}}"#)
        else {
            panic!("a request");
        };
        assert_eq!(id, br#""a-1""#);
        assert!(method.equals("tools/list"));
        assert!(params.is_some());
        assert!(!nested_duplicate);
        assert!(matches!(
            classify(br#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#),
            Ok(Message::Notification { .. })
        ));
        assert_eq!(
            classify(br#"{"jsonrpc":"2.0","id":3,"result":{}}"#),
            Ok(Message::Response {
                id: b"3",
                answer: Answer::Malformed
            })
        );
        let Ok(Message::Request { nested_duplicate, .. }) = classify(
            br#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"arguments":{"p":1,"p":2}}}"#,
        ) else {
            panic!("a request");
        };
        assert!(nested_duplicate);
    }

    #[test]
    fn answers_to_our_questions_are_read() {
        let answer = |line: &str| match classify(line.as_bytes()) {
            Ok(Message::Response { answer, .. }) => answer,
            other => panic!("{other:?}"),
        };
        assert_eq!(
            answer(
                r#"{"jsonrpc":"2.0","id":"q","result":{"action":"accept","content":{"confirm":true}}}"#
            ),
            Answer::Accept { confirm: true }
        );
        assert_eq!(
            answer(
                r#"{"jsonrpc":"2.0","id":"q","result":{"action":"accept","content":{"confirm":"true"}}}"#
            ),
            Answer::Accept { confirm: false }
        );
        assert_eq!(
            answer(r#"{"jsonrpc":"2.0","id":"q","result":{"action":"accept"}}"#),
            Answer::Accept { confirm: false }
        );
        assert_eq!(
            answer(r#"{"jsonrpc":"2.0","id":"q","result":{"action":"decline"}}"#),
            Answer::Decline
        );
        assert_eq!(
            answer(r#"{"jsonrpc":"2.0","id":"q","result":{"action":"cancel"}}"#),
            Answer::Cancel
        );
        assert_eq!(
            answer(r#"{"jsonrpc":"2.0","id":"q","error":{"code":-32602,"message":"no"}}"#),
            Answer::Error(-32602)
        );
        assert_eq!(
            answer(r#"{"jsonrpc":"2.0","id":"q","result":{"action":"maybe"}}"#),
            Answer::Malformed
        );
        let Ok(Message::Notification { params, .. }) = classify(
            br#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":3}}"#,
        ) else {
            panic!("a notification");
        };
        assert!(params.unwrap().get("requestId").is_some());
        assert_eq!(
            error_of(r#"{"jsonrpc":"2.0","result":{}}"#),
            (None, INVALID_REQUEST)
        );
    }

    #[test]
    fn what_is_not_a_request_gets_the_specification_s_codes() {
        assert_eq!(error_of("{"), (None, PARSE_ERROR));
        assert_eq!(error_of("[]"), (None, INVALID_REQUEST));
        assert_eq!(
            error_of("[{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}]"),
            (None, INVALID_REQUEST)
        );
        assert_eq!(error_of("1"), (None, INVALID_REQUEST));
        assert_eq!(
            error_of(r#"{"jsonrpc":"1.0","id":7,"method":"ping"}"#),
            (Some(b"7".to_vec()), INVALID_REQUEST)
        );
        assert_eq!(
            error_of(r#"{"jsonrpc":"2.0","id":7,"method":"ping","method":"x"}"#),
            (None, INVALID_REQUEST)
        );
        assert_eq!(
            error_of(r#"{"jsonrpc":"2.0","id":null,"method":"ping"}"#),
            (None, INVALID_REQUEST)
        );
        assert_eq!(
            error_of(r#"{"jsonrpc":"2.0","id":7,"method":1}"#),
            (Some(b"7".to_vec()), INVALID_REQUEST)
        );
        assert_eq!(
            error_of(r#"{"jsonrpc":"2.0","id":7,"method":"x","params":[1]}"#),
            (Some(b"7".to_vec()), INVALID_PARAMS)
        );
        assert_eq!(
            error_of(r#"{"jsonrpc":"2.0","id":7}"#),
            (Some(b"7".to_vec()), INVALID_REQUEST)
        );
    }

    #[test]
    fn ids_up_to_the_cap_are_echoed_and_longer_ones_refused() {
        let longest = format!("\"{}\"", "i".repeat(MAX_ID_BYTES - 2));
        let line = format!(r#"{{"jsonrpc":"2.0","id":{longest},"method":"ping"}}"#);
        let Ok(Message::Request { id, .. }) = classify(line.as_bytes()) else {
            panic!("a request");
        };
        assert_eq!(id, longest.as_bytes());
        let longer = format!("\"{}\"", "i".repeat(MAX_ID_BYTES - 1));
        let line = format!(r#"{{"jsonrpc":"2.0","id":{longer},"method":"ping"}}"#);
        assert_eq!(error_of(&line), (None, INVALID_REQUEST));
    }

    #[test]
    fn answers_are_one_json_object_with_the_id() {
        let mut buffer = [0u8; 256];
        let mut out = JsonWriter::new(&mut buffer);
        write_result(&mut out, b"\"x\"", |out| {
            out.begin_object();
            out.end_object();
        });
        assert_eq!(out.as_bytes(), br#"{"jsonrpc":"2.0","id":"x","result":{}}"#);
        out.clear();
        write_error(&mut out, None, PARSE_ERROR, "bad");
        assert_eq!(
            out.as_bytes(),
            br#"{"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"bad"}}"#
        );
    }
}
