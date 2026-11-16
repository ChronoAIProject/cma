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

#[test]
fn bounded_result_is_optional_and_does_not_grant_readiness() {
    let mut value = sandbox();
    value["lifecycle"] = json!({"pause":{"allowed":false},"revive":{"allowed":false}});
    assert!(
        serde_json::from_value::<Sandbox>(value.clone())
            .unwrap()
            .lifecycle
            .unwrap()
            .result
            .is_none()
    );
    value["lifecycle"]["result"] = json!({"state":"recoverable_pending", "code":"lifecycle_wait_exceeded",
        "operation_id":"original", "started_at":"2026-11-16T00:00:00Z", "result_due_at":"2026-11-16T00:01:30Z",
        "continuing":true, "actions":["observe","retire"], "retry_after_seconds":5});
    let parsed = serde_json::from_value::<Sandbox>(value.clone())
        .unwrap()
        .lifecycle
        .unwrap();
    assert!(!parsed.revive.allowed);
    assert_eq!(
        parsed.result.unwrap().code.as_deref(),
        Some("lifecycle_wait_exceeded")
    );
    value["lifecycle"]["result"]["continuing"] = json!("yes");
    assert!(serde_json::from_value::<Sandbox>(value).is_err());
}
