use super::*;

#[test]
fn retained_owner_wire_remains_strict_and_has_no_execution_fields() {
    let source = r#"{"protocol":1,"agent_id":"synthetic-agent","instance_id":"synthetic-manager","epoch":1}"#;
    let owner: layer3_adapter::owner_protocol::TransportOwner =
        serde_json::from_str(source).unwrap();
    owner.validate().unwrap();
    let value = serde_json::to_value(&owner).unwrap();
    assert_eq!(value.as_object().unwrap().len(), 4);
    let mut invalid = value;
    invalid["resource_execution"] = true.into();
    assert!(
        serde_json::from_value::<layer3_adapter::owner_protocol::TransportOwner>(invalid).is_err()
    );
}

#[test]
fn sidecar_envelope_requires_known_version_and_bounded_record() {
    let mut document = records::encode("activation", &ActivationStateV1::NeverActivated).unwrap();
    assert_eq!(
        records::decode::<ActivationStateV1>(&document).unwrap(),
        ActivationStateV1::NeverActivated
    );
    document.insert("protocol", 2_i32);
    assert!(records::decode::<ActivationStateV1>(&document).is_err());
    document.insert("protocol", 1_i32);
    document.insert("record", " ".repeat(64 * 1024 + 1));
    assert!(records::decode::<ActivationStateV1>(&document).is_err());
}

#[test]
fn activated_history_cannot_become_legacy_when_either_latch_record_is_missing() {
    use collections::activation_pair;
    let never = records::encode("activation", &ActivationStateV1::NeverActivated).unwrap();
    assert_eq!(activation_pair(None, None).unwrap(), None);
    assert_eq!(
        activation_pair(Some(&never), None).unwrap(),
        Some(ActivationStateV1::NeverActivated)
    );
    let state = ActivationStateV1::Activated {
        epoch: wire::Generation::new(1).unwrap(),
    };
    let mut latch = records::encode("activation", &state).unwrap();
    latch.insert("drain_evidence", "a".repeat(64));
    let mut receipt = latch.clone();
    receipt.insert("_id", "activation_receipt");
    assert_eq!(
        activation_pair(Some(&latch), Some(&receipt)).unwrap(),
        Some(state)
    );
    for (latch, receipt) in [
        (None, Some(&receipt)),
        (Some(&latch), None),
        (Some(&never), Some(&receipt)),
    ] {
        assert_eq!(
            activation_pair(latch, receipt),
            Err(ExecutionError::Corrupt)
        );
    }
    receipt.insert("drain_evidence", "b".repeat(64));
    assert_eq!(
        activation_pair(Some(&latch), Some(&receipt)),
        Err(ExecutionError::Corrupt)
    );
}
