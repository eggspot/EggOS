//! A checker of a JSON value against the subset of JSON Schema the tools' `outputSchema` uses
//! (D226): `type` (a name or a list of names), `enum`, `required`, `properties` with
//! `additionalProperties: false`, `items` and `oneOf`. Tests hold real output to the schemas
//! `tools/list` declares with it, here and in the crates whose commands are the twins.

extern crate alloc;

use crate::json::{Str, Value};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

fn type_holds(value: Value<'_>, name: &str) -> bool {
    match (name, value) {
        ("object", Value::Object(_))
        | ("array", Value::Array(_))
        | ("string", Value::Str(_))
        | ("boolean", Value::Bool(_))
        | ("null", Value::Null)
        | ("number", Value::Number(_)) => true,
        ("integer", Value::Number(number)) => {
            number.as_i64().is_some() || number.as_u64().is_some()
        }
        _ => false,
    }
}

fn decoded(text: Str<'_>) -> String {
    text.chars().collect()
}

fn type_names(types: Value<'_>) -> Result<Vec<String>, String> {
    match types {
        Value::Str(name) => Ok(alloc::vec![decoded(name)]),
        Value::Array(names) => names
            .elements()
            .map(|name| {
                name.as_str()
                    .map(decoded)
                    .ok_or_else(|| String::from("a type's name is a string"))
            })
            .collect(),
        _ => Err(String::from("a schema's type is a name or a list of names")),
    }
}

/// Whether `value` meets `schema`: `Ok`, or what does not, naming the member's path.
pub fn conforms(value: Value<'_>, schema: Value<'_>) -> Result<(), String> {
    let shown = || String::from_utf8_lossy(value.raw()).into_owned();
    if let Some(types) = schema.get("type") {
        let names = type_names(types)?;
        if !names.iter().any(|name| type_holds(value, name)) {
            return Err(format!("{} is not of the types {names:?}", shown()));
        }
    }
    if let Some(choices @ Value::Array(listed)) = schema.get("enum")
        && !listed.elements().any(|choice| choice.raw() == value.raw())
    {
        return Err(format!(
            "{} is none of {}",
            shown(),
            String::from_utf8_lossy(choices.raw())
        ));
    }
    if let Some(Value::Array(required)) = schema.get("required") {
        for name in required.elements() {
            let name = name
                .as_str()
                .map(decoded)
                .ok_or_else(|| String::from("a required name is a string"))?;
            if value.get(&name).is_none() {
                return Err(format!("{} lacks {name}", shown()));
            }
        }
    }
    if let Value::Object(object) = value {
        let closed = schema.get("additionalProperties") == Some(Value::Bool(false));
        for (key, member) in object.members() {
            let key = decoded(key);
            match schema.get("properties").and_then(|named| named.get(&key)) {
                Some(property) => {
                    conforms(member, property).map_err(|why| format!("{key}: {why}"))?;
                }
                None if closed => {
                    return Err(format!(
                        "{} has {key}, which its schema does not name",
                        shown()
                    ));
                }
                None => {}
            }
        }
    }
    if let (Value::Array(elements), Some(items)) = (value, schema.get("items")) {
        for (index, element) in elements.elements().enumerate() {
            conforms(element, items).map_err(|why| format!("item {index}: {why}"))?;
        }
    }
    if let Some(Value::Array(choices)) = schema.get("oneOf") {
        let matching = choices
            .elements()
            .filter(|choice| conforms(value, *choice).is_ok())
            .count();
        if matching != 1 {
            return Err(format!("{} matches {matching} of oneOf's schemas", shown()));
        }
    }
    Ok(())
}
