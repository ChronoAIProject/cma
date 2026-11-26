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
    let extended_latch = changed(&latch, KEYS, keys.clone());
    let extended_receipt = changed(&receipt, KEYS, keys);
    assert!(activation_pair(Some(&extended_latch), Some(&extended_receipt)).is_err());
    receipt.insert("drain_evidence", "b".repeat(64));
    assert_eq!(
        activation_pair(Some(&latch), Some(&receipt)),
        Err(ExecutionError::Corrupt)
    );
}
const KEYS: &str = "erasure_keys_v1";

#[test]
fn grant_pair_accepts_matching_versions_and_checks_both_envelopes() {
    let grant: GrantControlV1 = fixture(serde_json::json!({
        "protocol":1, "key":family(), "epoch":1, "revision":1, "members":0, "state":{"state":"open"}
    }));
    let old = records::grant_document(&grant).unwrap();
    let receipt = records::grant_receipt(&grant);
    let keys = doc! {"sandbox_id":"sandbox","owner_subject":"owner","share_id":"share"};
    let extended = changed(&old, KEYS, keys.clone());
    let extended_receipt = changed(&receipt, KEYS, keys.clone());
    let origin = grant.key.origin.clone();
    let valid = Ok(Some(grant));
    let corrupt = Err(ExecutionError::Corrupt);
    for (row, receipt, expected) in [
        (Some(&old), Some(&receipt), &valid),
        (Some(&extended), Some(&extended_receipt), &valid),
        (Some(&old), Some(&extended_receipt), &corrupt),
        (Some(&extended), Some(&receipt), &corrupt),
        (Some(&extended), None, &corrupt),
        (None, Some(&extended_receipt), &corrupt),
    ] {
        assert_eq!(&records::grant_pair(&origin, row, receipt), expected);
    }
    for field in ["sandbox_id", "owner_subject", "share_id", "actor_subject"] {
        let wrong_keys = changed(&keys, field, "other");
        let wrong = changed(&receipt, KEYS, wrong_keys.clone());
        let result = records::grant_pair(&origin, Some(&extended), Some(&wrong));
        assert_eq!(result, corrupt, "{field}");
        if matches!(field, "share_id" | "actor_subject") {
            let wrong_control = changed(&old, KEYS, wrong_keys);
            assert!(records::read_grant(&wrong_control).is_err(), "{field}");
        }
    }
}

#[test]
fn member_and_effect_enforce_v1_attribution_for_each_authority() {
    use layer2_core::resource_access::{ResourceActor, ResourceOwner};
    use serde_json::json;
    let actor: ResourceActor =
        fixture(json!({"kind":"api","subject":"owner","delegation_id":"synthetic-grant"}));
    let actor = records::hash(&("resource-execution-actor-v1", actor)).unwrap();
    let owner_digest = |kind| {
        let owner: ResourceOwner = fixture(json!({"kind":kind,"subject":"owner"}));
        records::hash(&owner).unwrap()
    };
    let (person, organization) = (owner_digest("person"), owner_digest("organization"));
    let execution: wire::ExecutionKey = fixture(json!({
        "command_id":"command", "command_fingerprint":digest(), "dispatch_sequence":1,
        "prompt_id":"prompt", "turn_id":"turn", "agent_id":"agent", "attempt":1,
        "receiver":{"kind":"runtime","instance":"manager","incarnation":digest(),"boot":"boot"}
    }));
    let member = wire::MemberKey::Execution {
        execution: execution.clone(),
    };
    let registration = registration_id(&member).unwrap();
    let independent = |authority, owner| {
        json!({"kind":"independent","authority":authority,
        "owner":owner,"target_ancestry":digest(),"capability":"prompt","actor":actor})
    };
    let cases = json!([
        ["owner", {"kind":"independent_owner","owner":person,"target_ancestry":digest(),"capability":"prompt","actor":actor}],
        ["share", {"kind":"share","grant":{"family":family(),"share_revision":1,"policy_revision":1,
            "target_ancestry":digest(),"capability":"prompt","actor":actor}}],
        ["operator", independent(json!({"kind":"global_operator"}), &person)],
        ["organization", independent(json!({"kind":"owning_organization","issuer":digest(),"organization":digest(),"role":digest()}), &organization)],
        ["binding", independent(json!({"kind":"external_binding","binding":digest(),"generation":1}), &person)]
    ]);
    for case in cases.as_array().unwrap() {
        let role = case[0].as_str().unwrap();
        let basis = &case[1];
        let record: ExecutionRecordV1 = fixture(json!({
            "protocol":1,"registration":registration,"epoch":1,"member":member,
            "target":{"kind":"sandbox","sandbox_id":"sandbox"},"basis":basis,"ordinal":1,"effect_count":0
        }));
        let intent: wire::EffectIntentV1 = fixture(json!({
            "effect_id":"effect","execution":execution,"registration":registration,"grant":basis,
            "operation":"tool_request","request_digest":digest()
        }));
        let permission_id = records::hash(&("resource-effect-permission-v1", &intent)).unwrap();
        let effect: EffectRecordV1 = fixture(json!({"protocol":1,"observations":[],
            "permission":{"protocol":1,"ledger_sequence":1,"permission_id":permission_id,"intent":intent}
        }));
        let member_row = records::member_document(&record).unwrap();
        let effect_row = records::effect_document(&effect).unwrap();
        let mut keys = match role {
            "share" => doc! {"sandbox_id":"sandbox","share_id":"share"},
            "organization" => doc! {"sandbox_id":"sandbox"},
            _ => doc! {"sandbox_id":"sandbox","owner_subject":"owner"},
        };
        let reads = |keys: &Document| {
            (
                records::read_member(&changed(&member_row, KEYS, keys.clone())),
                records::read_effect(&changed(&effect_row, KEYS, keys.clone())),
            )
        };
        let valid = (Ok(record), Ok(effect));
        let corrupt = (Err(ExecutionError::Corrupt), Err(ExecutionError::Corrupt));
        let expected = if role == "binding" { &valid } else { &corrupt };
        assert!(&reads(&keys) == expected, "{role}");
        keys.insert("actor_subject", "owner");
        assert!(reads(&keys) == valid, "{role}");
        if role == "organization" {
            let wrong = changed(&keys, "owner_subject", "owner");
            assert!(records::read_effect(&changed(&effect_row, KEYS, wrong)).is_err());
        }
        if role == "owner" {
            check_invalid_envelopes(&member_row, &keys);
            assert!(reads(&changed(&keys, "actor_subject", "other")) == corrupt);
            let extended = changed(&effect_row, KEYS, keys.clone());
            assert!(records::read_effect(&changed(&extended, "registration", "wrong")).is_err());
        }
        assert!(
            (
                records::read_member(&member_row),
                records::read_effect(&effect_row)
            ) == valid
        );
    }
}

fn check_invalid_envelopes(row: &Document, keys: &Document) {
    let malformed: Vec<mongodb::bson::Bson> = fixture(serde_json::json!([
        null, "metadata", {"actor_subject":"owner"}, {"sandbox_id":"x".repeat(129)},
        {"sandbox_id":"","actor_subject":"owner","owner_subject":"owner"},
        {"sandbox_id":"other","actor_subject":"owner","owner_subject":"owner"},
        {"sandbox_id":"sandbox","actor_subject":"owner","owner_subject":"other"},
        {"sandbox_id":"sandbox","actor_subject":"owner","owner_subject":"owner","extra":"value"},
        {"sandbox_id":"sandbox","actor_subject":["owner","owner"]},
        {"sandbox_id":"sandbox","actor_subject":null}
    ]));
    for invalid in malformed {
        assert!(records::read_member(&changed(row, KEYS, invalid)).is_err());
    }
    let good = changed(row, KEYS, keys.clone());
    for (field, wrong) in doc! {"_id":"other","ordinal":2_i64,"protocol":1_i64,
    "extra":true,"record":format!(" {}", row.get_str("record").unwrap())}
    {
        assert!(records::read_member(&changed(&good, &field, wrong)).is_err());
    }
}

#[test]
fn revocation_checks_erasure_keys_against_the_exact_intent() {
    let mut intent: RevocationIntentV1 = fixture(serde_json::json!({
        "selector":{"kind":"sandbox","sandbox_id":"sandbox"},
        "owner":{"kind":"person","subject":"owner"},
        "actor":{"kind":"human","subject":"owner"},
        "key_sha256":digest(), "change":{"kind":"revoke","share_id":"share"},
        "expected_share_revision":1, "sandbox_id":"sandbox", "sandbox_created_at":"2026-01-01T00:00:00Z",
        "ancestry":digest(), "original_policy_revision":1, "proposed_id":"proposed",
        "requested_at":"2026-01-01T00:00:01Z", "fingerprint":digest()
    }));
    let request = intent.mutation(1);
    intent.fingerprint = wire::Digest::new(&request.fingerprint()).unwrap();
    let operation: RevocationOperationV1 = fixture(serde_json::json!({
        "protocol":1, "key":{"operation_id":request.operation_id(), "family":family(), "watermark":1},
        "epoch":1, "intent":intent, "frozen_members":0, "frozen_at_unix_ms":1000,
        "deadline_unix_ms":1000 + wire::STOP_BOUND_MS, "phase":{"state":"pending"}
    }));
    let old = records::operation_document(&operation).unwrap();
    assert!(records::read_operation(&old).unwrap() == operation);
    let keys = doc! {"sandbox_id":"sandbox","actor_subject":"owner","owner_subject":"owner","share_id":"share"};
    assert!(records::read_operation(&changed(&old, KEYS, keys.clone())).unwrap() == operation);
    for field in keys.keys() {
        assert!(
            records::read_operation(&changed(&old, KEYS, changed(&keys, field, "other"))).is_err()
        );
    }
}
fn changed(row: &Document, field: &str, value: impl Into<mongodb::bson::Bson>) -> Document {
    let mut row = row.clone();
    row.insert(field, value);
    row
}

fn family() -> wire::GrantFamilyKey {
    fixture(serde_json::json!({"origin":digest(),"share_id":"share","generation":1}))
}
fn digest() -> wire::Digest {
    wire::Digest::new(&"a".repeat(64)).unwrap()
}
fn fixture<T: DeserializeOwned>(value: serde_json::Value) -> T {
    serde_json::from_value(value).unwrap()
}
