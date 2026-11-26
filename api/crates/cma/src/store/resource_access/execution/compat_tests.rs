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
    let keys = doc! {"sandbox_id":"sandbox"};
    assert!(activation_pair(
        Some(&with_keys(&latch, keys.clone())),
        Some(&with_keys(&receipt, keys))
    )
    .is_err());
    receipt.insert("drain_evidence", "b".repeat(64));
    assert_eq!(
        activation_pair(Some(&latch), Some(&receipt)),
        Err(ExecutionError::Corrupt)
    );
}

const KEYS: &str = "erasure_keys_v1";

#[test]
fn grant_pair_accepts_matching_versions_and_checks_both_envelopes() {
    let grant: GrantControlV1 = serde_json::from_value(serde_json::json!({
        "protocol":1, "key":family(), "epoch":1, "revision":1, "members":0, "state":{"state":"open"}
    }))
    .unwrap();
    let old = records::grant_document(&grant).unwrap();
    let receipt = records::grant_receipt(&grant);
    let keys = doc! {"sandbox_id":"sandbox","owner_subject":"owner","share_id":"share"};
    let extended = with_keys(&old, keys.clone());
    let extended_receipt = with_keys(&receipt, keys.clone());
    let read =
        |a: Option<&Document>, b: Option<&Document>| records::grant_pair(&grant.key.origin, a, b);
    for (row, evidence) in [(&old, &receipt), (&extended, &extended_receipt)] {
        let value = read(Some(row), Some(evidence)).unwrap();
        assert_eq!(value.as_ref(), Some(&grant));
    }
    for (row, evidence) in [
        (Some(&old), Some(&extended_receipt)),
        (Some(&extended), Some(&receipt)),
        (Some(&extended), None),
        (None, Some(&extended_receipt)),
    ] {
        assert!(read(row, evidence).is_err());
    }
    for (field, value) in [
        ("sandbox_id", "other"),
        ("owner_subject", "other"),
        ("share_id", "other"),
        ("actor_subject", "owner"),
    ] {
        let mut changed = keys.clone();
        changed.insert(field, value);
        assert!(read(Some(&extended), Some(&with_keys(&receipt, changed.clone()))).is_err());
        if matches!(field, "share_id" | "actor_subject") {
            assert!(records::read_grant(&with_keys(&old, changed)).is_err());
        }
    }
}

#[test]
fn member_and_effect_accept_legacy_and_validate_bound_erasure_roles() {
    use layer2_core::resource_access::{ResourceOwner, ResourceTarget};
    let execution: wire::ExecutionKey = serde_json::from_value(serde_json::json!({
        "command_id": "command", "command_fingerprint": digest(), "dispatch_sequence": 1,
        "prompt_id": "prompt", "turn_id": "turn", "agent_id": "agent", "attempt": 1,
        "receiver": {"kind":"runtime","instance":"manager","incarnation":digest(),"boot":"boot"}
    }))
    .unwrap();
    let member = wire::MemberKey::Execution {
        execution: execution.clone(),
    };
    let registration = registration_id(&member).unwrap();
    let owner = ResourceOwner::Person {
        subject: layer2_core::SubjectId::new("owner"),
    };
    let record: ExecutionRecordV1 = serde_json::from_value(serde_json::json!({
        "protocol": 1, "registration": registration, "epoch": 1, "member": member,
        "target": ResourceTarget::Sandbox { sandbox_id: layer2_core::SandboxId("sandbox".into()) },
        "basis": {"kind":"independent_owner", "owner":records::hash(&owner).unwrap(),
                  "actor":digest(), "target_ancestry":digest(), "capability":"prompt"},
        "ordinal":1, "effect_count":0
    }))
    .unwrap();
    let keys = doc! {"sandbox_id":"sandbox","actor_subject":"owner","owner_subject":"owner"};
    let old = records::member_document(&record).unwrap();
    assert!(records::read_member(&old).unwrap() == record);
    assert!(records::read_member(&with_keys(&old, keys.clone())).unwrap() == record);
    for invalid in [
        doc! {"actor_subject":"owner","owner_subject":"owner"},
        doc! {"sandbox_id":"sandbox","actor_subject":"owner","owner_subject":"other"},
        doc! {"sandbox_id":"other","actor_subject":"owner","owner_subject":"owner"},
        doc! {"sandbox_id":"sandbox","actor_subject":["owner","owner"]},
        doc! {"sandbox_id":"sandbox","actor_subject":null},
        doc! {"sandbox_id":"sandbox","actor_subject":""},
        doc! {"sandbox_id":"x".repeat(129)},
        doc! {"sandbox_id":"sandbox","unknown":"value"},
    ] {
        assert!(records::read_member(&with_keys(&old, invalid)).is_err());
    }
    let valid = with_keys(&old, keys);
    for (field, bad) in doc! {"_id":"other","ordinal":2_i64,"protocol":1_i64,"extra":true,"record":format!(" {}", old.get_str("record").unwrap())}
    {
        let mut changed = valid.clone();
        changed.insert(field, bad);
        assert!(records::read_member(&changed).is_err());
    }
    for malformed in [mongodb::bson::Bson::Null, "metadata".into()] {
        assert!(records::read_member(&with_keys(&old, malformed)).is_err());
    }
    let organization = ResourceOwner::Organization {
        subject: layer2_core::SubjectId::new("organization"),
    };
    let grant = serde_json::json!({
        "kind":"independent", "owner":records::hash(&organization).unwrap(), "actor":digest(),
        "target_ancestry":digest(), "capability":"prompt",
        "authority":{"kind":"owning_organization","issuer":digest(),"organization":digest(),"role":digest()}
    });
    let intent: wire::EffectIntentV1 = serde_json::from_value(serde_json::json!({
        "effect_id":"effect", "execution":execution, "registration":registration, "grant":grant,
        "operation":"tool_request", "request_digest":digest()
    }))
    .unwrap();
    let effect = EffectRecordV1 {
        protocol: 1,
        observations: vec![],
        permission: wire::EffectPermissionV1 {
            protocol: 1,
            ledger_sequence: generation(),
            permission_id: records::hash(&("resource-effect-permission-v1", &intent)).unwrap(),
            intent,
        },
    };
    let old = records::effect_document(&effect).unwrap();
    assert!(records::read_effect(&old).unwrap() == effect);
    let effect_keys = doc! {"sandbox_id":"sandbox","actor_subject":"delegate"};
    let mut extended = with_keys(&old, effect_keys.clone());
    assert!(records::read_effect(&extended).unwrap() == effect);
    let mut invalid = effect_keys;
    invalid.insert("owner_subject", "organization");
    assert!(records::read_effect(&with_keys(&old, invalid)).is_err());
    extended.insert("registration", "different");
    assert!(records::read_effect(&extended).is_err());
}

#[test]
fn revocation_checks_erasure_keys_against_the_exact_intent() {
    let mut intent: RevocationIntentV1 = serde_json::from_value(serde_json::json!({
        "selector":{"kind":"sandbox","sandbox_id":"sandbox"},
        "owner":{"kind":"person","subject":"owner"},
        "actor":{"kind":"api","subject":"owner","delegation_id":null},
        "key_sha256":digest(), "change":{"kind":"revoke","share_id":"share"},
        "expected_share_revision":1, "sandbox_id":"sandbox", "sandbox_created_at":"2026-01-01T00:00:00Z",
        "ancestry":digest(), "original_policy_revision":1, "proposed_id":"proposed",
        "requested_at":"2026-01-01T00:00:01Z", "fingerprint":digest()
    })).unwrap();
    let request = intent.mutation(1);
    intent.fingerprint = wire::Digest::new(&request.fingerprint()).unwrap();
    let operation: RevocationOperationV1 = serde_json::from_value(serde_json::json!({
        "protocol":1, "key":{"operation_id":request.operation_id(), "family":family(), "watermark":1},
        "epoch":1, "intent":intent, "frozen_members":0, "frozen_at_unix_ms":1000,
        "deadline_unix_ms":1000 + wire::STOP_BOUND_MS, "phase":{"state":"pending"}
    })).unwrap();
    let old = records::operation_document(&operation).unwrap();
    assert!(records::read_operation(&old).unwrap() == operation);
    let keys = doc! {"sandbox_id":"sandbox","actor_subject":"owner","owner_subject":"owner","share_id":"share"};
    assert!(records::read_operation(&with_keys(&old, keys.clone())).unwrap() == operation);
    for field in keys.keys() {
        let mut changed = keys.clone();
        changed.insert(field, "other");
        assert!(records::read_operation(&with_keys(&old, changed)).is_err());
    }
}

fn with_keys(row: &Document, keys: impl Into<mongodb::bson::Bson>) -> Document {
    let mut row = row.clone();
    row.insert(KEYS, keys);
    row
}

fn family() -> wire::GrantFamilyKey {
    wire::GrantFamilyKey {
        origin: digest(),
        share_id: wire::OpaqueId::new("share").unwrap(),
        generation: generation(),
    }
}

fn digest() -> wire::Digest {
    wire::Digest::new(&"a".repeat(64)).unwrap()
}

fn generation() -> wire::Generation {
    wire::Generation::new(1).unwrap()
}
