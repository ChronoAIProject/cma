use super::*;
use crate::sandbox::{SandboxEventKind, SandboxLifecycleState};
use serde_json::{json, Value};

const AT: &str = "2026-11-16T00:00:00Z";

fn pin() -> Value {
    json!({
        "restore_id": "restore-example", "backup_id": "hb-example-1",
        "operation_id": "sandbox-lifecycle-v1:example:2", "requested_at": AT,
        "requested_by": "operator-example", "key_sha256": "a".repeat(64),
        "evidence": {"kind": "gone", "observed_at": AT}
    })
}

fn receipt() -> Value {
    json!({
        "inventory_sha256": "b".repeat(64), "restored_to": AT, "completed_at": AT,
        "home_identity": "restored-home-example", "source_generation": 2,
        "covered_agents": ["agent-covered"], "uncovered_agents": ["agent-missing"],
        "missing_workspaces": ["workspace-missing"], "missing_worktrees": ["worktree-missing"],
        "removed_agent_homes": ["agent-deleted"], "removed_worktrees": ["worktree-deleted"]
    })
}

#[test]
fn legacy_lifecycle_remains_readable_without_emitting_restore_fields() {
    let old = json!({"next_authority": 1});
    let lifecycle: SandboxLifecycleState = serde_json::from_value(old).unwrap();
    assert!(lifecycle.home_restore.is_none());
    assert!(lifecycle.superseded_homes.is_empty());
    let encoded = serde_json::to_value(lifecycle).unwrap();
    assert!(encoded.get("home_restore").is_none());
    assert!(encoded.get("superseded_homes").is_none());
}

#[test]
fn pin_and_completed_receipt_round_trip_without_losing_evidence() {
    for kind in ["gone", "deleting", "unbound", "present", "unreadable"] {
        let mut value = pin();
        value["evidence"]["kind"] = json!(kind);
        value["evidence"]["observed_storage_generation"] = json!("observed-generation");
        value["replace_existing_home"] = json!({"storage_generation": "observed-generation"});
        for completed in [false, true] {
            if completed {
                value["receipt"] = receipt();
            }
            let parsed: SandboxHomeRestore = serde_json::from_value(value.clone()).unwrap();
            parsed.validate().unwrap();
            assert_eq!(serde_json::to_value(parsed).unwrap(), value);
        }
    }
}

#[test]
fn restore_reader_refuses_unknown_fields_variants_and_incomplete_receipts() {
    for path in ["pin", "evidence", "receipt", "attestation"] {
        let mut value = pin();
        match path {
            "evidence" => value["evidence"]["unknown"] = json!(true),
            "receipt" => {
                value["receipt"] = receipt();
                value["receipt"]["unknown"] = json!(true);
            }
            "attestation" => {
                value["replace_existing_home"] = json!({
                    "storage_generation": "original", "unknown": true
                })
            }
            _ => value["unknown"] = json!(true),
        }
        assert!(
            serde_json::from_value::<SandboxHomeRestore>(value).is_err(),
            "{path}"
        );
    }
    let mut value = pin();
    value["evidence"]["kind"] = json!("future_evidence");
    assert!(serde_json::from_value::<SandboxHomeRestore>(value).is_err());
    let mut value = pin();
    value["receipt"] = json!({"completed_at": AT});
    assert!(serde_json::from_value::<SandboxHomeRestore>(value).is_err());
}

#[test]
fn restore_validation_refuses_invalid_digests_timestamps_and_overlapping_coverage() {
    for field in ["key_sha256", "requested_at", "operation_id"] {
        let mut value = pin();
        value[field] = json!("");
        assert!(serde_json::from_value::<SandboxHomeRestore>(value)
            .unwrap()
            .validate()
            .is_err());
    }
    let mut value = pin();
    value["receipt"] = receipt();
    value["receipt"]["uncovered_agents"] = json!(["agent-covered"]);
    assert!(serde_json::from_value::<SandboxHomeRestore>(value)
        .unwrap()
        .validate()
        .is_err());
}

#[test]
fn restored_event_round_trips_and_unknown_event_fields_are_refused() {
    let event = json!({"type": "home_restored", "restore_id": "restore-example",
        "backup_id": "hb-example-1", "restored_to": AT});
    let parsed: SandboxEventKind = serde_json::from_value(event.clone()).unwrap();
    assert_eq!(serde_json::to_value(parsed).unwrap(), event);
    let mut invalid = event;
    invalid["unknown"] = json!(true);
    assert!(serde_json::from_value::<SandboxEventKind>(invalid).is_err());
}

fn home(name: &str) -> SandboxHome {
    let id = crate::SandboxId("sbx_lifecycle".into());
    let mut home = SandboxHome::reserve(
        &id,
        layer3_adapter::HomeVolumePolicy {
            mode: Default::default(),
            storage_class: "fixture-storage".into(),
            capacity_gib: 64,
            template_name: "fixture-seed".into(),
            template_generation: "seed-generation".into(),
        },
    )
    .unwrap();
    home.bind(layer3_adapter::HomeVolumeReceipt {
        native_authority: None,
        volume_id: home.request.volume_id.clone(),
        request_sha256: home.request.computed_sha256(),
        storage_reference: name.into(),
        storage_generation: format!("generation-{name}"),
    })
    .unwrap();
    home
}

#[test]
fn superseded_home_retains_original_authority_and_cannot_become_current_again() {
    let mut sandbox = crate::sandbox::lifecycle_test_support::sandbox();
    let old = home("original-home");
    sandbox.lifecycle.runtime_generation_high_water = 2;
    sandbox.lifecycle.home = Some(Box::new(home("current-home")));
    sandbox
        .lifecycle
        .superseded_homes
        .push(SandboxSupersededHome {
            restore_id: "restore-example".into(),
            home_identity: old.attachment().unwrap().identity().unwrap(),
            home: Box::new(old.clone()),
            source_generation: 1,
            superseded_at: AT.into(),
        });
    sandbox.lifecycle.validate_home_restore(&sandbox).unwrap();
    let encoded = serde_json::to_value(&sandbox.lifecycle).unwrap();
    let decoded: SandboxLifecycleState = serde_json::from_value(encoded.clone()).unwrap();
    assert_eq!(decoded, sandbox.lifecycle);
    sandbox.lifecycle.home = Some(Box::new(old));
    assert!(sandbox.lifecycle.validate_home_restore(&sandbox).is_err());
    let mut invalid = encoded;
    invalid["superseded_homes"][0]["unknown"] = json!(true);
    assert!(serde_json::from_value::<SandboxLifecycleState>(invalid).is_err());
}

#[test]
fn superseded_home_refuses_wrong_identity_generation_and_duplicates() {
    let mut sandbox = crate::sandbox::lifecycle_test_support::sandbox();
    let old = home("original-home");
    sandbox.lifecycle.runtime_generation_high_water = 2;
    let superseded = SandboxSupersededHome {
        restore_id: "restore-example".into(),
        home_identity: old.attachment().unwrap().identity().unwrap(),
        home: Box::new(old),
        source_generation: 1,
        superseded_at: AT.into(),
    };
    for field in ["identity", "generation", "duplicate", "timestamp"] {
        let mut old = superseded.clone();
        match field {
            "identity" => old.home_identity = "wrong-home".into(),
            "generation" => old.source_generation = 3,
            "timestamp" => old.superseded_at = "invalid".into(),
            _ => {}
        }
        sandbox.lifecycle.superseded_homes = vec![old.clone()];
        if field == "duplicate" {
            sandbox.lifecycle.superseded_homes.push(old);
        }
        assert!(
            sandbox.lifecycle.validate_home_restore(&sandbox).is_err(),
            "{field}"
        );
    }
}
