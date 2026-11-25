use super::*;
use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::sandbox::{
    RuntimeBinding, SandboxCredentialAuthority, SandboxImageRotationPhase,
    SandboxLifecycleOperation, SandboxRotationIdentity, SandboxRuntimeUpdateService,
    SandboxUnpublishedRecoveryProof, SandboxUnpublishedRecoverySnapshot,
    SandboxUnpublishedRecoveryVerifier, TypedFailure,
};
const PATH: &str = "/api/v1/sandboxes/sbx_update/runtime-update";

#[path = "sandbox_retained_recovery_tests.rs"]
pub(crate) mod retained;

pub(crate) async fn update_fixture(configured: bool) -> Fixture {
    let mut sandbox = stored_sandbox("sbx_update", "owner", SandboxStatus::Active, 4);
    sandbox.runtime = Some(RuntimeBinding {
        runtime_id: "retained-runtime".into(),
        runtime_version: "runtime-v2".into(),
        provision_generation: None,
        boot_id: None,
        bound_at: NOW.into(),
    });
    runtime_update_fixture(sandbox, configured).await
}

async fn runtime_update_fixture(sandbox: Sandbox, configured: bool) -> Fixture {
    let mut f = fixture(3);
    f.store.create_sandbox(sandbox).await.unwrap();
    if configured {
        f.state = f
            .state
            .with_runtime_updates(Arc::new(SandboxRuntimeUpdateService::new(
                f.store.clone(),
                "runtime-v2".into(),
            )));
    }
    f.router = hierarchy_router(f.state.clone());
    f
}

pub(in crate::rest) fn native_runtime_update_sandbox(
    mut sandbox: Sandbox,
    custom: bool,
) -> Sandbox {
    let home = retained::reopen::home(&sandbox.id);
    let source = home
        .native_pool
        .as_ref()
        .unwrap()
        .request
        .pool
        .spec
        .image
        .uri
        .clone();
    sandbox.profile.content.runtime_image = custom.then(|| source.clone());
    sandbox.runtime = Some(RuntimeBinding {
        runtime_id: "agent-sandbox:member:sandbox-uid".into(),
        runtime_version: source,
        provision_generation: Some(1),
        boot_id: Some("b".repeat(32)),
        bound_at: NOW.into(),
    });
    sandbox.lifecycle.home = Some(Box::new(home));
    sandbox.lifecycle.runtime_generation_high_water = 1;
    sandbox.work = SandboxWorkState::new(SandboxWorkStep::None, NOW.into());
    sandbox.validate().unwrap();
    sandbox
}

pub(in crate::rest) fn assert_managed_update_queued(
    original: &Sandbox,
    stored: &Sandbox,
    view: &Value,
) {
    let request = stored.runtime_update_request().unwrap();
    assert_eq!(request.target_version, "runtime-v2");
    assert!(!request.automatic);
    assert!(request.started_at.is_none());
    assert_eq!(view["runtime_update"]["state"], "queued");
    assert_eq!(
        view["runtime_update"]["target_version"],
        request.target_version
    );
    assert_eq!(view["revision"], stored.revision);
    assert_eq!(stored.revision, original.revision + 1);
    assert_eq!(stored.id, original.id);
    assert_eq!(stored.owner, original.owner);
    assert_eq!(stored.status, original.status);
    assert_eq!(stored.profile, original.profile);
    assert_eq!(stored.lifecycle.home, original.lifecycle.home);
    assert_eq!(stored.runtime, original.runtime);
}

#[tokio::test]
async fn owner_managed_runtime_update_refuses_an_explicit_image_on_a_retained_home() {
    for custom in [true, false] {
        let original = native_runtime_update_sandbox(
            stored_sandbox("sbx_update", "owner", SandboxStatus::Active, 4),
            custom,
        );
        let f = runtime_update_fixture(original.clone(), true).await;
        let (status, body) = json_response(
            &f.router,
            authenticated(
                Method::POST,
                PATH,
                Some(json!({"target_version":"runtime-v2"})),
                Some("managed-update"),
            ),
        )
        .await;
        let stored = f.store.get_sandbox(&original.id).await.unwrap().unwrap();
        if custom {
            assert_eq!(status, StatusCode::CONFLICT, "{body}");
            assert_eq!(body["code"], "runtime_update_not_available");
            assert_eq!(stored, original);
        } else {
            assert_eq!(status, StatusCode::ACCEPTED, "{body}");
            assert_managed_update_queued(&original, &stored, &body);
        }
    }
}

#[tokio::test]
async fn runtime_update_endpoint_auth_target_and_idempotent_noop_are_exact() {
    let f = update_fixture(true).await;
    let body = json!({"target_version":"runtime-v2"});
    for (subject, expected) in [
        (None, StatusCode::UNAUTHORIZED),
        (
            Some(("owner", Authorization::none())),
            StatusCode::FORBIDDEN,
        ),
        (Some(("foreign", all_capabilities())), StatusCode::NOT_FOUND),
    ] {
        assert_eq!(
            json_response(
                &f.router,
                request(
                    Method::POST,
                    PATH,
                    Some(body.clone()),
                    subject,
                    Some("auth")
                )
            )
            .await
            .0,
            expected
        );
    }
    assert_eq!(
        json_response(
            &f.router,
            authenticated(Method::POST, PATH, Some(body.clone()), None)
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let (status, result) = json_response(
        &f.router,
        authenticated(Method::POST, PATH, Some(body.clone()), Some("same-target")),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(result["runtime_update"]["state"], "up_to_date");
    assert_eq!(result["runtime_update"]["current_version"], "runtime-v2");
    assert_eq!(result["revision"], 1);
    assert_eq!(
        json_response(
            &f.router,
            authenticated(Method::POST, PATH, Some(body), Some("same-target"))
        )
        .await,
        (status, result)
    );
    let stale = json!({"target_version":"runtime-v1"});
    let (status, result) = json_response(
        &f.router,
        authenticated(Method::POST, PATH, Some(stale.clone()), Some("stale")),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(result["code"], "runtime_update_target_changed");
    assert_eq!(
        json_response(
            &f.router,
            authenticated(Method::POST, PATH, Some(stale), Some("same-target"))
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
}

#[tokio::test]
async fn runtime_update_detail_projection_is_optional_without_feature_and_hides_other_owner() {
    let configured = update_fixture(true).await;
    let path = "/api/v1/sandboxes/sbx_update";
    let (status, result) = json_response(
        &configured.router,
        authenticated(Method::GET, path, None, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        result["runtime_update"]["schedule"],
        json!({"timezone":"Asia/Singapore","local_time":"02:00"})
    );
    assert!(result["runtime_update"].get("requested_at").is_none());
    let unconfigured = update_fixture(false).await;
    let (_, result) = json_response(
        &unconfigured.router,
        authenticated(Method::GET, path, None, None),
    )
    .await;
    assert!(result.get("runtime_update").is_none());
    let result = json_response(
        &configured.router,
        request(
            Method::GET,
            path,
            None,
            Some(("foreign", all_capabilities())),
            None,
        ),
    )
    .await;
    assert_eq!(result.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn failed_sandbox_reports_new_runtime_without_suggesting_pending_work() {
    let mut original = native_runtime_update_sandbox(
        stored_sandbox("sbx_update", "owner", SandboxStatus::Active, 4),
        false,
    );
    let f = runtime_update_fixture(original.clone(), true).await;
    let active_revision = original.revision;
    original
        .transition(
            SandboxStatus::Failed,
            NOW.into(),
            Some(TypedFailure {
                class: "terminal_validation".into(),
                detail: "Synthetic retained Sandbox failure".into(),
            }),
        )
        .unwrap();
    original.validate().unwrap();
    assert!(original.lifecycle.operation.is_none());
    assert!(original.runtime_update_request().is_none());
    f.store
        .save_sandbox(active_revision, original.clone())
        .await
        .unwrap();
    let (status, body) = json_response(
        &f.router,
        authenticated(Method::GET, "/api/v1/sandboxes/sbx_update", None, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "failed");
    let update = &body["runtime_update"];
    assert_eq!(update["state"], "available");
    assert_eq!(update["update_available"], true);
    assert_eq!(update["can_update"], false);
    assert_eq!(update["preserves_home"], true);
    assert_eq!(update["latest_version"], "runtime-v2");
    assert_eq!(
        update["current_version"],
        original.runtime.as_ref().unwrap().runtime_version
    );
    assert_eq!(update["blocked_reason"], "Runtime updates are blocked by the Sandbox failure. Open Technical details for the reported failure.");
    let (status, body) = json_response(
        &f.router,
        authenticated(
            Method::POST,
            PATH,
            Some(json!({"target_version":"runtime-v2"})),
            Some("failed-sandbox-update"),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "runtime_update_not_available");
    assert_eq!(
        f.store.get_sandbox(&original.id).await.unwrap(),
        Some(original)
    );
}

#[test]
fn runtime_update_blocked_reasons_describe_each_lifecycle_status() {
    use crate::rest::sandbox_runtime_update::runtime_update_lifecycle_reason;
    for (status, description) in [
        (SandboxStatus::Provisioning, "provisioning"),
        (SandboxStatus::Installing, "installing software"),
        (SandboxStatus::Pausing, "pausing"),
        (SandboxStatus::Reviving, "resuming"),
        (SandboxStatus::CredentialRotating, "credential rotation"),
        (SandboxStatus::Rotating, "runtime rotation"),
        (SandboxStatus::Failed, "Sandbox failure"),
        (SandboxStatus::Retiring, "retiring"),
        (SandboxStatus::Retired, "retired"),
    ] {
        let reason = runtime_update_lifecycle_reason(status).unwrap();
        assert!(reason.contains(description), "{reason}");
        assert!(!reason.contains("Wait for the current Sandbox operation"));
    }
    for status in [SandboxStatus::Active, SandboxStatus::Paused] {
        assert_eq!(runtime_update_lifecycle_reason(status), None);
    }
}

struct NeverVerify;

#[async_trait::async_trait]
impl SandboxUnpublishedRecoveryVerifier for NeverVerify {
    async fn verify(
        &self,
        _: &SandboxUnpublishedRecoverySnapshot,
    ) -> Result<SandboxUnpublishedRecoveryProof, SandboxStoreError> {
        panic!("reading runtime guidance must not verify or admit a recovery")
    }
}

fn parked_rotation(phase: SandboxImageRotationPhase) -> Sandbox {
    let mut sandbox = stored_sandbox("sbx_update", "owner", SandboxStatus::Active, 4);
    sandbox.runtime = Some(RuntimeBinding {
        runtime_id: "agent-sandbox:member:sandbox-uid".into(),
        runtime_version: "runtime-v1".into(),
        provision_generation: Some(1),
        boot_id: Some("b".repeat(32)),
        bound_at: NOW.into(),
    });
    sandbox.lifecycle.home = Some(Box::new(retained::reopen::home(&sandbox.id)));
    sandbox.lifecycle.runtime_generation_high_water = 1;
    sandbox.credential.key_generation = 1;
    sandbox.credential.delivered = true;
    sandbox.credential.current = Some(SandboxCredentialAuthority {
        generation: 1,
        provider_key_id: "fixture-credential-reference".into(),
        provider_state_version: 1,
        issued_at: NOW.into(),
        expires_at: "2026-09-06T00:00:00Z".into(),
        grant_fingerprint: "b".repeat(64),
    });
    assert!(sandbox
        .request_runtime_update("runtime-v2".into(), false, NOW.into())
        .unwrap());
    let identity = SandboxRotationIdentity::capture(&sandbox, &[]).unwrap();
    sandbox
        .begin_image_rotation("runtime-v2".into(), identity, NOW.into())
        .unwrap();
    if let Some(SandboxLifecycleOperation::ImageRotation { phase: step, .. }) =
        sandbox.lifecycle.operation.as_mut()
    {
        *step = phase;
    }
    if phase == SandboxImageRotationPhase::ConversationReopen {
        sandbox.runtime.as_mut().unwrap().runtime_version = "runtime-v2".into();
    }
    let holder = "fixture-worker";
    let fence = sandbox
        .work
        .claim(holder.into(), NOW, "2026-09-05T01:00:00Z".into())
        .unwrap();
    sandbox
        .park_failed_runtime_update(
            TypedFailure {
                class: "terminal_validation".into(),
                detail: "Synthetic provider refusal".into(),
            },
            holder,
            fence,
            NOW.into(),
        )
        .unwrap();
    sandbox.revision = 1;
    sandbox.validate().unwrap();
    sandbox
}

#[tokio::test]
async fn parked_update_guidance_matches_available_retry_and_keeps_the_pinned_request() {
    use SandboxImageRotationPhase::{CaptureAndPublishCheckpoint, ConversationReopen};
    for (phase, recovery, expected) in [
        (ConversationReopen, true, "can use the latest runtime"),
        (
            CaptureAndPublishCheckpoint,
            true,
            "with the requested runtime",
        ),
        (ConversationReopen, false, "Retry is unavailable"),
    ] {
        let mut f = fixture(3);
        let sandbox = parked_rotation(phase);
        assert_eq!(
            sandbox.unpublished_recovery_candidate(),
            phase == ConversationReopen
        );
        f.store.create_sandbox(sandbox.clone()).await.unwrap();
        let mut service = SandboxRuntimeUpdateService::new(f.store.clone(), "runtime-v3".into());
        if recovery {
            service = service.with_unpublished_recovery(Arc::new(NeverVerify));
        }
        f.state = f.state.with_runtime_updates(Arc::new(service));
        let (status, body) = json_response(
            &hierarchy_router(f.state),
            authenticated(Method::GET, "/api/v1/sandboxes/sbx_update", None, None),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "rotating");
        let update = &body["runtime_update"];
        assert_eq!(update["state"], "failed");
        assert_eq!(update["can_update"], recovery);
        assert_eq!(update["target_version"], "runtime-v2");
        assert_eq!(update["latest_version"], "runtime-v3");
        assert_eq!(update["preserves_home"], true);
        assert_eq!(update["failure"]["code"], "terminal_validation");
        let message = update["failure"]["message"].as_str().unwrap();
        assert!(message.starts_with("The runtime update stopped at its current step."));
        assert!(message.contains(expected), "{message}");
        assert_eq!(message.contains("Retry the runtime update"), recovery);
        assert!(!message.contains("Synthetic provider refusal"));
        assert_eq!(
            f.store.get_sandbox(&sandbox.id).await.unwrap(),
            Some(sandbox)
        );
    }
}
