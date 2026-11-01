//! Pairing an agent that connects over the network (D193). The person starts it (`agent
//! pair`, or Settings' Agents pane): that adds the agent with no grants and its token's hash to
//! the grants store, and leaves a one-time code, its hash and the token in [`PAIRING_PATH`],
//! which only root reads. The agent sends the code once over TLS, after checking EggOS's
//! certificate by its fingerprint, and gets the token back; from then on it sends the token
//! with every request. The network server only reads the store, and changes nothing but this
//! file: it counts wrong codes and removes the file once the token is handed out.

use crate::grant_files::{FOLDER_MODE, GrantFiles, STORE_FOLDERS};
use crate::grants::{
    AgentId, GrantStore, StoreState, TOKEN_HASH_BYTES, decimal, digest_from_hex, fnv1a, hex_digest,
    hex_value, seconds,
};
use core::ffi::CStr;
use eggos_crypto::constant_time;
use eggos_crypto::sha2::Sha256;
use eggos_runtime::errno::Errno;

/// The file whose presence turns the network server on (`agent network on`); it may hold the
/// port, in decimal.
pub const NETWORK_ENABLED: &CStr = c"/var/lib/eggos/agent-network";
/// The file [`NETWORK_ENABLED`] is written to first.
pub const NETWORK_ENABLED_TEMPORARY: &CStr = c"/var/lib/eggos/agent-network.new";
/// The network server's private key: 32 bytes, readable by root only.
pub const NETWORK_KEY: &CStr = c"/var/lib/eggos/agent-network.key";
/// The network server's certificate, DER.
pub const NETWORK_CERTIFICATE: &CStr = c"/var/lib/eggos/agent-network.der";
/// The network server's certificate, PEM, for the person to give their agent's client.
pub const NETWORK_CERTIFICATE_PEM: &CStr = c"/var/lib/eggos/agent-network.pem";
/// The bytes of a certificate's fingerprint as people read it.
pub const FINGERPRINT_TEXT_BYTES: usize = 32 * 3 - 1;

/// Where the code waiting to be used is kept: in memory, gone at shutdown.
pub const PAIRING_PATH: &CStr = c"/run/eggos/agent-pairing";
/// The file a new pairing is written to, then renamed over [`PAIRING_PATH`].
pub const PAIRING_TEMPORARY_PATH: &CStr = c"/run/eggos/agent-pairing.new";
/// The folder the pairing file is in.
pub const PAIRING_FOLDER: &CStr = c"/run/eggos";
/// The most bytes the pairing file takes.
pub const PAIRING_BYTES: usize = 512;
/// The characters a code is made of: capital letters and digits, without `0`, `O`, `1`, `I`
/// and `L`, which people mistake for each other.
pub const CODE_ALPHABET: &[u8; 31] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";
/// The characters of a code: about 39.6 bits.
pub const CODE_LENGTH: usize = 8;
/// The bytes of a code as shown: two groups of four joined by `-`.
pub const CODE_TEXT_BYTES: usize = CODE_LENGTH + 1;
/// How long a code can be used, in seconds.
pub const CODE_LIFETIME_S: u64 = 300;
/// How many wrong codes burn the waiting one.
pub const MAX_WRONG_CODES: u8 = 5;
/// How long the server waits after a wrong code before it answers anything else.
pub const WRONG_CODE_PAUSE_MS: u64 = 1000;
/// How long after a burned code no new one can be made, in seconds.
pub const BURNED_WAIT_S: u64 = 30;
/// What every token starts with, so people and secret scanners recognise one.
pub const TOKEN_PREFIX: &[u8] = b"eggos_";
/// The bytes of randomness in a token.
pub const TOKEN_RANDOM_BYTES: usize = 32;
/// The bytes of a token: the prefix and 32 random bytes in base64url without padding.
pub const TOKEN_TEXT_BYTES: usize = 6 + 43;

const MAGIC: &[u8] = b"eggos-agent-pairing 1";
const SUM_PREFIX: &[u8] = b"sum ";
const SUM_DIGITS: usize = 16;
const FIELDS: usize = 6;
const BASE64URL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// A one-time code.
pub type Code = [u8; CODE_LENGTH];
/// A token, as the agent sends it.
pub type Token = [u8; TOKEN_TEXT_BYTES];

/// A code from fresh randomness, or `None` when too many of its bytes fell outside the
/// alphabet (about once in 10^12): draw again.
pub fn code_from(randomness: &[u8; 32]) -> Option<Code> {
    let mut code = [0u8; CODE_LENGTH];
    let mut length = 0;
    for &byte in randomness {
        let index = usize::from(byte & 31);
        if let (Some(&symbol), Some(slot)) = (CODE_ALPHABET.get(index), code.get_mut(length)) {
            *slot = symbol;
            length += 1;
        }
    }
    (length == CODE_LENGTH).then_some(code)
}

/// `code` as shown to the person: `ABCD-EFGH`.
pub fn code_text(code: &Code) -> [u8; CODE_TEXT_BYTES] {
    let mut text = [b'-'; CODE_TEXT_BYTES];
    text[..4].copy_from_slice(&code[..4]);
    text[5..].copy_from_slice(&code[4..]);
    text
}

/// The code a person typed, forgiving case, spaces and dashes; `None` when it is not one.
pub fn typed_code(typed: &[u8]) -> Option<Code> {
    let mut code = [0u8; CODE_LENGTH];
    let mut length = 0;
    for &byte in typed {
        if matches!(byte, b'-' | b' ') {
            continue;
        }
        let symbol = byte.to_ascii_uppercase();
        if !CODE_ALPHABET.contains(&symbol) {
            return None;
        }
        *code.get_mut(length)? = symbol;
        length += 1;
    }
    (length == CODE_LENGTH).then_some(code)
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::default();
    hasher.update(bytes);
    hasher.finish()
}

/// A token from 32 bytes of fresh randomness.
pub fn token_from(randomness: &[u8; TOKEN_RANDOM_BYTES]) -> Token {
    let mut token = [0u8; TOKEN_TEXT_BYTES];
    token[..TOKEN_PREFIX.len()].copy_from_slice(TOKEN_PREFIX);
    let mut at = TOKEN_PREFIX.len();
    for chunk in randomness.chunks(3) {
        let triple = chunk
            .iter()
            .enumerate()
            .fold(0u32, |value, (index, &byte)| {
                value | (u32::from(byte) << (16 - 8 * index))
            });
        for index in 0..=chunk.len() {
            token[at] = BASE64URL[((triple >> (18 - 6 * index)) & 63) as usize];
            at += 1;
        }
    }
    token
}

/// The hash the grants store keeps of a token.
pub fn token_hash(token: &[u8]) -> [u8; TOKEN_HASH_BYTES] {
    sha256(token)
}

/// A certificate's SHA-256 fingerprint as people compare it: upper-case hex pairs joined by
/// colons, as `openssl x509 -fingerprint -sha256` prints it.
pub fn fingerprint_text(der: &[u8]) -> [u8; FINGERPRINT_TEXT_BYTES] {
    const DIGITS: &[u8; 16] = b"0123456789ABCDEF";
    let mut text = [b':'; FINGERPRINT_TEXT_BYTES];
    for (index, byte) in sha256(der).iter().enumerate() {
        text[3 * index] = DIGITS[usize::from(byte >> 4)];
        text[3 * index + 1] = DIGITS[usize::from(byte & 15)];
    }
    text
}

/// What came of a code an agent sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Attempt {
    /// The code is right: the agent gets this token, and the file goes.
    Paired(AgentId, Token),
    /// The code is wrong; this many tries are left.
    Wrong(u8),
    /// The code is wrong and was the last try: the waiting code is burned.
    Burned,
    /// No code can be used now: it expired, was burned, or none is waiting.
    Closed,
}

/// Why a pairing file cannot be read or written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairingError {
    /// Not a pairing file of this version.
    Version,
    /// The checksum does not match.
    Checksum,
    /// A field is malformed.
    Malformed,
    /// The buffer given for writing is too small.
    Overflow,
}

/// The code waiting to be used, as kept in [`PAIRING_PATH`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingPairing {
    agent: AgentId,
    code_hash: [u8; 32],
    token: Token,
    expires: u64,
    wrong: u8,
    burned_at: Option<u64>,
}

impl PendingPairing {
    /// A code for `agent`, waiting from `now`, that gives `token`.
    pub fn new(agent: AgentId, code: &Code, token: Token, now: u64) -> Self {
        Self {
            agent,
            code_hash: sha256(code),
            token,
            expires: now.saturating_add(CODE_LIFETIME_S),
            wrong: 0,
            burned_at: None,
        }
    }

    /// The agent being paired.
    pub fn agent(&self) -> &AgentId {
        &self.agent
    }

    /// When the code stops working, in seconds since 1970.
    pub fn expires(&self) -> u64 {
        self.expires
    }

    /// How many wrong codes were sent.
    pub fn wrong_codes(&self) -> u8 {
        self.wrong
    }

    /// Whether a code can be tried at `now`.
    pub fn is_open(&self, now: u64) -> bool {
        self.burned_at.is_none() && now < self.expires
    }

    /// How many seconds must pass from `now` before a new code may replace this one: only a
    /// burned code holds the next one back.
    pub fn wait_before_next(&self, now: u64) -> u64 {
        match self.burned_at {
            Some(at) => at.saturating_add(BURNED_WAIT_S).saturating_sub(now),
            None => 0,
        }
    }

    /// Tries the code an agent sent at `now`.
    pub fn attempt(&mut self, sent: &[u8], now: u64) -> Attempt {
        if !self.is_open(now) {
            return Attempt::Closed;
        }
        let matches = typed_code(sent)
            .is_some_and(|code| constant_time::equal(&sha256(&code), &self.code_hash));
        if matches {
            return Attempt::Paired(self.agent, self.token);
        }
        self.wrong = self.wrong.saturating_add(1);
        match MAX_WRONG_CODES.checked_sub(self.wrong) {
            Some(left) if left > 0 => Attempt::Wrong(left),
            _ => {
                self.burned_at = Some(now);
                Attempt::Burned
            }
        }
    }

    /// Writes the file into `out` ([`PAIRING_BYTES`] is enough): a header, one tab-separated
    /// line (agent id, the code's hash, the token, when it expires, wrong tries, when it was
    /// burned or `-`) and the FNV-1a checksum.
    pub fn encode(&self, out: &mut [u8]) -> Result<usize, PairingError> {
        let mut length = 0;
        let mut put = |bytes: &[u8]| -> Result<(), PairingError> {
            out.get_mut(length..length + bytes.len())
                .ok_or(PairingError::Overflow)?
                .copy_from_slice(bytes);
            length += bytes.len();
            Ok(())
        };
        let mut digits = [0u8; 20];
        put(MAGIC)?;
        put(b"\n")?;
        put(self.agent.as_bytes())?;
        put(b"\t")?;
        put(&hex_digest(&self.code_hash))?;
        put(b"\t")?;
        put(&self.token)?;
        put(b"\t")?;
        put(decimal(self.expires, &mut digits))?;
        put(b"\t")?;
        put(decimal(u64::from(self.wrong), &mut digits))?;
        put(b"\t")?;
        match self.burned_at {
            Some(at) => put(decimal(at, &mut digits))?,
            None => put(b"-")?,
        }
        put(b"\n")?;
        let sum = fnv1a(out.get(..length).unwrap_or_default());
        let mut hex = [0u8; SUM_DIGITS];
        for (index, digit) in hex.iter_mut().enumerate() {
            *digit = b"0123456789abcdef"[(sum >> ((SUM_DIGITS - 1 - index) * 4)) as usize & 15];
        }
        let mut put = |bytes: &[u8]| -> Result<(), PairingError> {
            out.get_mut(length..length + bytes.len())
                .ok_or(PairingError::Overflow)?
                .copy_from_slice(bytes);
            length += bytes.len();
            Ok(())
        };
        put(SUM_PREFIX)?;
        put(&hex)?;
        put(b"\n")?;
        Ok(length)
    }

    /// Reads a pairing file; any doubt is an error.
    pub fn decode(file: &[u8]) -> Result<Self, PairingError> {
        let body = file.strip_suffix(b"\n").ok_or(PairingError::Checksum)?;
        let sum_start = body
            .iter()
            .rposition(|&byte| byte == b'\n')
            .map_or(0, |newline| newline + 1);
        let content = file.get(..sum_start).ok_or(PairingError::Checksum)?;
        let written = body
            .get(sum_start..)
            .and_then(|line| line.strip_prefix(SUM_PREFIX))
            .and_then(hex_value)
            .ok_or(PairingError::Checksum)?;
        if written != fnv1a(content) {
            return Err(PairingError::Checksum);
        }
        let rest = content
            .strip_prefix(MAGIC)
            .and_then(|rest| rest.strip_prefix(b"\n"))
            .ok_or(PairingError::Version)?;
        let line = rest.strip_suffix(b"\n").ok_or(PairingError::Malformed)?;
        let mut fields = [&b""[..]; FIELDS];
        let mut count = 0;
        for field in line.split(|&byte| byte == b'\t') {
            *fields.get_mut(count).ok_or(PairingError::Malformed)? = field;
            count += 1;
        }
        let [agent, code_hash, token, expires, wrong, burned_at] = fields;
        let malformed = PairingError::Malformed;
        if count != FIELDS || token.len() != TOKEN_TEXT_BYTES || !token.starts_with(TOKEN_PREFIX) {
            return Err(malformed);
        }
        let mut kept_token = [0u8; TOKEN_TEXT_BYTES];
        kept_token.copy_from_slice(token);
        Ok(Self {
            agent: AgentId::new(agent).ok_or(malformed)?,
            code_hash: digest_from_hex(code_hash).ok_or(malformed)?,
            token: kept_token,
            expires: seconds(expires).ok_or(malformed)?,
            wrong: seconds(wrong)
                .and_then(|wrong| u8::try_from(wrong).ok())
                .filter(|&wrong| wrong <= MAX_WRONG_CODES)
                .ok_or(malformed)?,
            burned_at: match burned_at {
                b"-" => None,
                at => Some(seconds(at).ok_or(malformed)?),
            },
        })
    }
}

#[cfg(test)]
mod tests;

/// What else pairing and the network switch need beyond [`GrantFiles`]: removing a file and
/// fresh randomness.
pub trait PairingFiles: GrantFiles {
    /// Removes the file at `path`; `ENOENT` when there is none.
    fn remove(&mut self, path: &CStr) -> Result<(), Errno>;
    /// Fills `out` with fresh randomness from the system's generator.
    fn random(&mut self, out: &mut [u8]) -> Result<(), Errno>;
}

/// The port agents connect to unless the network file says another.
pub const DEFAULT_PORT: u16 = 8443;
/// The most bytes of a certificate kept for its fingerprint.
pub const MAX_CERTIFICATE_BYTES: usize = 1536;
/// The pairing file's mode: only root reads the token in it.
pub const PAIRING_MODE: u32 = 0o600;
/// The network file's mode.
pub const NETWORK_MODE: u32 = 0o644;
/// The pairing file's folder's mode.
pub const PAIRING_FOLDER_MODE: u32 = 0o700;

/// Whether agents on the network are answered from EggOS's next start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkState {
    /// They are not.
    Off,
    /// They are, on this port.
    On(u16),
}

/// A port as `agent network --port` and the network file write it, `None` when it is not one.
pub fn parse_port(digits: &[u8]) -> Option<u16> {
    let digits = digits.strip_suffix(b"\n").unwrap_or(digits);
    if digits.is_empty() || digits.len() > 5 || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let value = digits
        .iter()
        .fold(0u32, |value, &digit| value * 10 + u32::from(digit - b'0'));
    u16::try_from(value).ok().filter(|&port| port > 0)
}

/// Whether the network file is there, and its port ([`DEFAULT_PORT`] when it holds none).
pub fn network_state(files: &mut impl GrantFiles) -> NetworkState {
    let mut text = [0u8; 16];
    match files.read_whole(NETWORK_ENABLED, &mut text) {
        Ok(length) => NetworkState::On(
            text.get(..length)
                .and_then(parse_port)
                .unwrap_or(DEFAULT_PORT),
        ),
        Err(_) => NetworkState::Off,
    }
}

/// Turns the network server on with `port`, or off with `None`, from EggOS's next start.
pub fn turn_network(
    files: &mut impl PairingFiles,
    port: Option<u16>,
) -> Result<(), (&'static CStr, Errno)> {
    let Some(port) = port else {
        return match files.remove(NETWORK_ENABLED) {
            Ok(()) | Err(Errno::ENOENT) => Ok(()),
            Err(errno) => Err((NETWORK_ENABLED, errno)),
        };
    };
    for folder in STORE_FOLDERS {
        match files.make_directory(folder, FOLDER_MODE) {
            Ok(()) | Err(Errno::EEXIST) => {}
            Err(errno) => return Err((folder, errno)),
        }
    }
    let mut digits = [0u8; 20];
    let text = decimal(u64::from(port), &mut digits);
    let mut line = [0u8; 8];
    let length = text.len() + 1;
    line[..text.len()].copy_from_slice(text);
    line[text.len()] = b'\n';
    write_replacing(
        files,
        NETWORK_ENABLED,
        NETWORK_ENABLED_TEMPORARY,
        NETWORK_MODE,
        &line[..length],
    )
}

fn write_replacing(
    files: &mut impl GrantFiles,
    target: &'static CStr,
    temporary: &'static CStr,
    mode: u32,
    bytes: &[u8],
) -> Result<(), (&'static CStr, Errno)> {
    files
        .write_new(temporary, mode, bytes)
        .map_err(|errno| (temporary, errno))?;
    files.sync();
    files
        .rename(temporary, target)
        .map_err(|errno| (target, errno))
}

/// The network server's certificate's fingerprint, when the server has made one.
pub fn certificate_fingerprint(
    files: &mut impl GrantFiles,
) -> Option<[u8; FINGERPRINT_TEXT_BYTES]> {
    let mut der = [0u8; MAX_CERTIFICATE_BYTES];
    match files.read_whole(NETWORK_CERTIFICATE, &mut der) {
        Ok(length) if length > 0 => Some(fingerprint_text(der.get(..length)?)),
        _ => None,
    }
}

/// The pairing waiting in [`PAIRING_PATH`], if one is there and reads back.
pub fn pending(files: &mut impl GrantFiles) -> Option<PendingPairing> {
    let mut file = [0u8; PAIRING_BYTES];
    let length = files.read_whole(PAIRING_PATH, &mut file).ok()?;
    PendingPairing::decode(file.get(..length)?).ok()
}

/// Why a pairing could not begin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairError {
    /// The last code was burned by wrong tries: this many seconds must pass.
    Burned(u64),
    /// No randomness could be had.
    Randomness(Errno),
    /// The grants file cannot be trusted or cannot hold the agent.
    Store(crate::grants::StoreError),
    /// This file or folder could not be made or written.
    File(&'static CStr, Errno),
}

/// Pairs the agent `id`: adds it with no grants unless the store has it (then its grants stay,
/// and its name unless `name` gives one), gives it a new token in place of any old one, writes
/// the pairing file with a new code, then the grants file, at `now`; returns the code.
pub fn begin_pairing(
    files: &mut impl PairingFiles,
    id: AgentId,
    name: Option<crate::grants::AgentName>,
    now: u64,
) -> Result<Code, PairError> {
    if let Some(waiting) = pending(files) {
        let wait = waiting.wait_before_next(now);
        if wait > 0 {
            return Err(PairError::Burned(wait));
        }
    }
    let mut store = match crate::grant_files::load(files) {
        StoreState::Loaded(store) => store,
        StoreState::Missing => GrantStore::new(),
        StoreState::Unreadable(error) => return Err(PairError::Store(error)),
    };
    let code = loop {
        let mut randomness = [0u8; 32];
        files
            .random(&mut randomness)
            .map_err(PairError::Randomness)?;
        if let Some(code) = code_from(&randomness) {
            break code;
        }
    };
    let mut randomness = [0u8; TOKEN_RANDOM_BYTES];
    files
        .random(&mut randomness)
        .map_err(PairError::Randomness)?;
    let token = token_from(&randomness);
    let shown = match (name, store.get(&id)) {
        (Some(name), _) => name,
        (None, Some(entry)) => *entry.name(),
        (None, None) => crate::grants::AgentName::new(id.as_bytes())
            .ok_or(PairError::Store(crate::grants::StoreError::Malformed))?,
    };
    store
        .add(id, shown)
        .map_err(PairError::Store)?
        .set_token_hash(Some(token_hash(&token)));
    let mut file = [0u8; PAIRING_BYTES];
    let length = PendingPairing::new(id, &code, token, now)
        .encode(&mut file)
        .map_err(|_| PairError::File(PAIRING_PATH, Errno::ENOSPC))?;
    match files.make_directory(PAIRING_FOLDER, PAIRING_FOLDER_MODE) {
        Ok(()) | Err(Errno::EEXIST) => {}
        Err(errno) => return Err(PairError::File(PAIRING_FOLDER, errno)),
    }
    write_replacing(
        files,
        PAIRING_PATH,
        PAIRING_TEMPORARY_PATH,
        PAIRING_MODE,
        file.get(..length).unwrap_or_default(),
    )
    .map_err(|(path, errno)| PairError::File(path, errno))?;
    match crate::grant_files::save(files, &store) {
        Ok(()) => Ok(code),
        Err(error) => {
            let _ = files.remove(PAIRING_PATH);
            Err(match error {
                crate::grant_files::SaveError::Store(error) => PairError::Store(error),
                crate::grant_files::SaveError::File(path, errno) => PairError::File(path, errno),
            })
        }
    }
}
