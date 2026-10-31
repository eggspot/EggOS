use crate::options::wants_json;
use crate::output::{FAILURE, Output};
use crate::system::System;
use core::ffi::CStr;
use eggos_commands::{CommandDefinition, commands, errors};
use eggos_mcp::grants::{
    self, AgentGrants, AgentId, AgentName, Grant, GrantStore, Grants, Permission, STORE_BYTES,
    StoreError, StoreState,
};
use eggos_mcp::json::JsonWriter;
use eggos_runtime::errno::Errno;

const USAGE: &[u8] = b"usage: agent list [--json] | show [--json] <id> | grant [--ask] \
    [--for <duration>] [--name <name>] [--yes] [--dry-run] <id> <view|write|install|run> | \
    revoke <id> [<grant>] | forget <id>";
const USAGE_STATUS: i32 = 2;
const HEADER: &[u8] = b"AGENT  NAME  VIEW  WRITE  INSTALL  RUN\n";
const STORE: &CStr = c"/var/lib/eggos/agent-grants";
const STORE_TEMPORARY: &CStr = c"/var/lib/eggos/agent-grants.new";
const STORE_FOLDERS: [&CStr; 3] = [c"/var", c"/var/lib", c"/var/lib/eggos"];
const COMMAND_LINE: &CStr = c"/proc/cmdline";
const COMMAND_LINE_BYTES: usize = 1024;
const STORE_MODE: u32 = 0o600;
const FOLDER_MODE: u32 = 0o755;
const RECORD_BYTES: usize = 1024;
const SECONDS_PER: [(u8, u64); 4] = [(b's', 1), (b'm', 60), (b'h', 3600), (b'd', 86_400)];

pub fn agent<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    let rest = arguments.get(1..).unwrap_or_default();
    match arguments.first().map(|verb| verb.to_bytes()) {
        Some(b"list") => list(output, rest),
        Some(b"show") => show(output, rest),
        Some(b"grant") => grant(output, rest),
        Some(b"revoke") => revoke(output, rest),
        Some(b"forget") => forget(output, rest),
        _ => usage(output),
    }
}

fn usage<S: System>(output: &mut Output<S>) -> i32 {
    output.coded(&errors::USAGE, &[USAGE]);
    USAGE_STATUS
}

#[derive(Default)]
struct Words<'a> {
    operands: [&'a [u8]; 2],
    count: usize,
    json: bool,
    ask: bool,
    yes: bool,
    dry_run: bool,
    duration: Option<&'a [u8]>,
    name: Option<&'a [u8]>,
}

fn words<'a, S: System>(
    output: &mut Output<S>,
    definition: &CommandDefinition,
    arguments: &[&'a CStr],
    most: usize,
) -> Result<Words<'a>, i32> {
    if wants_json(definition, arguments) {
        output.use_json();
    }
    let mut words = Words::default();
    let mut options_end = false;
    let mut index = 0;
    while let Some(argument) = arguments.get(index) {
        let bytes = argument.to_bytes();
        index += 1;
        let takes = |long: &str| definition.long_option(long.as_bytes()).is_some();
        match bytes {
            b"--" if !options_end => options_end = true,
            b"--json" if !options_end && takes("json") => words.json = true,
            b"--ask" if !options_end && takes("ask") => words.ask = true,
            b"-y" | b"--yes" if !options_end && takes("yes") => words.yes = true,
            b"--dry-run" if !options_end && takes("dry-run") => words.dry_run = true,
            b"--for" | b"--name" if !options_end && takes(&bytes_text(bytes)[2..]) => {
                let Some(value) = arguments.get(index) else {
                    return Err(usage(output));
                };
                index += 1;
                match bytes {
                    b"--for" => words.duration = Some(value.to_bytes()),
                    _ => words.name = Some(value.to_bytes()),
                }
            }
            option if !options_end && option.starts_with(b"-") && option.len() > 1 => {
                output.invalid_option(option);
                return Err(usage(output));
            }
            operand => {
                if words.count >= most {
                    return Err(usage(output));
                }
                words.operands[words.count] = operand;
                words.count += 1;
            }
        }
    }
    Ok(words)
}

fn bytes_text(bytes: &[u8]) -> &str {
    core::str::from_utf8(bytes).unwrap_or("--")
}

fn agent_id<S: System>(output: &mut Output<S>, id: &[u8]) -> Result<AgentId, i32> {
    AgentId::stored(id).ok_or_else(|| {
        output.coded(
            &errors::USAGE,
            &[
                b"'",
                id,
                b"' is not an agent id: 1 to 32 letters, digits, ., _ or -",
            ],
        );
        USAGE_STATUS
    })
}

fn read_store<S: System>(output: &mut Output<S>) -> Result<GrantStore, i32> {
    let mut file = [0u8; STORE_BYTES];
    let length = match read_whole(output, STORE, &mut file) {
        Ok(length) => length,
        Err(Errno::ENOENT) => return Ok(GrantStore::new()),
        Err(errno) => {
            output.file_error(STORE, errno);
            return Err(FAILURE);
        }
    };
    match StoreState::from_read(Ok(&file[..length])) {
        StoreState::Loaded(store) => Ok(store),
        StoreState::Missing => Ok(GrantStore::new()),
        StoreState::Unreadable(error) => {
            output.coded(
                &errors::DAMAGED,
                &[
                    STORE.to_bytes(),
                    b" cannot be trusted: ",
                    error.reason().as_bytes(),
                    b"; remove it to start over (every agent loses its grants)",
                ],
            );
            Err(FAILURE)
        }
    }
}

fn read_whole<S: System>(
    output: &mut Output<S>,
    path: &CStr,
    buffer: &mut [u8],
) -> Result<usize, Errno> {
    let descriptor = output.system.open(path)?;
    let mut length = 0;
    let result = loop {
        let Some(free) = buffer.get_mut(length..).filter(|free| !free.is_empty()) else {
            break Ok(length);
        };
        match output.system.read(descriptor, free) {
            Ok(0) => break Ok(length),
            Ok(count) => length += count,
            Err(Errno::EINTR) => {}
            Err(errno) => break Err(errno),
        }
    };
    output.system.close(descriptor);
    result
}

fn boot_grants<S: System>(output: &mut Output<S>) -> Grants {
    let mut line = [0u8; COMMAND_LINE_BYTES];
    match read_whole(output, COMMAND_LINE, &mut line) {
        Ok(length) => grants::from_command_line(&line[..length]).0,
        Err(_) => Grants::NONE,
    }
}

fn write_store<S: System>(output: &mut Output<S>, store: &GrantStore) -> Result<(), i32> {
    let mut file = [0u8; STORE_BYTES];
    let length = match store.encode(&mut file) {
        Ok(length) => length,
        Err(error) => return Err(store_error(output, error)),
    };
    for folder in STORE_FOLDERS {
        match output.system.make_directory(folder, FOLDER_MODE) {
            Ok(()) | Err(Errno::EEXIST) => {}
            Err(errno) => {
                output.file_error(folder, errno);
                return Err(FAILURE);
            }
        }
    }
    let descriptor = match output.system.create(STORE_TEMPORARY, STORE_MODE, true) {
        Ok(descriptor) => descriptor,
        Err(errno) => {
            output.file_error(STORE_TEMPORARY, errno);
            return Err(FAILURE);
        }
    };
    let written = output.system.write(descriptor, &file[..length]);
    output.system.close(descriptor);
    if let Err(errno) = written {
        output.file_error(STORE_TEMPORARY, errno);
        return Err(FAILURE);
    }
    output.system.sync();
    if let Err(errno) = output.system.rename(STORE_TEMPORARY, STORE) {
        output.file_error(STORE, errno);
        return Err(FAILURE);
    }
    Ok(())
}

fn store_error<S: System>(output: &mut Output<S>, error: StoreError) -> i32 {
    let definition = match error {
        StoreError::Full => &errors::TOO_MANY_AGENTS,
        _ => &errors::FAILED,
    };
    output.coded(
        definition,
        &[
            b"the agents' grants file cannot hold this: ",
            error.reason().as_bytes(),
        ],
    );
    FAILURE
}

fn local_from_boot(boot: Grants) -> AgentGrants {
    let mut local = AgentGrants::new(AgentId::LOCAL, local_name());
    for grant in Grant::ALL.into_iter().filter(|&grant| boot.allows(grant)) {
        local.set(grant, Permission::Allow, None);
    }
    local
}

fn local_name() -> AgentName {
    AgentName::new(grants::LOCAL_AGENT_ID).unwrap_or_else(|| unreachable!())
}

fn new_entry(id: AgentId, name: AgentName) -> AgentGrants {
    let mut entry = AgentGrants::new(id, name);
    if id.is_local() {
        for grant in Grant::ALL {
            entry.set(grant, Permission::Allow, None);
        }
    }
    entry
}

fn entry_of(store: &GrantStore, id: &AgentId, boot: Grants) -> Option<AgentGrants> {
    match (store.get(id), id.is_local()) {
        (Some(entry), _) => Some(*entry),
        (None, true) => Some(local_from_boot(boot)),
        (None, false) => None,
    }
}

fn no_such_agent<S: System>(output: &mut Output<S>, id: &AgentId) -> i32 {
    output.coded(
        &errors::NO_SUCH_AGENT,
        &[b"agent ", id.as_bytes(), b": EggOS does not know this agent"],
    );
    FAILURE
}

fn list<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    let words = match words(output, &commands::AGENT_LIST, arguments, 0) {
        Ok(words) => words,
        Err(status) => return status,
    };
    let store = match read_store(output) {
        Ok(store) => store,
        Err(status) => return status,
    };
    let boot = boot_grants(output);
    if !words.json {
        output.write(HEADER);
    }
    if store.get(&AgentId::LOCAL).is_none() {
        write_agent(output, &local_from_boot(boot), words.json);
    }
    for entry in store.agents() {
        write_agent(output, entry, words.json);
    }
    if !words.json {
        let mut described = [0u8; 32];
        let length = boot.describe(&mut described);
        output.write(b"EggOS was started with: ");
        output.write(&described[..length]);
        output.write(b"\n");
    }
    0
}

fn show<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    let words = match words(output, &commands::AGENT_SHOW, arguments, 1) {
        Ok(words) if words.count == 1 => words,
        Ok(_) => return usage(output),
        Err(status) => return status,
    };
    let id = match agent_id(output, words.operands[0]) {
        Ok(id) => id,
        Err(status) => return status,
    };
    let store = match read_store(output) {
        Ok(store) => store,
        Err(status) => return status,
    };
    let boot = boot_grants(output);
    let Some(entry) = entry_of(&store, &id, boot) else {
        return no_such_agent(output, &id);
    };
    if !words.json {
        output.write(HEADER);
    }
    write_agent(output, &entry, words.json);
    0
}

fn write_agent<S: System>(output: &mut Output<S>, entry: &AgentGrants, json: bool) {
    let mut record = [0u8; RECORD_BYTES];
    let length = match json {
        true => agent_json(entry, &mut record),
        false => agent_row(entry, &mut record),
    };
    output.write(&record[..length]);
}

fn agent_json(entry: &AgentGrants, record: &mut [u8]) -> usize {
    let mut out = JsonWriter::new(record);
    out.begin_object();
    out.key("id");
    out.string_lossy(entry.id().as_bytes());
    out.key("name");
    out.string_lossy(entry.name().as_bytes());
    for (grant, ends_at) in [
        (Grant::View, "view_ends_at"),
        (Grant::Write, "write_ends_at"),
        (Grant::Install, "install_ends_at"),
        (Grant::Run, "run_ends_at"),
    ] {
        let class = entry.class(grant);
        out.key(grant.name());
        out.string(class.permission.word());
        out.key(ends_at);
        match class.until.and_then(time_stamp) {
            Some(stamp) => out.string_lossy(&stamp),
            None => out.null(),
        }
    }
    out.end_object();
    out.end_line();
    match out.overflowed() {
        true => 0,
        false => out.len(),
    }
}

fn time_stamp(seconds: u64) -> Option<[u8; 20]> {
    eggos_calendar::rfc3339(i64::try_from(seconds).ok()?)
}

fn agent_row(entry: &AgentGrants, record: &mut [u8]) -> usize {
    let mut row = Row { record, length: 0 };
    row.display(entry.id().as_bytes());
    row.put(b"  ");
    row.display(entry.name().as_bytes());
    for grant in Grant::ALL {
        let class = entry.class(grant);
        row.put(b"  ");
        row.put(class.permission.word().as_bytes());
        if let Some(stamp) = class.until.and_then(time_stamp) {
            row.put(b" until ");
            row.put(&stamp);
        }
    }
    row.put(b"\n");
    row.length
}

struct Row<'r> {
    record: &'r mut [u8],
    length: usize,
}

impl Row<'_> {
    fn put(&mut self, bytes: &[u8]) {
        if let Some(place) = self.record.get_mut(self.length..self.length + bytes.len()) {
            place.copy_from_slice(bytes);
            self.length += bytes.len();
        }
    }

    fn display(&mut self, field: &[u8]) {
        let start = self.length.min(self.record.len());
        self.length = start + eggos_mcp::audit::display_into(field, &mut self.record[start..]);
    }
}

fn duration_seconds(text: &[u8]) -> Option<u64> {
    let (&unit, digits) = text.split_last()?;
    let &(_, scale) = SECONDS_PER.iter().find(|&&(letter, _)| letter == unit)?;
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let count: u64 = core::str::from_utf8(digits).ok()?.parse().ok()?;
    count.checked_mul(scale).filter(|&seconds| seconds > 0)
}

fn grant<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    let words = match words(output, &commands::AGENT_GRANT, arguments, 2) {
        Ok(words) if words.count == 2 => words,
        Ok(_) => return usage(output),
        Err(status) => return status,
    };
    let id = match agent_id(output, words.operands[0]) {
        Ok(id) => id,
        Err(status) => return status,
    };
    let Some(given) = Grant::named(words.operands[1]) else {
        return usage(output);
    };
    let until = match words.duration.map(duration_seconds) {
        None => None,
        Some(Some(seconds)) => Some(output.system.now().saturating_add(seconds)),
        Some(None) => {
            output.coded(
                &errors::USAGE,
                &[b"--for takes a number and s, m, h or d: 30m, 2h, 7d"],
            );
            return USAGE_STATUS;
        }
    };
    let name = match words.name.map(AgentName::new) {
        None => None,
        Some(Some(name)) => Some(name),
        Some(None) => {
            output.coded(
                &errors::USAGE,
                &[b"--name takes text of at most 64 bytes with no hidden character"],
            );
            return USAGE_STATUS;
        }
    };
    if matches!(given, Grant::Run | Grant::Install) && !words.yes && !words.dry_run {
        let consequence: &[u8] = match given {
            Grant::Run => b" run gives it full control of EggOS",
            _ => b" install lets it add programs",
        };
        output.coded(
            &errors::NEEDS_CONFIRMATION,
            &[
                b"giving ",
                id.as_bytes(),
                consequence,
                b"; add --yes to give it",
            ],
        );
        return FAILURE;
    }
    let mut store = match read_store(output) {
        Ok(store) => store,
        Err(status) => return status,
    };
    let fallback_name = name
        .or_else(|| AgentName::new(id.as_bytes()))
        .unwrap_or_else(local_name);
    if store.get(&id).is_none() {
        match store.add(id, fallback_name) {
            Ok(entry) => *entry = new_entry(id, fallback_name),
            Err(error) => return store_error(output, error),
        }
    }
    let Some(entry) = store.get_mut(&id) else {
        return no_such_agent(output, &id);
    };
    if let Some(name) = name {
        let mut renamed = AgentGrants::new(id, name);
        for each in Grant::ALL {
            let class = entry.class(each);
            renamed.set(each, class.permission, class.until);
        }
        *entry = renamed;
    }
    let permission = match words.ask {
        true => Permission::Ask,
        false => Permission::Allow,
    };
    entry.set(given, permission, until);
    if !words.dry_run
        && let Err(status) = write_store(output, &store)
    {
        return status;
    }
    let verb: &[u8] = match words.dry_run {
        true => b"would give ",
        false => b"gave ",
    };
    output.write(verb);
    output.write(id.as_bytes());
    output.write(b" ");
    output.write(given.name().as_bytes());
    if words.ask {
        output.write(b", asking the user each time");
    }
    if let Some(stamp) = until.and_then(time_stamp) {
        output.write(b", until ");
        output.write(&stamp);
    }
    output.write(b", from its next call\n");
    if !boot_grants(output).allows(given) {
        output.write(b"note: EggOS was started without ");
        output.write(given.name().as_bytes());
        output.write(b", and no agent gets more than EggOS was started with: start it with ");
        output.write(b"--mcp-allow ");
        output.write(given.name().as_bytes());
        output.write(b"\n");
    }
    0
}

fn revoke<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    let words = match words(output, &commands::AGENT_REVOKE, arguments, 2) {
        Ok(words) if words.count >= 1 => words,
        Ok(_) => return usage(output),
        Err(status) => return status,
    };
    let id = match agent_id(output, words.operands[0]) {
        Ok(id) => id,
        Err(status) => return status,
    };
    let taken = match words.count {
        2 => match Grant::named(words.operands[1]) {
            Some(grant) => Some(grant),
            None => return usage(output),
        },
        _ => None,
    };
    let mut store = match read_store(output) {
        Ok(store) => store,
        Err(status) => return status,
    };
    let boot = boot_grants(output);
    if store.get(&id).is_none() {
        if !id.is_local() {
            return no_such_agent(output, &id);
        }
        match store.add(id, local_name()) {
            Ok(entry) => *entry = local_from_boot(boot),
            Err(error) => return store_error(output, error),
        }
    }
    let Some(entry) = store.get_mut(&id) else {
        return no_such_agent(output, &id);
    };
    match taken {
        Some(grant) => entry.revoke(grant),
        None => Grant::ALL.into_iter().for_each(|grant| entry.revoke(grant)),
    }
    let keeps_run = entry.class(Grant::Run).permission != Permission::Deny;
    if let Err(status) = write_store(output, &store) {
        return status;
    }
    match taken {
        Some(grant) => {
            output.write(b"took ");
            output.write(grant.name().as_bytes());
            output.write(b" back from ");
            output.write(id.as_bytes());
            output.write(b", from its next call\n");
            if keeps_run && grant != Grant::Run {
                output.write(b"note: ");
                output.write(id.as_bytes());
                output.write(b" keeps run, which brings ");
                output.write(grant.name().as_bytes());
                output.write(b"; agent revoke ");
                output.write(id.as_bytes());
                output.write(b" run takes it too\n");
            }
        }
        None => {
            output.write(b"took every grant back from ");
            output.write(id.as_bytes());
            output.write(b"; it keeps reading\n");
        }
    }
    0
}

fn forget<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    let words = match words(output, &commands::AGENT_FORGET, arguments, 1) {
        Ok(words) if words.count == 1 => words,
        Ok(_) => return usage(output),
        Err(status) => return status,
    };
    let id = match agent_id(output, words.operands[0]) {
        Ok(id) => id,
        Err(status) => return status,
    };
    let mut store = match read_store(output) {
        Ok(store) => store,
        Err(status) => return status,
    };
    if !store.forget(&id) {
        return no_such_agent(output, &id);
    }
    if let Err(status) = write_store(output, &store) {
        return status;
    }
    output.write(b"forgot ");
    output.write(id.as_bytes());
    let after: &[u8] = match id.is_local() {
        true => b"; it has the grants EggOS was started with again\n",
        false => b"; it gets nothing from its next call\n",
    };
    output.write(after);
    0
}

#[cfg(test)]
#[path = "agent/tests.rs"]
mod tests;
