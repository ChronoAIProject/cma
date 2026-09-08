use super::sandbox;
use cma_sdk::instances::{
    Sandbox, SandboxCheckpointPhase, SandboxLifecycleBlocker, SandboxLifecycleOperation,
    SandboxRevivePhase,
};
use serde_json::json;

#[test]
fn sandbox_lifecycle_retains_eligibility_and_recorded_checkpoint_phases() {
    for (phase, expected) in [
        ("capture", SandboxCheckpointPhase::Capture),
        ("resume_writers", SandboxCheckpointPhase::ResumeWriters),
    ] {
        let mut value = sandbox();
        value["lifecycle"] = json!({
            "pause": {"allowed":false,"reason":"checkpoint_in_progress"},
            "revive": {"allowed":false,"reason":"not_paused"},
            "checkpoint": {"phase":phase,"started_at":"2026-09-08T00:00:00Z","epoch":6}
            ,"latest_snapshot": {"epoch":5,"saved_at":"2026-09-07T23:30:00Z","download_url":"/api/v1/sandboxes/sbx_1/snapshots/5/download"}
        });
        let parsed: Sandbox = serde_json::from_value(value).unwrap();
        let lifecycle = parsed.lifecycle.unwrap();
        assert!(!lifecycle.pause.allowed);
        assert_eq!(
            lifecycle.pause.reason,
            Some(SandboxLifecycleBlocker::CheckpointInProgress)
        );
        let checkpoint = lifecycle.checkpoint.unwrap();
        assert_eq!(checkpoint.phase, expected);
        assert_eq!(checkpoint.epoch, 6);
        assert_eq!(checkpoint.started_at, "2026-09-08T00:00:00Z");
        assert!(lifecycle.operation.is_none());
        assert_eq!(lifecycle.latest_snapshot.unwrap().epoch, 5);
    }
    let parsed: Sandbox = serde_json::from_value(sandbox()).unwrap();
    assert!(parsed.lifecycle.is_none());
}

#[test]
fn sandbox_lifecycle_requires_known_operation_phases_and_boolean_eligibility() {
    let mut value = sandbox();
    value["lifecycle"] = json!({
        "pause":{"allowed":false,"reason":"not_active"},
        "revive":{"allowed":false,"reason":"not_paused"},
        "operation":{"purpose":"revive","phase":"home_restore","started_at":"2026-09-08T00:00:00Z"}
    });
    let parsed: Sandbox = serde_json::from_value(value.clone()).unwrap();
    assert!(matches!(
        parsed.lifecycle.unwrap().operation,
        Some(SandboxLifecycleOperation::Revive {
            phase: SandboxRevivePhase::HomeRestore,
            ..
        })
    ));
    for (path, replacement) in [
        ("/lifecycle/pause/allowed", json!("yes")),
        ("/lifecycle/pause/reason", json!("unknown")),
        ("/lifecycle/operation/purpose", json!("unknown")),
        ("/lifecycle/operation/phase", json!("publish_paused")),
    ] {
        let mut malformed = value.clone();
        *malformed.pointer_mut(path).unwrap() = replacement;
        assert!(serde_json::from_value::<Sandbox>(malformed).is_err());
    }
}
