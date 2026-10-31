use super::*;
use crate::fake::Fake;

fn run(fake: &mut Fake, arguments: &[&CStr]) -> i32 {
    let mut output = Output::new(fake, b"agent");
    let status = agent(&mut output, arguments);
    output.finish(status)
}

fn started_with(grants: &str) -> Fake {
    let line = format!("console=ttyS0 eggos.mcp_allow={grants}\n");
    Fake::default()
        .with_directory("/proc", 0)
        .with_file("/proc/cmdline", 0o444, line.as_bytes(), 0)
}

fn stored(fake: &Fake) -> GrantStore {
    GrantStore::decode(&fake.contents("/var/lib/eggos/agent-grants")).unwrap()
}

#[test]
fn the_paths_are_the_server_s() {
    assert_eq!(STORE.to_str().unwrap(), grants::STORE_PATH);
    assert_eq!(STORE_TEMPORARY.to_str().unwrap(), grants::STORE_TEMPORARY_PATH);
    assert_eq!(STORE_FOLDERS[2].to_str().unwrap(), grants::STORE_FOLDER);
    assert_eq!(COMMAND_LINE.to_str().unwrap(), grants::COMMAND_LINE_PATH);
}

#[test]
fn a_grant_is_written_where_the_server_reads_it_and_listed() {
    let mut fake = started_with("view,write");
    assert_eq!(run(&mut fake, &[c"grant", c"--name", c"My phone", c"phone", c"write"]), 0);
    assert_eq!(fake.output(), "gave phone write, from its next call\n");
    let store = stored(&fake);
    let phone = store.get(&AgentId::new(b"phone").unwrap()).unwrap();
    assert_eq!(phone.name().as_bytes(), b"My phone");
    assert_eq!(phone.class(Grant::Write).permission, Permission::Allow);
    assert_eq!(phone.class(Grant::View).permission, Permission::Deny);
    assert_eq!(run(&mut fake, &[c"list"]), 0);
    assert!(
        fake.output().ends_with(
            "AGENT  NAME  VIEW  WRITE  INSTALL  RUN\nlocal  local  allow  allow  deny  deny\n\
             phone  My phone  deny  allow  deny  deny\nEggOS was started with: read,view,write\n"
        ),
        "{}",
        fake.output()
    );
}

#[test]
fn run_and_install_need_yes_and_a_time_limit_ends_the_grant() {
    let mut fake = started_with("run");
    assert_eq!(run(&mut fake, &[c"grant", c"phone", c"run"]), 1);
    assert!(fake.errors().contains("full control"), "{}", fake.errors());
    fake.now = 1_000;
    assert_eq!(run(&mut fake, &[c"grant", c"-y", c"--for", c"2h", c"phone", c"run"]), 0);
    let store = stored(&fake);
    let run_grant = store
        .get(&AgentId::new(b"phone").unwrap())
        .unwrap()
        .class(Grant::Run);
    assert_eq!(run_grant.until, Some(1_000 + 7_200));
    for bad in [c"2", c"h", c"0m", c"2w"] {
        assert_eq!(run(&mut fake, &[c"grant", c"--for", bad, c"phone", c"view"]), 2);
    }
}

#[test]
fn revoking_from_local_keeps_its_other_boot_grants_and_says_what_run_still_brings() {
    let mut fake = started_with("write,run");
    assert_eq!(run(&mut fake, &[c"revoke", c"local", c"write"]), 0);
    assert!(fake.output().contains("local keeps run, which brings write"));
    let local = *stored(&fake).get(&AgentId::LOCAL).unwrap();
    assert_eq!(local.class(Grant::Write).permission, Permission::Deny);
    assert_eq!(local.class(Grant::Run).permission, Permission::Allow);
    assert_eq!(run(&mut fake, &[c"revoke", c"local"]), 0);
    let local = *stored(&fake).get(&AgentId::LOCAL).unwrap();
    for grant in Grant::ALL {
        assert_eq!(local.class(grant).permission, Permission::Deny);
    }
    assert_eq!(run(&mut fake, &[c"forget", c"local"]), 0);
    assert!(stored(&fake).get(&AgentId::LOCAL).is_none());
}

#[test]
fn unknown_agents_bad_ids_and_a_damaged_store_are_refused() {
    let mut fake = started_with("write");
    assert_eq!(run(&mut fake, &[c"revoke", c"phone"]), 1);
    assert!(fake.errors().contains("no_such_agent") || fake.errors().contains("does not know"));
    assert_eq!(run(&mut fake, &[c"show", c"bad id!"]), 2);
    assert_eq!(run(&mut fake, &[c"grant", c"phone", c"everything"]), 2);
    assert_eq!(run(&mut fake, &[c"frobnicate"]), 2);
    let mut damaged = started_with("write")
        .with_directory("/var", 0)
        .with_directory("/var/lib", 0)
        .with_directory("/var/lib/eggos", 0)
        .with_file("/var/lib/eggos/agent-grants", 0o600, b"eggos-agent-grants 1\n", 0);
    assert_eq!(run(&mut damaged, &[c"grant", c"phone", c"view"]), 1);
    assert!(damaged.errors().contains("cannot be trusted"), "{}", damaged.errors());
}

#[test]
fn json_writes_one_record_an_agent() {
    let mut fake = started_with("view");
    assert_eq!(run(&mut fake, &[c"show", c"--json", c"--", c"local"]), 0);
    assert_eq!(
        fake.output(),
        "{\"id\":\"local\",\"name\":\"local\",\"view\":\"allow\",\"view_ends_at\":null,\
         \"write\":\"deny\",\"write_ends_at\":null,\"install\":\"deny\",\
         \"install_ends_at\":null,\"run\":\"deny\",\"run_ends_at\":null}\n"
    );
}
