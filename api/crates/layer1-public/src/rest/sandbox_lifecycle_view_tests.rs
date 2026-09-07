use super::*;
use layer2_core::sandbox::{RuntimeBinding, SandboxReviveCause};

fn capturing_sandbox() -> Sandbox {
    let mut value = stored_sandbox("sbx_capturing", "owner", SandboxStatus::Active, 4);
    value.runtime = Some(RuntimeBinding {
        runtime_id: "runtime-capturing".into(),
        runtime_version: "runtime-image-pinned".into(),
        provision_generation: Some(1),
        bound_at: NOW.into(),
    });
    value.lifecycle.runtime_generation_high_water = 1;
    value.begin_checkpoint_capture(NOW.into()).unwrap();
    value.revision = 1;
    value
}

fn assert_public_lifecycle(value: &Value) {
    let text = serde_json::to_string(value).unwrap();
    for forbidden in [
        "authority",
        "writer_resume_token",
        "lease",
        "holder",
        "fence",
        "runtime_id",
    ] {
        assert!(
            !text.contains(forbidden),
            "private lifecycle field {forbidden}"
        );
    }
}

fn claim_capture(value: &mut Sandbox) -> u64 {
    value
        .claim_checkpoint_capture(
            "checkpoint-worker".into(),
            NOW.into(),
            "2026-09-05T00:05:00Z".into(),
        )
        .unwrap();
    value
        .lifecycle
        .checkpoint_capture
        .as_ref()
        .unwrap()
        .work
        .lease
        .as_ref()
        .unwrap()
        .fence
}

#[tokio::test]
async fn sandbox_list_and_detail_project_checkpoint_eligibility_and_busy_pause_refusal() {
    let fixture = fixture(10);
    let value = capturing_sandbox();
    fixture.store.create_sandbox(value.clone()).await.unwrap();
    let expected = json!({
        "pause": {"allowed": false, "reason": "checkpoint_in_progress"},
        "revive": {"allowed": false, "reason": "not_paused"},
        "checkpoint": {"phase": "capture", "started_at": NOW, "epoch": 1}
    });
    for path in ["/api/v1/sandboxes", "/api/v1/sandboxes/sbx_capturing"] {
        let (status, body) = json_response(
            &fixture.router,
            authenticated(Method::GET, path, None, None),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let sandbox = if path.ends_with("sbx_capturing") {
            &body
        } else {
            &body["items"][0]
        };
        assert_eq!(sandbox["status"], "active");
        assert_eq!(sandbox["lifecycle"], expected);
        assert_public_lifecycle(sandbox);
    }
    let (status, problem) = json_response(
        &fixture.router,
        authenticated(
            Method::POST,
            "/api/v1/sandboxes/sbx_capturing/pause",
            None,
            Some("pause-during-checkpoint"),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(problem["code"], "sandbox_lifecycle_busy");
    assert_eq!(problem["reason"], "checkpoint_in_progress");
    assert_public_lifecycle(&problem);
    assert_eq!(
        fixture.store.get_sandbox(&value.id).await.unwrap(),
        Some(value)
    );
}

#[test]
fn lifecycle_projection_tracks_writer_release_and_records_real_operation_phases() {
    use crate::rest::instances_openapi_shapes::SandboxView;
    let mut value = capturing_sandbox();
    let fence = claim_capture(&mut value);
    value
        .publish_captured_checkpoint(checkpoint(1), "checkpoint-worker", fence, NOW.into())
        .unwrap();
    let view = serde_json::to_value(SandboxView::from(&value)).unwrap();
    assert_eq!(view["lifecycle"]["checkpoint"]["phase"], "resume_writers");
    assert_eq!(view["lifecycle"]["pause"]["allowed"], false);
    assert_public_lifecycle(&view);
    let fence = claim_capture(&mut value);
    value
        .complete_checkpoint_writer_resume("checkpoint-worker", fence, NOW.into())
        .unwrap();
    let view = serde_json::to_value(SandboxView::from(&value)).unwrap();
    assert_eq!(view["lifecycle"]["pause"], json!({"allowed": true}));
    assert!(view["lifecycle"].get("checkpoint").is_none());
    value.begin_pause(NOW.into(), None).unwrap();
    let view = serde_json::to_value(SandboxView::from(&value)).unwrap();
    assert_eq!(
        view["lifecycle"]["operation"],
        json!({"purpose":"pause", "phase":"capture_and_publish_checkpoint", "started_at":NOW})
    );
    assert_public_lifecycle(&view);

    let mut paused = paused_sandbox("sbx_restoring", "owner");
    let view = serde_json::to_value(SandboxView::from(&paused)).unwrap();
    assert_eq!(view["lifecycle"]["revive"], json!({"allowed": true}));
    paused
        .begin_revive(SandboxReviveCause::ExplicitRequest, NOW.into())
        .unwrap();
    let view = serde_json::to_value(SandboxView::from(&paused)).unwrap();
    assert_eq!(
        view["lifecycle"]["operation"],
        json!({"purpose":"revive", "phase":"checkpoint_verification", "started_at":NOW})
    );
    assert_eq!(view["lifecycle"]["revive"]["allowed"], false);
    assert_public_lifecycle(&view);
}
