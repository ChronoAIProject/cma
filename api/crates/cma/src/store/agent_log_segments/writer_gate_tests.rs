use super::*;

#[test]
fn defaults_are_closed_and_selections_are_exact_and_independent() {
    let closed = Policy::default();
    closed.validate().unwrap();
    assert!(!closed.selects("agt_fixture", "owner-fixture", "creation-fixture"));
    let enabled =
        Policy::parse(r#"{"enabled":true,"agents":["agt_fixture"],"owners":["owner-selected"],"cohort_seed":"","basis_points":0}"#)
            .unwrap();
    assert!(enabled.selects("agt_fixture", "owner-other", "creation-fixture"));
    assert!(enabled.selects("agt_other", "owner-selected", "creation-fixture"));
    assert!(!enabled.selects(
        "agt_fixture_suffix",
        "owner-selected_suffix",
        "creation-fixture"
    ));
    assert!(!Policy {
        enabled: false,
        ..enabled.clone()
    }
    .selects("agt_fixture", "owner-selected", "creation-fixture"));
    let gate = GateRecord {
        name: GateName::Segments,
        format_version: 1,
        revision: 7,
        admission_fence: 11,
        policy: enabled,
        slots: Vec::new(),
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
        r#"{"enabled":true,"agents":[],"owners":[],"cohort_seed":"","basis_points":0}"#,
        r#"{"enabled":true,"agents":["*"],"owners":[],"cohort_seed":"","basis_points":0}"#,
        r#"{"enabled":true,"agents":["agt_a","agt_a"],"owners":[],"cohort_seed":"","basis_points":0}"#,
        r#"{"enabled":true,"agents":[],"owners":["owner-a","owner-a"],"cohort_seed":"","basis_points":0}"#,
        r#"{"enabled":true,"agents":[" agt_a"],"owners":[],"cohort_seed":"","basis_points":0}"#,
        r#"{"enabled":true,"agents":["agt_a"],"owners":[],"cohort_seed":"","basis_points":0,"ui_enabled":true}"#,
        r#"{"enabled":true,"enabled":false,"agents":["agt_a"],"owners":[],"cohort_seed":"","basis_points":0}"#,
        r#"{"enabled":false,"agents":[""],"owners":[],"cohort_seed":"","basis_points":0}"#,
    ] {
        assert!(Policy::parse(json).is_err());
    }
    let mut bounded = Policy {
        enabled: true,
        agents: (0..64).map(|n| format!("agt_fixture_{n}")).collect(),
        owners: Vec::new(),
        ..Policy::default()
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
        slots: Vec::new(),
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

#[test]
fn percentage_membership_is_stable_creation_bound_and_monotone() {
    let incarnation = "0".repeat(64);
    assert_eq!(
        cohort_bucket("fixture-seed", "agt_fixture", &incarnation),
        1920
    );
    let mut policy = Policy {
        enabled: true,
        cohort_seed: "fixture-seed".into(),
        basis_points: 1920,
        ..Policy::default()
    };
    policy.validate_for(GateName::Segments).unwrap();
    assert!(!policy.selects("agt_fixture", "owner", &incarnation));
    policy.basis_points = 1921;
    assert!(policy.selects("agt_fixture", "different-owner", &incarnation));
    for n in 0..1000 {
        let id = format!("agt_fixture_{n}");
        let mut was_selected = false;
        for threshold in [0, 1000, 5000, 10_000] {
            policy.basis_points = threshold;
            let selected = policy.selects(&id, "owner", &incarnation);
            assert!(!was_selected || selected);
            was_selected = selected;
        }
        assert!(was_selected);
    }
    assert_ne!(
        cohort_bucket("fixture-seed", "agt_fixture", &incarnation),
        cohort_bucket("fixture-seed", "agt_fixture", &"1".repeat(64))
    );
}

#[test]
fn recovery_never_inherits_owner_or_percentage_selection() {
    let mut policy = Policy {
        enabled: true,
        agents: vec!["agt_fixture".into()],
        ..Policy::default()
    };
    policy.validate_for(GateName::CapacityRecovery).unwrap();
    policy.owners.push("owner".into());
    assert!(policy.validate_for(GateName::CapacityRecovery).is_err());
    policy.owners.clear();
    policy.cohort_seed = "fixture-seed".into();
    policy.basis_points = 10_000;
    assert!(policy.validate_for(GateName::CapacityRecovery).is_err());
    policy.validate_for(GateName::Segments).unwrap();
    policy.basis_points = 10_001;
    assert!(policy.validate().is_err());
    policy.basis_points = 1;
    policy.cohort_seed.clear();
    assert!(policy.validate().is_err());
}

#[test]
fn persisted_slots_are_bounded_unique_and_fenced() {
    let slot = slots::Slot {
        holder: "fixture-operation".into(),
        fingerprint: super::super::digest::binary(vec![1; 32]),
        fence: 1,
        expires_at: bson::DateTime::from_millis(1),
    };
    let mut gate = GateRecord {
        name: GateName::CapacityRecovery,
        format_version: 1,
        revision: 1,
        admission_fence: 1,
        policy: Policy::default(),
        slots: vec![slot.clone()],
    };
    slots::validate(&gate).unwrap();
    gate.slots.push(slot.clone());
    assert!(slots::validate(&gate).is_err());
    gate.name = GateName::Segments;
    assert!(slots::validate(&gate).is_err());
    gate.slots[1].holder = "fixture-other-operation".into();
    assert!(slots::validate(&gate).is_err());
    gate.slots[1].fence = 2;
    assert!(slots::validate(&gate).is_err());
    gate.admission_fence = 2;
    slots::validate(&gate).unwrap();
    gate.slots.pop();
    gate.slots[0].fence = 0;
    assert!(slots::validate(&gate).is_err());
}
