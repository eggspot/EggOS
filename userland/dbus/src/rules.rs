//! Match rules: which broadcast signals a connection asked the bus for (`AddMatch`), as
//! comma-separated `key='value'` pairs.

use crate::message::{Message, kind};
use alloc::vec::Vec;

/// The highest argument index a rule may name.
pub const MAX_ARGUMENT: u8 = 63;

/// How an argument key compares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgumentTest {
    /// `argN`: the string argument equals the value.
    Equal,
    /// `argNpath`: one is the other, or a prefix of it ending in `/`.
    Path,
    /// `arg0namespace`: the bus or interface name is the value or inside it.
    Namespace,
}

/// A parsed match rule.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rule {
    pub kind: Option<u8>,
    pub sender: Option<Vec<u8>>,
    pub interface: Option<Vec<u8>>,
    pub member: Option<Vec<u8>>,
    pub path: Option<Vec<u8>>,
    pub path_namespace: Option<Vec<u8>>,
    pub destination: Option<Vec<u8>>,
    pub arguments: Vec<(u8, ArgumentTest, Vec<u8>)>,
}

fn pairs(text: &[u8]) -> Option<Vec<(Vec<u8>, Vec<u8>)>> {
    let mut pairs = Vec::new();
    let mut at = 0;
    while at < text.len() {
        while text.get(at) == Some(&b' ') {
            at += 1;
        }
        let equals = at + text[at..].iter().position(|&byte| byte == b'=')?;
        let key = text[at..equals].to_vec();
        at = equals + 1;
        let mut value = Vec::new();
        let mut quoted = false;
        while at < text.len() {
            let byte = text[at];
            at += 1;
            match (quoted, byte) {
                (true, b'\'') => quoted = false,
                (true, _) => value.push(byte),
                (false, b'\'') => quoted = true,
                (false, b'\\') if text.get(at) == Some(&b'\'') => {
                    value.push(b'\'');
                    at += 1;
                }
                (false, b',') => break,
                (false, _) => value.push(byte),
            }
        }
        if quoted || key.is_empty() {
            return None;
        }
        pairs.push((key, value));
    }
    Some(pairs)
}

fn argument_key(key: &[u8]) -> Option<(u8, ArgumentTest)> {
    let digits = key.strip_prefix(b"arg")?;
    let count = digits.iter().take_while(|byte| byte.is_ascii_digit()).count();
    if count == 0 || count > 2 {
        return None;
    }
    let index = core::str::from_utf8(&digits[..count]).ok()?.parse::<u8>().ok()?;
    if index > MAX_ARGUMENT {
        return None;
    }
    let test = match &digits[count..] {
        b"" => ArgumentTest::Equal,
        b"path" => ArgumentTest::Path,
        b"namespace" if index == 0 => ArgumentTest::Namespace,
        _ => return None,
    };
    Some((index, test))
}

impl Rule {
    /// The rule `text` writes; `None` when it is not a valid rule.
    pub fn parse(text: &[u8]) -> Option<Self> {
        let mut rule = Self::default();
        for (key, value) in pairs(text)? {
            let slot = match key.as_slice() {
                b"type" => {
                    rule.kind = Some(match value.as_slice() {
                        b"method_call" => kind::METHOD_CALL,
                        b"method_return" => kind::METHOD_RETURN,
                        b"error" => kind::ERROR,
                        b"signal" => kind::SIGNAL,
                        _ => return None,
                    });
                    continue;
                }
                b"eavesdrop" => continue,
                b"sender" => &mut rule.sender,
                b"interface" => &mut rule.interface,
                b"member" => &mut rule.member,
                b"path" => &mut rule.path,
                b"path_namespace" => &mut rule.path_namespace,
                b"destination" => &mut rule.destination,
                other => {
                    let (index, test) = argument_key(other)?;
                    if rule.arguments.iter().any(|&(taken, ..)| taken == index) {
                        return None;
                    }
                    rule.arguments.push((index, test, value));
                    continue;
                }
            };
            if slot.is_some() {
                return None;
            }
            *slot = Some(value);
        }
        if rule.path.is_some() && rule.path_namespace.is_some() {
            return None;
        }
        rule.arguments.sort_by_key(|&(index, ..)| index);
        Some(rule)
    }

    /// Whether `message`, sent by a connection named by `is_sender`, matches.
    pub fn matches(&self, message: &Message<'_>, is_sender: impl Fn(&[u8]) -> bool) -> bool {
        let fields = &message.fields;
        let same = |wanted: &Option<Vec<u8>>, have: Option<&[u8]>| match wanted {
            None => true,
            Some(wanted) => have == Some(wanted.as_slice()),
        };
        if self.kind.is_some_and(|wanted| wanted != message.kind)
            || !same(&self.interface, fields.interface)
            || !same(&self.member, fields.member)
            || !same(&self.path, fields.path)
            || !same(&self.destination, fields.destination)
        {
            return false;
        }
        if let Some(sender) = &self.sender
            && !is_sender(sender)
        {
            return false;
        }
        if let Some(namespace) = &self.path_namespace {
            let Some(path) = fields.path else {
                return false;
            };
            let inside = namespace.as_slice() == b"/"
                || path == namespace.as_slice()
                || (path.starts_with(namespace) && path.get(namespace.len()) == Some(&b'/'));
            if !inside {
                return false;
            }
        }
        self.arguments.iter().all(|(index, test, wanted)| {
            let Some((code, value)) = message.string_argument(usize::from(*index)) else {
                return false;
            };
            match test {
                ArgumentTest::Equal => code == b's' && value == wanted.as_slice(),
                ArgumentTest::Path => {
                    value == wanted.as_slice()
                        || (wanted.ends_with(b"/") && value.starts_with(wanted))
                        || (value.ends_with(b"/") && wanted.starts_with(value))
                }
                ArgumentTest::Namespace => {
                    code == b's'
                        && (value == wanted.as_slice()
                            || (value.starts_with(wanted) && value.get(wanted.len()) == Some(&b'.')))
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{Fields, build};
    use crate::wire::{Endian, Writer};

    fn name_owner_changed(name: &[u8]) -> Vec<u8> {
        let mut body = Writer::new(Endian::Little);
        body.string(name);
        body.string(b"");
        body.string(b":1.3");
        let fields = Fields {
            path: Some(b"/org/freedesktop/DBus"),
            interface: Some(b"org.freedesktop.DBus"),
            member: Some(b"NameOwnerChanged"),
            sender: Some(b"org.freedesktop.DBus"),
            signature: b"sss",
            ..Fields::default()
        };
        build(Endian::Little, kind::SIGNAL, 0, 5, &fields, body.bytes())
    }

    #[test]
    fn quoting_and_escaped_apostrophes_parse() {
        let rule = Rule::parse(br"type='signal',member='it'\''s',arg2=a\'b").unwrap();
        assert_eq!(rule.kind, Some(kind::SIGNAL));
        assert_eq!(rule.member.as_deref(), Some(&b"it's"[..]));
        assert_eq!(rule.arguments, [(2, ArgumentTest::Equal, b"a'b".to_vec())]);
        assert!(Rule::parse(b"type='nothing'").is_none());
        assert!(Rule::parse(b"member='a',member='b'").is_none());
        assert!(Rule::parse(b"arg64='x'").is_none());
        assert!(Rule::parse(b"member='open").is_none());
    }

    #[test]
    fn a_rule_matches_its_signal_by_fields_sender_and_arguments() {
        let bytes = name_owner_changed(b"org.example.Editor");
        let message = Message::parse(&bytes).unwrap();
        let bus = |name: &[u8]| name == b"org.freedesktop.DBus";
        let rule = Rule::parse(
            b"type='signal',sender='org.freedesktop.DBus',member='NameOwnerChanged',arg0namespace='org.example'",
        )
        .unwrap();
        assert!(rule.matches(&message, bus));
        let other = Rule::parse(b"arg0='org.example'").unwrap();
        assert!(!other.matches(&message, bus));
        let path = Rule::parse(b"path_namespace='/org/freedesktop'").unwrap();
        assert!(path.matches(&message, bus));
        let narrower = Rule::parse(b"path_namespace='/org/free'").unwrap();
        assert!(!narrower.matches(&message, bus));
    }
}
