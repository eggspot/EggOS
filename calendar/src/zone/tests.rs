use super::*;

fn local(zone_name: &str, utc: i64) -> (i64, String, bool) {
    let rule = zone(zone_name).unwrap();
    let local = rule.at(utc);
    (
        local.offset,
        local.abbreviation.as_str().to_owned(),
        local.daylight,
    )
}

#[test]
fn every_zone_is_sorted_and_its_rule_reads() {
    for pair in ZONES.windows(2) {
        assert!(pair[0].0 < pair[1].0, "{} {}", pair[0].0, pair[1].0);
    }
    for (name, rule) in ZONES {
        assert!(parse(rule).is_some(), "{name}: {rule}");
        assert!(name.len() <= MAX_ZONE_NAME_BYTES, "{name}");
    }
    assert_eq!(find("Europe/Paris"), Some("CET-1CEST,M3.5.0,M10.5.0/3"));
    assert_eq!(find("Mars/Olympus"), None);
}

#[test]
fn berlin_changes_to_summer_time_on_the_last_sunday_of_march_at_one_utc() {
    assert_eq!(
        local("Europe/Berlin", 1_774_745_999),
        (3600, "CET".into(), false)
    );
    assert_eq!(
        local("Europe/Berlin", 1_774_746_000),
        (7200, "CEST".into(), true)
    );
    assert_eq!(
        local("Europe/Berlin", 1_792_889_999),
        (7200, "CEST".into(), true)
    );
    assert_eq!(
        local("Europe/Berlin", 1_792_890_000),
        (3600, "CET".into(), false)
    );
}

#[test]
fn new_york_and_sydney_and_nuuk_follow_their_rules() {
    assert_eq!(local("America/New_York", 1_772_953_199).0, -5 * 3600);
    assert_eq!(
        local("America/New_York", 1_772_953_200),
        (-4 * 3600, "EDT".into(), true)
    );
    assert_eq!(local("America/New_York", 1_793_512_799).0, -4 * 3600);
    assert_eq!(local("America/New_York", 1_793_512_800).0, -5 * 3600);
    assert_eq!(
        local(":Australia/Sydney", 1_775_318_399),
        (11 * 3600, "AEDT".into(), true)
    );
    assert_eq!(
        local("Australia/Sydney", 1_775_318_400),
        (10 * 3600, "AEST".into(), false)
    );
    assert_eq!(local("Australia/Sydney", 1_791_043_200).0, 11 * 3600);
    assert_eq!(local("Australia/Sydney", 1_782_907_200).0, 10 * 3600);
    assert_eq!(
        local("America/Nuuk", 1_774_745_999),
        (-2 * 3600, "-02".into(), false)
    );
    assert_eq!(
        local("America/Nuuk", 1_774_746_000),
        (-3600, "-01".into(), true)
    );
    assert_eq!(
        local("Asia/Kolkata", 1_782_907_200),
        (19_800, "IST".into(), false)
    );
}

#[test]
fn posix_rules_read_with_julian_days_and_the_default_rules() {
    let leap = parse("XXX3YYY,J60/0,J300/0").unwrap();
    assert!(!leap.at(1_835_438_400).daylight);
    assert!(leap.at(1_835_438_400 + 2 * 86_400).daylight);
    let us = parse("EST5EDT").unwrap();
    assert!(us.at(1_782_907_200).daylight);
    assert!(!us.at(1_772_953_199).daylight);
    for bad in [
        "",
        "X1",
        "CET",
        "CET-1CEST,M13.1.0,M10.5.0",
        "CET-1CEST,M3.5.0",
        "CET-1x",
    ] {
        assert!(parse(bad).is_none(), "{bad}");
    }
    assert_eq!(zone("UTC").unwrap().at(0).abbreviation.as_str(), "UTC");
    assert_eq!(UTC.at(0).offset, 0);
}

#[test]
fn local_times_turn_back_into_instants() {
    let berlin = zone("Europe/Berlin").unwrap();
    let noon = 1_782_907_200;
    assert_eq!(berlin.to_utc(noon + 7200), noon);
    let kolkata = zone("Asia/Kolkata").unwrap();
    assert_eq!(kolkata.to_utc(noon + 19_800), noon);
    let local = berlin.at(1_774_746_000);
    assert_eq!(&local.numeric_offset(), b"+0200");
    assert_eq!(
        &zone("America/St_Johns").unwrap().at(noon).numeric_offset(),
        b"-0230"
    );
}

#[test]
fn month_rules_are_what_windows_keeps() {
    let paris = zone("Europe/Paris").unwrap();
    let rules = paris.month_rules().unwrap();
    assert_eq!(
        rules.start,
        MonthRule {
            month: 3,
            week: 5,
            weekday: 0,
            seconds: 7200
        }
    );
    assert_eq!(
        rules.end,
        MonthRule {
            month: 10,
            week: 5,
            weekday: 0,
            seconds: 10_800
        }
    );
    assert_eq!(rules.daylight_offset, 7200);
    assert_eq!(paris.standard_offset(), 3600);
    assert!(zone("Asia/Kolkata").unwrap().month_rules().is_none());
    assert!(
        parse("XXX3YYY,J60/0,J300/0")
            .unwrap()
            .month_rules()
            .is_none()
    );
}
