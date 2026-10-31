//! What the user lets agents do (D21, D137, D193). The boot's grants are set on the host when
//! EggOS starts (`run-eggos.sh --mcp --mcp-allow write,install`, which puts
//! `eggos.mcp_allow=write,install` on the kernel command line); the server reads them from
//! `/proc/cmdline` only, which nothing in the machine can change. Without it, agents only read.
//! `view` lets them see window contents (D143). `run` runs any program and operates any window,
//! which can do anything, so it implies `view`, `write` and `install`.
//!
//! Each agent also has grants of its own, kept in the [`GrantStore`] at [`STORE_PATH`], which
//! only the user changes and the server reads again before every call, so a revoked grant
//! stops the next call. [`Policy::check`] decides a call: an unknown agent gets nothing, the
//! local channel's agent ([`AgentId::LOCAL`]) never more than the boot's grants, and an
//! unreadable store leaves every agent reading at most.

#![deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]

use crate::audit::is_hidden;
use eggos_runtime::errno::Errno;

/// The kernel command line's parameter holding the grants.
pub const PARAMETER: &[u8] = b"eggos.mcp_allow=";

/// Where the server reads the kernel command line.
pub const COMMAND_LINE_PATH: &str = "/proc/cmdline";

/// The most bytes of unknown names kept for the audit log.
pub const MAX_UNKNOWN_BYTES: usize = 128;

/// One thing the user can let agents do beyond reading.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grant {
    /// See window contents and menus (`wm_capture`, `wm_menu_list`).
    View,
    /// Write and delete files below `/home/root` (`tee`, `trash`, `fetch`, `ln`).
    Write,
    /// Install, open and uninstall apps (`app_install`, `app_open`, `app_uninstall`), and open
    /// a text file in the Text Editor (`edit_open`).
    Install,
    /// Run any program (`run`), operate any window (`wm_click`, `wm_type`, `wm_press`,
    /// `wm_scroll`, `wm_menu_choose`) and set the clock (`clock_set`, D150): full control of
    /// the machine.
    Run,
}

impl Grant {
    /// Every grant, in the order the log names them.
    pub const ALL: [Grant; 4] = [Grant::View, Grant::Write, Grant::Install, Grant::Run];

    /// Its name on the command line and in messages.
    pub fn name(self) -> &'static str {
        match self {
            Grant::View => "view",
            Grant::Write => "write",
            Grant::Install => "install",
            Grant::Run => "run",
        }
    }

    fn bit(self) -> u8 {
        match self {
            Grant::Write => 1,
            Grant::Install => 2,
            Grant::Run => 4,
            Grant::View => 8,
        }
    }

    /// The grant called `name` (`view`, `write`, `install` or `run`).
    pub fn named(name: &[u8]) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|grant| grant.name().as_bytes() == name)
    }
}

/// The grants of this boot.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Grants(u8);

impl Grants {
    /// Reading only.
    pub const NONE: Grants = Grants(0);

    /// Every grant.
    pub const ALL: Grants = Grants(15);

    /// These grants and `grant`, with what it implies: `run` brings `view`, `write` and
    /// `install`.
    pub fn with(self, grant: Grant) -> Self {
        match grant {
            Grant::Run => Grants::ALL,
            grant => Grants(self.0 | grant.bit()),
        }
    }

    /// Whether `grant` is given.
    pub fn allows(self, grant: Grant) -> bool {
        self.0 & grant.bit() != 0
    }

    /// The grants as the log shows them: `read`, then each given grant, comma-separated.
    pub fn describe(self, out: &mut [u8]) -> usize {
        let mut length = 0;
        let mut put = |text: &[u8], length: &mut usize| {
            if let Some(place) = out.get_mut(*length..*length + text.len()) {
                place.copy_from_slice(text);
                *length += text.len();
            }
        };
        put(b"read", &mut length);
        for grant in Grant::ALL.into_iter().filter(|&grant| self.allows(grant)) {
            put(b",", &mut length);
            put(grant.name().as_bytes(), &mut length);
        }
        length
    }
}

/// Names on the command line that are not grants, comma-separated as given, for the audit log
/// (a typo such as `Write` grants nothing, and shows there).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unknown {
    bytes: [u8; MAX_UNKNOWN_BYTES],
    length: usize,
}

impl Default for Unknown {
    fn default() -> Self {
        Self {
            bytes: [0; MAX_UNKNOWN_BYTES],
            length: 0,
        }
    }
}

impl Unknown {
    /// The names, as given; at most [`MAX_UNKNOWN_BYTES`].
    pub fn as_bytes(&self) -> &[u8] {
        self.bytes.get(..self.length).unwrap_or_default()
    }

    fn push(&mut self, name: &[u8]) {
        let separator: &[u8] = if self.length == 0 { b"" } else { b"," };
        for piece in [separator, name] {
            let room = MAX_UNKNOWN_BYTES - self.length;
            let taken = piece.get(..piece.len().min(room)).unwrap_or_default();
            if let Some(place) = self.bytes.get_mut(self.length..self.length + taken.len()) {
                place.copy_from_slice(taken);
                self.length += taken.len();
            }
        }
    }
}

/// The grants the kernel command line `line` gives: the value of its first
/// `eggos.mcp_allow=` word, comma-separated names; empty names are skipped, unknown ones grant
/// nothing and are returned.
pub fn from_command_line(line: &[u8]) -> (Grants, Unknown) {
    let mut grants = Grants::NONE;
    let mut unknown = Unknown::default();
    let Some(value) = line
        .split(u8::is_ascii_whitespace)
        .find_map(|word| word.strip_prefix(PARAMETER))
    else {
        return (grants, unknown);
    };
    for name in value
        .split(|&byte| byte == b',')
        .filter(|name| !name.is_empty())
    {
        match Grant::named(name) {
            Some(grant) => grants = grants.with(grant),
            None => unknown.push(name),
        }
    }
    (grants, unknown)
}

/// Where the user's per-agent grants are kept: on the disk, outside every folder a tool can
/// write, so no agent can raise its own grants by writing a file.
pub const STORE_PATH: &str = "/var/lib/eggos/agent-grants";
/// The file a new store is written to, synced, then renamed over [`STORE_PATH`], so a reader
/// sees the old store or the new one, never a part.
pub const STORE_TEMPORARY_PATH: &str = "/var/lib/eggos/agent-grants.new";
/// The folder the store is in.
pub const STORE_FOLDER: &str = "/var/lib/eggos";
/// The most bytes a store file takes; a larger file is refused as unreadable.
pub const STORE_BYTES: usize = 4096;
/// The most agents the store keeps.
pub const MAX_AGENTS: usize = 16;
/// The most bytes of an agent's id.
pub const MAX_AGENT_ID_BYTES: usize = 32;
/// The most bytes of an agent's name.
pub const MAX_AGENT_NAME_BYTES: usize = 64;
/// The id of the agent on the local channel, `/dev/agent`.
pub const LOCAL_AGENT_ID: &[u8] = b"local";

const STORE_MAGIC: &[u8] = b"eggos-agent-grants ";
const STORE_VERSION: &[u8] = b"1";
const SUM_PREFIX: &[u8] = b"sum ";
const SUM_DIGITS: usize = 16;
const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
const STORE_FIELDS: usize = 6;

/// An agent's stable id: given when it is paired or connects, 1 to [`MAX_AGENT_ID_BYTES`]
/// ASCII letters, digits, `.`, `_` or `-`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentId {
    bytes: [u8; MAX_AGENT_ID_BYTES],
    length: usize,
}

impl AgentId {
    /// The agent on the local channel, `/dev/agent`.
    pub const LOCAL: AgentId = AgentId::from_static(LOCAL_AGENT_ID);

    #[allow(clippy::indexing_slicing)]
    const fn from_static(id: &[u8]) -> Self {
        let mut bytes = [0u8; MAX_AGENT_ID_BYTES];
        let mut length = 0;
        while length < id.len() && length < MAX_AGENT_ID_BYTES {
            bytes[length] = id[length];
            length += 1;
        }
        Self { bytes, length }
    }

    /// The id `id` of an agent that paired or connected, or `None` when it is empty, too long,
    /// has another character or is `local`, which only the local channel is.
    pub fn new(id: &[u8]) -> Option<Self> {
        if id.is_empty()
            || id == LOCAL_AGENT_ID
            || id.len() > MAX_AGENT_ID_BYTES
            || !id
                .iter()
                .all(|&byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        {
            return None;
        }
        let mut bytes = [0u8; MAX_AGENT_ID_BYTES];
        bytes.get_mut(..id.len())?.copy_from_slice(id);
        Some(Self {
            bytes,
            length: id.len(),
        })
    }

    /// The id `id` as the store and the user name agents: [`AgentId::new`]'s, or `local`.
    pub fn stored(id: &[u8]) -> Option<Self> {
        match id == LOCAL_AGENT_ID {
            true => Some(Self::LOCAL),
            false => Self::new(id),
        }
    }

    /// The id's bytes.
    pub fn as_bytes(&self) -> &[u8] {
        self.bytes.get(..self.length).unwrap_or_default()
    }

    /// Whether it is the local channel's agent.
    pub fn is_local(&self) -> bool {
        *self == Self::LOCAL
    }
}

/// An agent's name as the user sees it: UTF-8, at most [`MAX_AGENT_NAME_BYTES`] bytes, with no
/// tab, newline or other hidden character.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentName {
    bytes: [u8; MAX_AGENT_NAME_BYTES],
    length: usize,
}

impl AgentName {
    /// The name `name`, or `None` when it is empty, too long, not UTF-8 or has a hidden
    /// character.
    pub fn new(name: &[u8]) -> Option<Self> {
        let text = core::str::from_utf8(name).ok()?;
        if text.is_empty() || name.len() > MAX_AGENT_NAME_BYTES || text.chars().any(is_hidden) {
            return None;
        }
        let mut bytes = [0u8; MAX_AGENT_NAME_BYTES];
        bytes.get_mut(..name.len())?.copy_from_slice(name);
        Some(Self {
            bytes,
            length: name.len(),
        })
    }

    /// The name's bytes.
    pub fn as_bytes(&self) -> &[u8] {
        self.bytes.get(..self.length).unwrap_or_default()
    }
}

/// What the user set for one grant of one agent, weakest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Permission {
    /// Never.
    Deny,
    /// Each call asks the user first.
    Ask,
    /// Without asking.
    Allow,
}

impl Permission {
    /// Its word in the store and in messages.
    pub fn word(self) -> &'static str {
        match self {
            Permission::Deny => "deny",
            Permission::Ask => "ask",
            Permission::Allow => "allow",
        }
    }

    /// The permission its word names.
    pub fn from_word(word: &[u8]) -> Option<Self> {
        [Permission::Deny, Permission::Ask, Permission::Allow]
            .into_iter()
            .find(|permission| permission.word().as_bytes() == word)
    }
}

/// One grant of one agent: the permission and, optionally, the wall clock's second it ends at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClassGrant {
    /// What the user set.
    pub permission: Permission,
    /// When it ends, in the wall clock's seconds; `None` for never.
    pub until: Option<u64>,
}

impl ClassGrant {
    /// Not given.
    pub const DENIED: ClassGrant = ClassGrant {
        permission: Permission::Deny,
        until: None,
    };

    /// The permission at `now`; `None` when its time has passed.
    pub fn at(self, now: u64) -> Option<Permission> {
        match self.until {
            Some(until) if now >= until => None,
            _ => Some(self.permission),
        }
    }
}

/// What a call may do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Go ahead.
    Allow,
    /// Go ahead only after the user says yes to this call.
    AskUser,
    /// Refuse, for this reason.
    Deny(Denial),
}

/// Why a call is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Denial {
    /// The agent needs a grant EggOS was not started with, which bounds every agent.
    NotStartedWith,
    /// The user has not given the agent this grant, or took it back.
    NotGiven,
    /// The grant's time limit has passed.
    Expired,
    /// The store has no such agent: the default for an agent the user never added.
    UnknownAgent,
    /// The store cannot be read, so no agent gets more than reading.
    StoreUnreadable(StoreError),
}

/// Why the store cannot be read or changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreError {
    /// The file could not be read.
    Unreadable,
    /// The file is larger than [`STORE_BYTES`].
    TooLarge,
    /// The file is not a store, or of a version this server does not know.
    Version,
    /// The checksum does not match: the file was cut or changed by other hands.
    Checksum,
    /// A line is not an agent's grants, or an agent is there twice.
    Malformed,
    /// More than [`MAX_AGENTS`] agents.
    Full,
    /// The buffer given for writing is too small.
    Overflow,
}

impl StoreError {
    /// Why, in a few words, for messages and the audit log.
    pub fn reason(self) -> &'static str {
        match self {
            StoreError::Unreadable => "it could not be read",
            StoreError::TooLarge => "it is too large",
            StoreError::Version => "it is not a grants file this version knows",
            StoreError::Checksum => "its checksum does not match",
            StoreError::Malformed => "a line in it is malformed",
            StoreError::Full => "it holds too many agents",
            StoreError::Overflow => "it does not fit",
        }
    }
}

/// One agent's grants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentGrants {
    id: AgentId,
    name: AgentName,
    view: ClassGrant,
    write: ClassGrant,
    install: ClassGrant,
    run: ClassGrant,
}

impl AgentGrants {
    /// An agent with no grants: it may read, nothing more.
    pub fn new(id: AgentId, name: AgentName) -> Self {
        Self {
            id,
            name,
            view: ClassGrant::DENIED,
            write: ClassGrant::DENIED,
            install: ClassGrant::DENIED,
            run: ClassGrant::DENIED,
        }
    }

    /// The agent's id.
    pub fn id(&self) -> &AgentId {
        &self.id
    }

    /// The agent's name.
    pub fn name(&self) -> &AgentName {
        &self.name
    }

    /// What the user set for `grant`.
    pub fn class(&self, grant: Grant) -> ClassGrant {
        match grant {
            Grant::View => self.view,
            Grant::Write => self.write,
            Grant::Install => self.install,
            Grant::Run => self.run,
        }
    }

    /// Sets `grant` to `permission` until `until` (a denial never ends).
    pub fn set(&mut self, grant: Grant, permission: Permission, until: Option<u64>) {
        let until = match permission {
            Permission::Deny => None,
            _ => until,
        };
        let class = ClassGrant { permission, until };
        match grant {
            Grant::View => self.view = class,
            Grant::Write => self.write = class,
            Grant::Install => self.install = class,
            Grant::Run => self.run = class,
        }
    }

    /// Takes `grant` back.
    pub fn revoke(&mut self, grant: Grant) {
        self.set(grant, Permission::Deny, None);
    }

    /// Whether a call needing `grant` may go ahead at `now`; `run` brings every other grant
    /// with it, as on the command line.
    pub fn decide(&self, grant: Grant, now: u64) -> Decision {
        let own = self.class(grant).at(now);
        let run = self.class(Grant::Run).at(now);
        let best = own
            .unwrap_or(Permission::Deny)
            .max(run.unwrap_or(Permission::Deny));
        match best {
            Permission::Allow => Decision::Allow,
            Permission::Ask => Decision::AskUser,
            Permission::Deny if own.is_none() => Decision::Deny(Denial::Expired),
            Permission::Deny => Decision::Deny(Denial::NotGiven),
        }
    }

    fn has_expiry(&self) -> bool {
        Grant::ALL
            .into_iter()
            .any(|grant| self.class(grant).until.is_some())
    }
}

/// Every agent the user added, with its grants. Only the user's front-ends (Settings and the
/// `agent` command) change it; the MCP server only reads it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GrantStore {
    agents: [Option<AgentGrants>; MAX_AGENTS],
}

impl GrantStore {
    /// A store with no agents.
    pub fn new() -> Self {
        Self::default()
    }

    /// The agents, in the order they were added.
    pub fn agents(&self) -> impl Iterator<Item = &AgentGrants> {
        self.agents.iter().flatten()
    }

    /// The agent `id`, if the user added it.
    pub fn get(&self, id: &AgentId) -> Option<&AgentGrants> {
        self.agents().find(|agent| agent.id == *id)
    }

    /// The agent `id`, added with no grants under `name` if it is new, or renamed if not.
    pub fn add(&mut self, id: AgentId, name: AgentName) -> Result<&mut AgentGrants, StoreError> {
        let index = match self
            .agents
            .iter()
            .position(|agent| matches!(agent, Some(agent) if agent.id == id))
        {
            Some(index) => index,
            None => self
                .agents
                .iter()
                .position(Option::is_none)
                .ok_or(StoreError::Full)?,
        };
        let slot = self.agents.get_mut(index).ok_or(StoreError::Full)?;
        let agent = slot.get_or_insert(AgentGrants::new(id, name));
        agent.name = name;
        Ok(agent)
    }

    /// The agent `id`'s grants, to change.
    pub fn get_mut(&mut self, id: &AgentId) -> Option<&mut AgentGrants> {
        self.agents
            .iter_mut()
            .flatten()
            .find(|agent| agent.id == *id)
    }

    /// Removes the agent `id` and every grant it had; whether it was there.
    pub fn forget(&mut self, id: &AgentId) -> bool {
        match self
            .agents
            .iter_mut()
            .find(|agent| matches!(agent, Some(agent) if agent.id == *id))
        {
            Some(slot) => {
                *slot = None;
                true
            }
            None => false,
        }
    }

    /// Whether any grant has a time limit, so deciding needs the wall clock.
    pub fn has_expiry(&self) -> bool {
        self.agents().any(AgentGrants::has_expiry)
    }

    /// Writes the store as its file into `out` ([`STORE_BYTES`] is always enough): a header
    /// line, one tab-separated line per agent (id, `view`, `write`, `install`, `run`, name;
    /// each permission a word with `@` and the second it ends at when it ends) and a last line
    /// with the FNV-1a checksum of everything before it.
    pub fn encode(&self, out: &mut [u8]) -> Result<usize, StoreError> {
        let mut writer = Writer { out, length: 0 };
        writer.put(STORE_MAGIC)?;
        writer.put(STORE_VERSION)?;
        writer.put(b"\n")?;
        for agent in self.agents() {
            writer.put(agent.id.as_bytes())?;
            for grant in Grant::ALL {
                let class = agent.class(grant);
                writer.put(b"\t")?;
                writer.put(class.permission.word().as_bytes())?;
                if let Some(until) = class.until {
                    writer.put(b"@")?;
                    let mut digits = [0u8; 20];
                    writer.put(decimal(until, &mut digits))?;
                }
            }
            writer.put(b"\t")?;
            writer.put(agent.name.as_bytes())?;
            writer.put(b"\n")?;
        }
        let sum = fnv1a(writer.out.get(..writer.length).unwrap_or_default());
        writer.put(SUM_PREFIX)?;
        let mut hex = [0u8; SUM_DIGITS];
        for (index, digit) in hex.iter_mut().enumerate() {
            let shift = (SUM_DIGITS - 1 - index) * 4;
            *digit = b"0123456789abcdef"
                .get((sum >> shift) as usize & 15)
                .copied()
                .unwrap_or(b'0');
        }
        writer.put(&hex)?;
        writer.put(b"\n")?;
        Ok(writer.length)
    }

    /// Reads a store file; any doubt about it is an error, never a guess.
    pub fn decode(file: &[u8]) -> Result<Self, StoreError> {
        let body = file.strip_suffix(b"\n").ok_or(StoreError::Checksum)?;
        let sum_start = body
            .iter()
            .rposition(|&byte| byte == b'\n')
            .map_or(0, |newline| newline + 1);
        let content = file.get(..sum_start).ok_or(StoreError::Checksum)?;
        let header_end = content
            .iter()
            .position(|&byte| byte == b'\n')
            .ok_or(StoreError::Version)?;
        let header = content.get(..header_end).ok_or(StoreError::Version)?;
        if header.strip_prefix(STORE_MAGIC) != Some(STORE_VERSION) {
            return Err(StoreError::Version);
        }
        let sum_line = body.get(sum_start..).ok_or(StoreError::Checksum)?;
        let written = sum_line
            .strip_prefix(SUM_PREFIX)
            .and_then(hex_value)
            .ok_or(StoreError::Checksum)?;
        if written != fnv1a(content) {
            return Err(StoreError::Checksum);
        }
        let mut store = Self::new();
        let lines = content.get(header_end + 1..).unwrap_or_default();
        for line in lines
            .strip_suffix(b"\n")
            .unwrap_or(lines)
            .split(|&byte| byte == b'\n')
            .filter(|line| !line.is_empty())
        {
            let agent = agent_line(line).ok_or(StoreError::Malformed)?;
            if store.get(&agent.id).is_some() {
                return Err(StoreError::Malformed);
            }
            *store.add(agent.id, agent.name)? = agent;
        }
        Ok(store)
    }
}

fn agent_line(line: &[u8]) -> Option<AgentGrants> {
    let mut fields = [&b""[..]; STORE_FIELDS];
    let mut count = 0;
    for field in line.split(|&byte| byte == b'\t') {
        *fields.get_mut(count)? = field;
        count += 1;
    }
    let [id, view, write, install, run, name] = fields;
    if count != STORE_FIELDS {
        return None;
    }
    let mut agent = AgentGrants::new(AgentId::stored(id)?, AgentName::new(name)?);
    for (grant, field) in Grant::ALL.into_iter().zip([view, write, install, run]) {
        let (word, until) = match field.iter().position(|&byte| byte == b'@') {
            Some(at) => (field.get(..at)?, Some(seconds(field.get(at + 1..)?)?)),
            None => (field, None),
        };
        let permission = Permission::from_word(word)?;
        if permission == Permission::Deny && until.is_some() {
            return None;
        }
        agent.set(grant, permission, until);
    }
    Some(agent)
}

fn seconds(digits: &[u8]) -> Option<u64> {
    if digits.is_empty() || digits.len() > 20 {
        return None;
    }
    digits.iter().try_fold(0u64, |value, &digit| {
        let digit = u64::from(digit.checked_sub(b'0').filter(|&digit| digit <= 9)?);
        value.checked_mul(10)?.checked_add(digit)
    })
}

fn hex_value(digits: &[u8]) -> Option<u64> {
    if digits.len() != SUM_DIGITS {
        return None;
    }
    digits.iter().try_fold(0u64, |value, &digit| {
        let digit = match digit {
            b'0'..=b'9' => digit - b'0',
            b'a'..=b'f' => digit - b'a' + 10,
            _ => return None,
        };
        Some((value << 4) | u64::from(digit))
    })
}

fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(FNV_OFFSET, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(FNV_PRIME)
    })
}

fn decimal(mut value: u64, digits: &mut [u8; 20]) -> &[u8] {
    let mut start = digits.len();
    while start > 0 {
        start -= 1;
        if let Some(digit) = digits.get_mut(start) {
            *digit = b'0' + (value % 10) as u8;
        }
        value /= 10;
        if value == 0 {
            break;
        }
    }
    digits.get(start..).unwrap_or_default()
}

struct Writer<'o> {
    out: &'o mut [u8],
    length: usize,
}

impl Writer<'_> {
    fn put(&mut self, bytes: &[u8]) -> Result<(), StoreError> {
        let place = self
            .out
            .get_mut(self.length..self.length + bytes.len())
            .ok_or(StoreError::Overflow)?;
        place.copy_from_slice(bytes);
        self.length += bytes.len();
        Ok(())
    }
}

/// The store as the server last read it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[allow(clippy::large_enum_variant)]
pub enum StoreState {
    /// There is no store: the user never changed an agent's grants.
    #[default]
    Missing,
    /// The store.
    Loaded(GrantStore),
    /// The store is there but cannot be trusted.
    Unreadable(StoreError),
}

impl StoreState {
    /// The state a read of the store file gave: its bytes (a read that filled
    /// [`STORE_BYTES`] is taken as too large), or why it could not be read (`ENOENT`: there is
    /// no store).
    pub fn from_read(read: Result<&[u8], Errno>) -> Self {
        match read {
            Ok(file) if file.len() >= STORE_BYTES => StoreState::Unreadable(StoreError::TooLarge),
            Ok(file) => match GrantStore::decode(file) {
                Ok(store) => StoreState::Loaded(store),
                Err(error) => StoreState::Unreadable(error),
            },
            Err(Errno::ENOENT) => StoreState::Missing,
            Err(_) => StoreState::Unreadable(StoreError::Unreadable),
        }
    }
}

/// The boot's grants and the store together: what decides every call.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Policy {
    /// The grants EggOS was started with: all any agent on this server's channel can ever
    /// have, since an agent names itself there in its own words.
    pub boot: Grants,
    /// The store.
    pub store: StoreState,
}

impl Policy {
    /// Whether a call by `agent` needing `capability` (`None` for reading) may go ahead at
    /// `now` (the wall clock's seconds). An agent the store does not have gets nothing, the
    /// local channel's agent excepted: it has the boot's grants until the user sets its own.
    /// The boot's grants bound every agent's. An unreadable store leaves the local agent
    /// reading and every other agent nothing.
    pub fn check(&self, agent: &AgentId, capability: Option<Grant>, now: u64) -> Decision {
        let entry = match &self.store {
            StoreState::Unreadable(error) => {
                return match (agent.is_local(), capability) {
                    (true, None) => Decision::Allow,
                    _ => Decision::Deny(Denial::StoreUnreadable(*error)),
                };
            }
            StoreState::Missing => None,
            StoreState::Loaded(store) => store.get(agent),
        };
        if agent.is_local() {
            let Some(grant) = capability else {
                return Decision::Allow;
            };
            if !self.boot.allows(grant) {
                return Decision::Deny(Denial::NotStartedWith);
            }
            return match entry {
                Some(entry) => entry.decide(grant, now),
                None => Decision::Allow,
            };
        }
        match (entry, capability) {
            (None, _) => Decision::Deny(Denial::UnknownAgent),
            (Some(_), None) => Decision::Allow,
            (Some(_), Some(grant)) if !self.boot.allows(grant) => {
                Decision::Deny(Denial::NotStartedWith)
            }
            (Some(entry), Some(grant)) => entry.decide(grant, now),
        }
    }

    /// The grants `agent` has without asking at `now`, as the log and its instructions show
    /// them.
    pub fn allowed(&self, agent: &AgentId, now: u64) -> Grants {
        Grant::ALL
            .into_iter()
            .filter(|&grant| self.check(agent, Some(grant), now) == Decision::Allow)
            .fold(Grants::NONE, Grants::with)
    }

    /// Whether deciding needs the wall clock: some grant in the store has a time limit.
    pub fn needs_clock(&self) -> bool {
        matches!(&self.store, StoreState::Loaded(store) if store.has_expiry())
    }
}

#[cfg(test)]
mod agent_tests;

#[cfg(test)]
mod tests {
    #![allow(clippy::indexing_slicing, clippy::unwrap_used, clippy::panic)]

    use super::*;

    fn described(grants: Grants) -> String {
        let mut out = [0u8; 64];
        let length = grants.describe(&mut out);
        String::from_utf8(out[..length].to_vec()).unwrap()
    }

    #[test]
    fn no_parameter_means_reading_only() {
        for line in [
            &b""[..],
            b"console=ttyS0",
            b"eggos.mcp_allowx=run",
            b"xeggos.mcp_allow=run",
        ] {
            let (grants, unknown) = from_command_line(line);
            assert_eq!(grants, Grants::NONE, "{line:?}");
            assert_eq!(unknown.as_bytes(), b"");
        }
        assert_eq!(described(Grants::NONE), "read");
    }

    #[test]
    fn named_grants_are_given_and_run_implies_the_others() {
        let (grants, _) = from_command_line(b"quiet eggos.mcp_allow=write,install\n");
        assert!(grants.allows(Grant::Write) && grants.allows(Grant::Install));
        assert!(!grants.allows(Grant::Run));
        assert_eq!(described(grants), "read,write,install");
        let (grants, _) = from_command_line(b"eggos.mcp_allow=run");
        assert_eq!(grants, Grants::ALL);
        assert_eq!(described(grants), "read,view,write,install,run");
        let (grants, _) = from_command_line(b"eggos.mcp_allow=install");
        assert!(!grants.allows(Grant::Write));
    }

    #[test]
    fn view_is_its_own_grant_and_only_run_implies_it() {
        let (grants, _) = from_command_line(b"eggos.mcp_allow=view");
        assert!(grants.allows(Grant::View));
        assert!(Grant::ALL[1..].iter().all(|&grant| !grants.allows(grant)));
        assert_eq!(described(grants), "read,view");
        let (grants, _) = from_command_line(b"eggos.mcp_allow=write,install");
        assert!(!grants.allows(Grant::View));
        assert!(Grants::NONE.with(Grant::Run).allows(Grant::View));
        let (grants, _) = from_command_line(b"eggos.mcp_allow=view,write,run,install");
        assert_eq!(grants, Grants::ALL);
    }

    #[test]
    fn the_grants_are_found_beside_the_offscreen_display_in_either_order() {
        for line in [
            &b"eggos.display=offscreen eggos.mcp_allow=view,write,run,install"[..],
            b"eggos.mcp_allow=view,write,run,install eggos.display=offscreen",
            b"eggos.display=offscreen\teggos.mcp_allow=run\n",
        ] {
            let (grants, unknown) = from_command_line(line);
            assert_eq!(grants, Grants::ALL, "{line:?}");
            assert_eq!(unknown.as_bytes(), b"");
        }
        let (grants, _) = from_command_line(b"eggos.display=offscreen");
        assert_eq!(grants, Grants::NONE);
    }

    #[test]
    fn the_first_parameter_wins_and_odd_names_grant_nothing() {
        let (grants, unknown) = from_command_line(b"eggos.mcp_allow=write eggos.mcp_allow=run");
        assert_eq!(described(grants), "read,write");
        assert_eq!(unknown.as_bytes(), b"");
        let (grants, unknown) = from_command_line(b"eggos.mcp_allow=Write,,run2,install,");
        assert_eq!(described(grants), "read,install");
        assert_eq!(unknown.as_bytes(), b"Write,run2");
        let (grants, _) = from_command_line(b"eggos.mcp_allow=");
        assert_eq!(grants, Grants::NONE);
        let (grants, _) = from_command_line(b"eggos.mcp_allow=wr");
        assert_eq!(grants, Grants::NONE);
        let long = format!("eggos.mcp_allow={}", "x".repeat(300));
        let (_, unknown) = from_command_line(long.as_bytes());
        assert_eq!(unknown.as_bytes().len(), MAX_UNKNOWN_BYTES);
    }
}
