//! The MCP session's life cycle (the 2025-11-25 specification's "Lifecycle"): the client's
//! `initialize` names the protocol version it wants and itself; the server answers with that
//! version when it implements it, otherwise its newest. A server init started again begins
//! initialized ([`Session::restarted`]), because its client never repeats `initialize`. The
//! client's capabilities say whether it can ask its user a question (elicitation, D137).

/// The protocol versions this server implements, newest first.
pub const SUPPORTED_VERSIONS: [&str; 2] = ["2025-11-25", "2025-06-18"];

/// The longest client name kept, in bytes; a longer one is cut and ends with `...`.
pub const MAX_CLIENT_NAME_BYTES: usize = 128;
/// The longest client version kept, in bytes.
pub const MAX_CLIENT_VERSION_BYTES: usize = 64;

/// The client's name while none was given.
pub const UNNAMED_CLIENT: &str = "unnamed client";
/// The client's name for a server that started again.
pub const RESTARTED_CLIENT: &str = "unknown (restarted)";

/// The first protocol version with elicitation.
pub const FIRST_ELICITATION_VERSION: &str = "2025-06-18";

/// Whether a client whose `capabilities.elicitation` is `capability` can ask its user in a
/// form: an object that is empty (2025-06-18's form, and 2025-11-25's for form only) or names
/// `form`; one naming only `url` cannot.
pub fn elicits_forms(capability: Option<crate::json::Value<'_>>) -> bool {
    match capability {
        Some(crate::json::Value::Object(object)) => {
            object.members().next().is_none() || object.get("form").is_some()
        }
        _ => false,
    }
}

/// The version answered to a client asking for `requested`.
pub fn negotiate(requested: &str) -> &'static str {
    SUPPORTED_VERSIONS
        .iter()
        .find(|&&version| version == requested)
        .copied()
        .unwrap_or(SUPPORTED_VERSIONS[0])
}

/// Text kept in a fixed array, cut at a character boundary with `...` when too long.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bounded<const N: usize> {
    bytes: [u8; N],
    length: usize,
}

impl<const N: usize> Bounded<N> {
    /// `text`, or as much of it as fits followed by `...`.
    pub fn new(text: &str) -> Self {
        let mut bounded = Self {
            bytes: [0; N],
            length: 0,
        };
        if text.len() <= N {
            bounded.bytes[..text.len()].copy_from_slice(text.as_bytes());
            bounded.length = text.len();
            return bounded;
        }
        let mut cut = N.saturating_sub(3);
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        bounded.bytes[..cut].copy_from_slice(&text.as_bytes()[..cut]);
        bounded.bytes[cut..cut + 3].copy_from_slice(b"...");
        bounded.length = cut + 3;
        bounded
    }

    /// The text.
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.length]).unwrap_or("")
    }
}

/// Where the session is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// No `initialize` yet.
    Fresh,
    /// Initialized: tools may be listed and called.
    Initialized,
}

/// The session with the client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    state: State,
    version: &'static str,
    client_name: Bounded<MAX_CLIENT_NAME_BYTES>,
    client_version: Bounded<MAX_CLIENT_VERSION_BYTES>,
    elicitation: bool,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            state: State::Fresh,
            version: SUPPORTED_VERSIONS[0],
            client_name: Bounded::new(UNNAMED_CLIENT),
            client_version: Bounded::new(""),
            elicitation: false,
        }
    }
}

impl Session {
    /// The session of a server init started again: initialized at the newest version, its
    /// client unknown.
    pub fn restarted() -> Self {
        Self {
            state: State::Initialized,
            client_name: Bounded::new(RESTARTED_CLIENT),
            ..Self::default()
        }
    }

    /// Starts the session again for a client that asked for `requested`, named itself and
    /// said whether it can ask its user (`elicitation`, honoured from
    /// [`FIRST_ELICITATION_VERSION`] on); returns the version answered.
    pub fn initialize(
        &mut self,
        requested: &str,
        name: &str,
        version: &str,
        elicitation: bool,
    ) -> &'static str {
        self.state = State::Initialized;
        self.version = negotiate(requested);
        self.elicitation = elicitation && self.version >= FIRST_ELICITATION_VERSION;
        self.client_name = Bounded::new(if name.is_empty() {
            UNNAMED_CLIENT
        } else {
            name
        });
        self.client_version = Bounded::new(version);
        self.version
    }

    /// The client left: the next one starts with `initialize`.
    pub fn end(&mut self) {
        *self = Self::default();
    }

    /// Whether tools may be listed and called.
    pub fn is_initialized(&self) -> bool {
        self.state == State::Initialized
    }

    /// The protocol version in use.
    pub fn version(&self) -> &'static str {
        self.version
    }

    /// Whether the client can ask its user a question (`elicitation/create`).
    pub fn elicits(&self) -> bool {
        self.elicitation
    }

    /// A copy of the client's name, which outlives a borrow of the session.
    pub fn client(&self) -> Bounded<MAX_CLIENT_NAME_BYTES> {
        self.client_name
    }

    /// The client's name, as it gave it.
    pub fn client_name(&self) -> &str {
        self.client_name.as_str()
    }

    /// The client's version, as it gave it.
    pub fn client_version(&self) -> &str {
        self.client_version.as_str()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_client_s_version_is_answered_when_implemented_else_the_newest() {
        assert_eq!(negotiate("2025-11-25"), "2025-11-25");
        assert_eq!(negotiate("2025-06-18"), "2025-06-18");
        assert_eq!(negotiate("2025-03-26"), "2025-11-25");
        assert_eq!(negotiate("2099-01-01"), "2025-11-25");
        assert_eq!(negotiate(""), "2025-11-25");
    }

    #[test]
    fn a_session_starts_fresh_initializes_and_ends() {
        let mut session = Session::default();
        assert!(!session.is_initialized());
        assert_eq!(
            session.initialize("2025-06-18", "claude-code", "2.1", true),
            "2025-06-18"
        );
        assert!(session.is_initialized());
        assert!(session.elicits());
        assert_eq!(session.client_name(), "claude-code");
        assert_eq!(session.client_version(), "2.1");
        assert_eq!(session.initialize("x", "", "", false), "2025-11-25");
        assert_eq!(session.client_name(), UNNAMED_CLIENT);
        assert!(!session.elicits());
        session.end();
        assert_eq!(session, Session::default());
        let restarted = Session::restarted();
        assert!(restarted.is_initialized());
        assert_eq!(restarted.client_name(), RESTARTED_CLIENT);
        assert_eq!(restarted.version(), SUPPORTED_VERSIONS[0]);
        assert!(!restarted.elicits());
    }

    #[test]
    fn forms_are_asked_for_with_an_empty_or_form_capability() {
        let capability = |text: &str| {
            let parsed = crate::json::parse(text.as_bytes()).unwrap();
            elicits_forms(Some(parsed.value))
        };
        assert!(capability("{}"));
        assert!(capability(r#"{"form":{},"url":{}}"#));
        assert!(capability(r#"{"form":{}}"#));
        assert!(!capability(r#"{"url":{}}"#));
        assert!(!capability("true"));
        assert!(!elicits_forms(None));
    }

    #[test]
    fn long_names_are_cut_at_a_character_boundary() {
        let name = "é".repeat(100);
        let bounded = Bounded::<MAX_CLIENT_NAME_BYTES>::new(&name);
        assert!(bounded.as_str().len() <= MAX_CLIENT_NAME_BYTES);
        assert!(bounded.as_str().ends_with("..."));
        assert!(bounded.as_str().starts_with("éé"));
        assert_eq!(Bounded::<4>::new("abcd").as_str(), "abcd");
        assert_eq!(Bounded::<4>::new("abcde").as_str(), "a...");
    }
}
