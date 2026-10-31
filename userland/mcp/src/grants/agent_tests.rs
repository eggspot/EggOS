#![allow(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]

use super::*;

fn id(text: &str) -> AgentId {
    AgentId::stored(text.as_bytes()).unwrap()
}

fn name(text: &str) -> AgentName {
    AgentName::new(text.as_bytes()).unwrap()
}

fn loaded(store: GrantStore, boot: Grants) -> Policy {
    Policy {
        boot,
        store: StoreState::Loaded(store),
    }
}

fn store_with(agent: &str, grants: &[(Grant, Permission, Option<u64>)]) -> GrantStore {
    let mut store = GrantStore::new();
    let entry = store.add(id(agent), name(agent)).unwrap();
    for &(grant, permission, until) in grants {
        entry.set(grant, permission, until);
    }
    store
}

fn encoded(store: &GrantStore) -> Vec<u8> {
    let mut out = [0u8; STORE_BYTES];
    let length = store.encode(&mut out).unwrap();
    out[..length].to_vec()
}

fn signed(content: &str) -> Vec<u8> {
    let mut file = content.as_bytes().to_vec();
    file.extend_from_slice(format!("sum {:016x}\n", fnv1a(content.as_bytes())).as_bytes());
    file
}

#[test]
fn an_unknown_agent_gets_nothing_not_even_reading() {
    let stranger = id("claude-desktop-7f3a");
    for policy in [
        Policy::default(),
        Policy {
            boot: Grants::ALL,
            store: StoreState::Missing,
        },
        loaded(
            store_with("other", &[(Grant::Run, Permission::Allow, None)]),
            Grants::ALL,
        ),
    ] {
        assert_eq!(
            policy.check(&stranger, None, 0),
            Decision::Deny(Denial::UnknownAgent)
        );
        for grant in Grant::ALL {
            assert_eq!(
                policy.check(&stranger, Some(grant), 0),
                Decision::Deny(Denial::UnknownAgent)
            );
        }
        assert_eq!(policy.allowed(&stranger, 0), Grants::NONE);
    }
}

#[test]
fn a_known_agent_reads_and_has_exactly_the_grants_given_with_run_bringing_the_rest() {
    for given in [Grant::View, Grant::Write, Grant::Install] {
        let policy = loaded(
            store_with("agent", &[(given, Permission::Allow, None)]),
            Grants::NONE,
        );
        assert_eq!(policy.check(&id("agent"), None, 0), Decision::Allow);
        for grant in Grant::ALL {
            let expected = match grant == given {
                true => Decision::Allow,
                false => Decision::Deny(Denial::NotGiven),
            };
            assert_eq!(
                policy.check(&id("agent"), Some(grant), 0),
                expected,
                "{given:?}"
            );
        }
        assert_eq!(policy.allowed(&id("agent"), 0), Grants::NONE.with(given));
    }
    let run = loaded(
        store_with("agent", &[(Grant::Run, Permission::Allow, None)]),
        Grants::NONE,
    );
    for grant in Grant::ALL {
        assert_eq!(run.check(&id("agent"), Some(grant), 0), Decision::Allow);
    }
    assert_eq!(run.allowed(&id("agent"), 0), Grants::ALL);
}

#[test]
fn ask_asks_the_user_and_the_stronger_of_the_grant_and_run_wins() {
    let policy = loaded(
        store_with(
            "agent",
            &[
                (Grant::Write, Permission::Ask, None),
                (Grant::Run, Permission::Ask, None),
                (Grant::View, Permission::Allow, None),
            ],
        ),
        Grants::NONE,
    );
    let agent = id("agent");
    assert_eq!(
        policy.check(&agent, Some(Grant::Write), 0),
        Decision::AskUser
    );
    assert_eq!(
        policy.check(&agent, Some(Grant::Install), 0),
        Decision::AskUser
    );
    assert_eq!(policy.check(&agent, Some(Grant::Run), 0), Decision::AskUser);
    assert_eq!(policy.check(&agent, Some(Grant::View), 0), Decision::Allow);
    assert_eq!(policy.allowed(&agent, 0), Grants::NONE.with(Grant::View));
}

#[test]
fn a_revoked_grant_stops_the_next_check() {
    let agent = id("agent");
    let mut store = store_with("agent", &[(Grant::Write, Permission::Allow, None)]);
    let before = StoreState::from_read(Ok(encoded(&store).as_slice()));
    let policy = Policy {
        boot: Grants::NONE,
        store: before,
    };
    assert_eq!(policy.check(&agent, Some(Grant::Write), 5), Decision::Allow);
    store.get_mut(&agent).unwrap().revoke(Grant::Write);
    let after = Policy {
        boot: Grants::NONE,
        store: StoreState::from_read(Ok(encoded(&store).as_slice())),
    };
    assert_eq!(
        after.check(&agent, Some(Grant::Write), 6),
        Decision::Deny(Denial::NotGiven)
    );
    assert_eq!(after.check(&agent, None, 6), Decision::Allow);
    assert!(store.forget(&agent));
    assert!(!store.forget(&agent));
    let forgotten = loaded(store, Grants::NONE);
    assert_eq!(
        forgotten.check(&agent, None, 7),
        Decision::Deny(Denial::UnknownAgent)
    );
}

#[test]
fn a_time_limited_grant_ends_at_its_second() {
    let store = store_with(
        "agent",
        &[
            (Grant::Write, Permission::Allow, Some(100)),
            (Grant::View, Permission::Allow, None),
        ],
    );
    assert!(store.has_expiry());
    let policy = loaded(store, Grants::NONE);
    assert!(policy.needs_clock());
    let agent = id("agent");
    assert_eq!(
        policy.check(&agent, Some(Grant::Write), 99),
        Decision::Allow
    );
    assert_eq!(
        policy.check(&agent, Some(Grant::Write), 100),
        Decision::Deny(Denial::Expired)
    );
    assert_eq!(
        policy.check(&agent, Some(Grant::View), 1_000),
        Decision::Allow
    );
    let mut ran = store_with("agent", &[(Grant::Run, Permission::Allow, Some(10))]);
    assert_eq!(
        ran.get(&agent).unwrap().decide(Grant::Install, 10),
        Decision::Deny(Denial::NotGiven)
    );
    ran.get_mut(&agent)
        .unwrap()
        .set(Grant::Run, Permission::Deny, Some(50));
    assert_eq!(
        ran.get(&agent).unwrap().class(Grant::Run),
        ClassGrant::DENIED
    );
    assert!(!ran.has_expiry());
    assert!(!loaded(ran, Grants::NONE).needs_clock());
}

#[test]
fn the_local_agent_keeps_the_boot_grants_until_the_user_lowers_them() {
    let local = AgentId::LOCAL;
    assert!(local.is_local());
    assert_eq!(local.as_bytes(), b"local");
    let boot = Grants::NONE.with(Grant::Write);
    let missing = Policy {
        boot,
        store: StoreState::Missing,
    };
    assert_eq!(missing.check(&local, None, 0), Decision::Allow);
    assert_eq!(
        missing.check(&local, Some(Grant::Write), 0),
        Decision::Allow
    );
    assert_eq!(
        missing.check(&local, Some(Grant::Install), 0),
        Decision::Deny(Denial::NotStartedWith)
    );
    assert_eq!(missing.allowed(&local, 0), boot);
    let without_local = loaded(
        store_with("other", &[(Grant::Install, Permission::Allow, None)]),
        boot,
    );
    assert_eq!(without_local.allowed(&local, 0), boot);
    let raised = loaded(
        store_with("local", &[(Grant::Run, Permission::Allow, None)]),
        boot,
    );
    assert_eq!(raised.check(&local, Some(Grant::Write), 0), Decision::Allow);
    assert_eq!(
        raised.check(&local, Some(Grant::Run), 0),
        Decision::Deny(Denial::NotStartedWith)
    );
    assert_eq!(raised.allowed(&local, 0), boot);
    let lowered = loaded(
        store_with(
            "local",
            &[
                (Grant::View, Permission::Allow, None),
                (Grant::Install, Permission::Ask, None),
            ],
        ),
        Grants::ALL,
    );
    assert_eq!(
        lowered.check(&local, Some(Grant::Write), 0),
        Decision::Deny(Denial::NotGiven)
    );
    assert_eq!(
        lowered.check(&local, Some(Grant::Install), 0),
        Decision::AskUser
    );
    assert_eq!(lowered.check(&local, None, 0), Decision::Allow);
    assert_eq!(lowered.allowed(&local, 0), Grants::NONE.with(Grant::View));
}

#[test]
fn an_unreadable_store_leaves_the_local_agent_reading_and_others_nothing() {
    let policy = Policy {
        boot: Grants::ALL,
        store: StoreState::Unreadable(StoreError::Checksum),
    };
    let denied = Decision::Deny(Denial::StoreUnreadable(StoreError::Checksum));
    assert_eq!(policy.check(&AgentId::LOCAL, None, 0), Decision::Allow);
    for grant in Grant::ALL {
        assert_eq!(policy.check(&AgentId::LOCAL, Some(grant), 0), denied);
        assert_eq!(policy.check(&id("agent"), Some(grant), 0), denied);
    }
    assert_eq!(policy.check(&id("agent"), None, 0), denied);
    assert_eq!(policy.allowed(&AgentId::LOCAL, 0), Grants::NONE);
}

#[test]
fn the_store_reads_back_what_was_written() {
    let mut store = GrantStore::new();
    store
        .add(id("claude-code.1"), name("Claude Code on the Mac"))
        .unwrap()
        .set(Grant::Write, Permission::Allow, Some(1_767_225_600));
    let second = store.add(id("phone_agent"), name("Téléphone")).unwrap();
    second.set(Grant::View, Permission::Ask, None);
    second.set(Grant::Run, Permission::Allow, Some(u64::MAX));
    store.add(id("idle"), name("idle")).unwrap();
    let file = encoded(&store);
    let text = String::from_utf8(file.clone()).unwrap();
    assert!(text.starts_with("eggos-agent-grants 1\nclaude-code.1\tdeny\tallow@1767225600\t"));
    assert!(text.contains("phone_agent\task\tdeny\tdeny\tallow@18446744073709551615\tTéléphone\n"));
    assert!(text.contains("idle\tdeny\tdeny\tdeny\tdeny\tidle\n"));
    assert_eq!(GrantStore::decode(&file), Ok(store));
    assert_eq!(
        StoreState::from_read(Ok(&file[..])),
        StoreState::Loaded(store)
    );
    assert_eq!(
        GrantStore::decode(&encoded(&GrantStore::new())),
        Ok(GrantStore::new())
    );
    store
        .add(id("idle"), name("renamed"))
        .unwrap()
        .set(Grant::Install, Permission::Allow, None);
    let renamed = GrantStore::decode(&encoded(&store)).unwrap();
    assert_eq!(renamed.agents().count(), 3);
    assert_eq!(
        renamed.get(&id("idle")).unwrap().name().as_bytes(),
        b"renamed"
    );
    let mut small = [0u8; 16];
    assert_eq!(store.encode(&mut small), Err(StoreError::Overflow));
}

#[test]
fn a_cut_changed_or_foreign_file_is_refused() {
    let file = encoded(&store_with(
        "agent",
        &[(Grant::Write, Permission::Allow, None)],
    ));
    let mut changed = file.clone();
    let at = changed.iter().position(|&byte| byte == b'w').unwrap();
    changed[at] = b'r';
    assert_eq!(GrantStore::decode(&changed), Err(StoreError::Checksum));
    assert_eq!(
        GrantStore::decode(&file[..file.len() - 1]),
        Err(StoreError::Checksum)
    );
    assert_eq!(
        GrantStore::decode(&file[..file.len() / 2]),
        Err(StoreError::Checksum)
    );
    assert_eq!(GrantStore::decode(b""), Err(StoreError::Checksum));
    assert_eq!(
        GrantStore::decode(&signed("eggos-agent-grants 2\n")),
        Err(StoreError::Version)
    );
    assert_eq!(
        GrantStore::decode(b"anything else\nsum 0000000000000000\n"),
        Err(StoreError::Version)
    );
    for line in [
        "agent\tallow\tdeny\tdeny\tdeny\n",
        "agent\tallow\tdeny\tdeny\tdeny\tname\textra\n",
        "agent\tyes\tdeny\tdeny\tdeny\tname\n",
        "agent\tdeny@5\tdeny\tdeny\tdeny\tname\n",
        "agent\tallow@\tdeny\tdeny\tdeny\tname\n",
        "agent\tallow@99999999999999999999\tdeny\tdeny\tdeny\tname\n",
        "bad id\tallow\tdeny\tdeny\tdeny\tname\n",
        "agent\tallow\tdeny\tdeny\tdeny\t\n",
        "agent\tallow\tdeny\tdeny\tdeny\tname\nagent\tdeny\tdeny\tdeny\tdeny\tname\n",
    ] {
        let file = signed(&format!("eggos-agent-grants 1\n{line}"));
        assert_eq!(
            GrantStore::decode(&file),
            Err(StoreError::Malformed),
            "{line:?}"
        );
    }
    let many: String = (0..=MAX_AGENTS)
        .map(|index| format!("a{index}\tdeny\tdeny\tdeny\tdeny\tn\n"))
        .collect();
    assert_eq!(
        GrantStore::decode(&signed(&format!("eggos-agent-grants 1\n{many}"))),
        Err(StoreError::Full)
    );
    assert_eq!(
        StoreState::from_read(Ok(&[b'x'; STORE_BYTES][..])),
        StoreState::Unreadable(StoreError::TooLarge)
    );
    assert_eq!(
        StoreState::from_read(Ok(&changed[..])),
        StoreState::Unreadable(StoreError::Checksum)
    );
    assert_eq!(
        StoreState::from_read(Err(Errno::ENOENT)),
        StoreState::Missing
    );
    assert_eq!(
        StoreState::from_read(Err(Errno::EIO)),
        StoreState::Unreadable(StoreError::Unreadable)
    );
}

#[test]
fn the_store_holds_at_most_its_agents() {
    let mut store = GrantStore::new();
    for index in 0..MAX_AGENTS {
        store.add(id(&format!("agent{index}")), name("n")).unwrap();
    }
    assert_eq!(
        store.add(id("one-more"), name("n")).map(|_| ()),
        Err(StoreError::Full)
    );
    assert!(store.add(id("agent3"), name("again")).is_ok());
    assert!(store.forget(&id("agent3")));
    assert!(store.add(id("one-more"), name("n")).is_ok());
    let file = encoded(&store);
    assert!(file.len() < STORE_BYTES);
}

#[test]
fn ids_and_names_refuse_what_could_disguise_them() {
    assert!(AgentId::new(b"claude-code_2.1").is_some());
    for bad in [
        &b""[..],
        b"has space",
        b"tab\t",
        b"\xc3\xa9",
        &[b'a'; MAX_AGENT_ID_BYTES + 1],
    ] {
        assert_eq!(AgentId::new(bad), None, "{bad:?}");
    }
    assert!(AgentId::new(&[b'a'; MAX_AGENT_ID_BYTES]).is_some());
    assert_eq!(AgentId::new(LOCAL_AGENT_ID), None);
    assert_eq!(AgentId::stored(LOCAL_AGENT_ID), Some(AgentId::LOCAL));
    assert_eq!(AgentId::stored(b"phone"), AgentId::new(b"phone"));
    assert_eq!(AgentId::stored(b"bad id"), None);
    assert!(AgentName::new("Claude Code 2.1 (Mac)".as_bytes()).is_some());
    for bad in [
        &b""[..],
        b"tab\there",
        b"new\nline",
        "evil\u{202e}gnp".as_bytes(),
        b"\xff",
        &[b'n'; MAX_AGENT_NAME_BYTES + 1],
    ] {
        assert_eq!(AgentName::new(bad), None, "{bad:?}");
    }
}
