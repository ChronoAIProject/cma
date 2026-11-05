use super::*;

#[test]
fn defaults_are_closed_and_selections_are_exact_and_independent() {
    let closed = Policy::default();
    closed.validate().unwrap();
    assert!(!closed.selects("agt_fixture", "owner-fixture"));
    let enabled =
        Policy::parse(r#"{"enabled":true,"agents":["agt_fixture"],"owners":["owner-selected"]}"#)
            .unwrap();
    assert!(enabled.selects("agt_fixture", "owner-other"));
    assert!(enabled.selects("agt_other", "owner-selected"));
    assert!(!enabled.selects("agt_fixture_suffix", "owner-selected_suffix"));
    assert!(!Policy {
        enabled: false,
        ..enabled.clone()
    }
    .selects("agt_fixture", "owner-selected"));
    let gate = GateRecord {
        name: GateName::Segments,
        format_version: 1,
        revision: 7,
        admission_fence: 11,
        policy: enabled,
    };
    let document = bson::serialize_to_document(&gate).unwrap();
    assert_eq!(
        GateRecord::read(GateName::Segments, document.clone()).unwrap(),
        gate
    );
    assert!(GateRecord::read(GateName::CapacityRecovery, document).is_err());
}

#[test]
fn policies_refuse_wildcards_duplicates_unknown_fields_and_unbounded_cohorts() {
    for json in [
        "",
        "{}",
        r#"{"enabled":true,"agents":[],"owners":[]}"#,
        r#"{"enabled":true,"agents":["*"],"owners":[]}"#,
        r#"{"enabled":true,"agents":["agt_a","agt_a"],"owners":[]}"#,
        r#"{"enabled":true,"agents":[],"owners":["owner-a","owner-a"]}"#,
        r#"{"enabled":true,"agents":[" agt_a"],"owners":[]}"#,
        r#"{"enabled":true,"agents":["agt_a"],"owners":[],"ui_enabled":true}"#,
        r#"{"enabled":true,"enabled":false,"agents":["agt_a"],"owners":[]}"#,
        r#"{"enabled":false,"agents":[""],"owners":[]}"#,
    ] {
        assert!(Policy::parse(json).is_err());
    }
    let mut bounded = Policy {
        enabled: true,
        agents: (0..64).map(|n| format!("agt_fixture_{n}")).collect(),
        owners: Vec::new(),
    };
    bounded.validate().unwrap();
    bounded.owners.push("owner-fixture".into());
    assert!(bounded.validate().is_err());
    bounded.agents.clear();
    bounded.owners = vec!["x".repeat(513)];
    assert!(bounded.validate().is_err());
    assert!(Policy::parse(&" ".repeat(MAX_POLICY_JSON + 1)).is_err());
}

#[test]
fn stored_gate_versions_revisions_fences_and_required_fields_are_strict() {
    let gate = GateRecord {
        name: GateName::CapacityRecovery,
        format_version: 1,
        revision: 1,
        admission_fence: 0,
        policy: Policy::default(),
    };
    let document = bson::serialize_to_document(&gate).unwrap();
    for field in document.keys() {
        let mut missing = document.clone();
        missing.remove(field);
        assert!(GateRecord::read(GateName::CapacityRecovery, missing).is_err());
    }
    for (field, value) in [
        ("format_version", 2),
        ("revision", 0),
        ("admission_fence", -1),
    ] {
        let mut malformed = document.clone();
        malformed.insert(field, value as i64);
        assert!(GateRecord::read(GateName::CapacityRecovery, malformed).is_err());
    }
    let mut future = document;
    future.insert("future", true);
    assert!(GateRecord::read(GateName::CapacityRecovery, future).is_err());
}
