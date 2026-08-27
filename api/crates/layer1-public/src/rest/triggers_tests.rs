use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use layer2_core::{
    AuthorityProvenance, Authorization, BillingMode, BranchRecord, Capability,
    CapabilityRevocationState, CheckpointId, CheckpointIntegrity, CommercialPolicy,
    CommitAttributionPolicy, CommitIdentity, Confirmation, ConfirmationAction, ConnectedAccount,
    ConnectionSnapshot, ConnectionState, ContributionId, ContributionPolicy, ContributionRecord,
    CredentialRoute, DelegationState, DiffArtifact, DiffArtifactId, DiffArtifactObjectStore,
    DiffChangeKind, DiffFile, DiffValidationOutcome, EditableField, EffectRecord, EffectState,
    ExecutorPolicy, FailureClass, ForkRecord, InputFieldDefinition, InputFieldKind, Launch,
    LaunchAuthority, LaunchBlockedReason, LaunchEffectKind, LaunchFailure, LaunchId, LaunchQuery,
    LaunchState, LaunchStore, OperationClaim, OperationReservation, OperationStore, OverridePolicy,
    PermissionMode, PromptDelivery, PromptDeliveryState, PublicProjectionPolicy, PublicTriggerId,
    PublicationState, PublishedRevision, PublisherAuthority, PublisherRepositoryRole,
    PullRequestDefaults, ReachStatus, RepositoryAccessPolicy, RepositoryIdentity,
    RepositoryOperationClass, RepositoryVisibility, RequiredCapability, ResourcePolicy, ScopeClass,
    SessionId, SessionLink, SessionSize, SetupPolicy, SourceMode, StoreError, SubjectId,
    TriggerAuthorityVerifier, TriggerConnection, TriggerConnectionAuthorization,
    TriggerConnectionManager, TriggerDraft, TriggerId, TriggerPage, TriggerPageRequest,
    TriggerRepositorySelection, TriggerRevision, TriggerRevisionContent, TriggerRevisionId,
    TriggerService, TriggerServiceError, TriggerSessionPolicy, TriggerStore, TriggerStoreError,
    TriggerTask, UpstreamRevision, WorkerLease, WorkflowChangePolicy, WorkspaceCheckpoint,
    WorkspaceEntry, WorkspaceEntryKind, WorkspaceManifest, WorkspaceRecord, TRIGGER_SCHEMA_VERSION,
};
use serde_json::{json, Value};
use sha2::{Digest as _, Sha256};
use tower::ServiceExt;

use super::*;
use crate::auth::AuthenticatedSubject;

const NOW: &str = "2026-08-26T12:00:00Z";
const OWNER: &str = "contributor";
const PRIVATE_PROMPT: &str = "PRIVATE_TRIGGER_PROMPT {{topic}}";
const TRIGGER_ID: &str = "11111111-1111-4111-8111-111111111111";
const REVISION_ID: &str = "22222222-2222-4222-8222-222222222222";
const PUBLIC_ID: &str = "33333333-3333-4333-8333-333333333333";
const LAUNCH_ID: &str = "44444444-4444-4444-8444-444444444444";
const CHECKPOINT_ID: &str = "55555555-5555-4555-8555-555555555555";
const DIFF_ID: &str = "66666666-6666-4666-8666-666666666666";
const CONTRIBUTION_ID: &str = "77777777-7777-4777-8777-777777777777";
const SDK_TRIGGER_PREVIEW_FIXTURE: &str =
    include_str!("../../tests/fixtures/rest-sdk/trigger_preview.json");
const SDK_REPOSITORY_SELECTION_FIXTURE: &str =
    include_str!("../../tests/fixtures/rest-sdk/repository_selection.json");
const SDK_LAUNCH_FIXTURE: &str = include_str!("../../tests/fixtures/rest-sdk/launch.json");
const SDK_LAUNCH_PROGRESS_FIXTURE: &str =
    include_str!("../../tests/fixtures/rest-sdk/launch_progress.json");
const SDK_DIFF_REVIEW_FIXTURE: &str =
    include_str!("../../tests/fixtures/rest-sdk/diff_review.json");
const SDK_PULL_REQUEST_DRAFT_FIXTURE: &str =
    include_str!("../../tests/fixtures/rest-sdk/pull_request_draft.json");
const SDK_LAUNCH_START_REQUEST_FIXTURE: &str =
    include_str!("../../tests/fixtures/rest-sdk/launch_start_request.json");
const SDK_PULL_REQUEST_REVIEW_REQUEST_FIXTURE: &str =
    include_str!("../../tests/fixtures/rest-sdk/pull_request_review_request.json");
const SDK_PULL_REQUEST_CONFIRM_REQUEST_FIXTURE: &str =
    include_str!("../../tests/fixtures/rest-sdk/pull_request_confirm_request.json");

#[tokio::test]
async fn anonymous_preview_is_side_effect_free_and_prompt_free() {
    let fixture = Fixture::new();
    let public_id = fixture.revision.public_id.0.clone();
    let app = fixture.router();

    let response = app
        .clone()
        .oneshot(
            Request::get(format!("/api/v1/triggers/public/{public_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(header::REFERRER_POLICY).unwrap(),
        "no-referrer"
    );
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let wire = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(wire.contains("Fix one bounded issue"));
    assert!(!wire.contains(PRIVATE_PROMPT));
    assert!(!wire.contains("backing-key"));

    let repeated = app
        .oneshot(
            Request::get(format!("/api/v1/triggers/public/{public_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(repeated.status(), StatusCode::OK);
    assert!(repeated.headers().get(header::SET_COOKIE).is_none());
    assert_eq!(fixture.store.writes.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.operations.writes.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn agui_trigger_companions_share_the_rest_shapes_without_thread_aliases() {
    let mut fixture = Fixture::new();
    fixture.make_ready();
    fixture.install_diff(b"companion review".to_vec());
    let public_id = fixture.revision.public_id.0.clone();
    let launch_id = fixture.launch.id.0.clone();
    let app = crate::agui::trigger::router(fixture.state());

    let preview = app
        .clone()
        .oneshot(
            Request::get(format!("/api/v1/agui/triggers/{public_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(preview.status(), StatusCode::OK);
    let preview = to_bytes(preview.into_body(), usize::MAX).await.unwrap();
    assert!(!String::from_utf8_lossy(&preview).contains(PRIVATE_PROMPT));

    for suffix in ["", "/events?after=0&limit=10", "/diff"] {
        let response = app
            .clone()
            .oneshot(authenticated_request(
                Method::GET,
                &format!("/api/v1/agui/launches/{launch_id}{suffix}"),
                OWNER,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "failed {suffix}");
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let wire = String::from_utf8(body.to_vec()).unwrap();
        assert!(!wire.contains("threadId"));
        assert!(!wire.contains("runId"));
        assert!(!wire.contains("idempotency_key_hash"));
        assert!(!wire.contains("provider_request_id"));
    }

    assert_eq!(fixture.store.writes.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.operations.writes.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn every_mutation_requires_an_authenticated_subject_before_writing() {
    let fixture = Fixture::new();
    let launch_id = fixture.launch.id.0.clone();
    let body = json!({
        "expected_revision": 0,
        "snapshot_digest": fixture.launch.effective.digest,
    });
    let request = Request::builder()
        .method(Method::POST)
        .uri(format!("/api/v1/launches/{launch_id}/start"))
        .header(header::CONTENT_TYPE, "application/json")
        .header("idempotency-key", "unauthenticated-start")
        .body(Body::from(body.to_string()))
        .unwrap();

    let response = fixture.router().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(fixture.store.writes.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.operations.writes.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn repeated_start_replays_the_exact_response_and_advances_once() {
    let fixture = Fixture::new();
    let launch_id = fixture.launch.id.0.clone();
    let body = json!({
        "expected_revision": 0,
        "snapshot_digest": fixture.launch.effective.digest,
    });
    let first = send(
        &fixture.router(),
        Method::POST,
        &format!("/api/v1/launches/{launch_id}/start"),
        body.clone(),
        "one-start",
    )
    .await;
    let replay = send(
        &fixture.router(),
        Method::POST,
        &format!("/api/v1/launches/{launch_id}/start"),
        body,
        "one-start",
    )
    .await;

    assert_eq!(first.0, StatusCode::ACCEPTED);
    assert_eq!(replay.0, StatusCode::ACCEPTED);
    assert_eq!(first.1, replay.1);
    assert_eq!(fixture.store.writes.load(Ordering::SeqCst), 1);
    assert_eq!(
        fixture
            .store
            .launch
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .revision_number,
        1
    );
}

#[tokio::test]
async fn prompt_recovery_is_explicit_payload_bound_and_replays_one_authorization() {
    let mut fixture = Fixture::new();
    fixture.make_prompt_recovery_required();
    let launch_id = fixture.launch.id.0.clone();
    let payload_hash = fixture
        .launch
        .delivery
        .as_ref()
        .unwrap()
        .payload_hash
        .clone();
    let uri = format!("/api/v1/launches/{launch_id}/recover-initial-prompt");

    for (key, snapshot_digest, acknowledged) in [
        (
            "prompt-recovery-unacknowledged",
            payload_hash.clone(),
            false,
        ),
        ("prompt-recovery-wrong-payload", "f".repeat(64), true),
    ] {
        let refused = send(
            &fixture.router(),
            Method::POST,
            &uri,
            json!({
                "expected_revision": fixture.launch.revision_number,
                "snapshot_digest": snapshot_digest,
                "acknowledge_duplicate_execution_risk": acknowledged,
            }),
            key,
        )
        .await;
        assert_eq!(refused.0, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(fixture.store.writes.load(Ordering::SeqCst), 0);
    }

    let body = json!({
        "expected_revision": fixture.launch.revision_number,
        "snapshot_digest": payload_hash,
        "acknowledge_duplicate_execution_risk": true,
    });
    let first = send(
        &fixture.router(),
        Method::POST,
        &uri,
        body.clone(),
        "one-prompt-recovery",
    )
    .await;
    let replay = send(
        &fixture.router(),
        Method::POST,
        &uri,
        body,
        "one-prompt-recovery",
    )
    .await;

    assert_eq!(first.0, StatusCode::ACCEPTED);
    assert_eq!(replay.0, StatusCode::ACCEPTED);
    assert_eq!(first.1, replay.1);
    assert_eq!(fixture.store.writes.load(Ordering::SeqCst), 1);
    let response: Value = serde_json::from_slice(&first.1).unwrap();
    assert_eq!(response["state"], "prompt_pending");
    assert_eq!(response["delivery"]["state"], "recovery_required");
    assert_eq!(response["delivery"]["recovery_generation"], 0);
    let retained = fixture.store.launch.lock().unwrap().clone().unwrap();
    assert_eq!(retained.effects.last().unwrap().state, EffectState::Queued);
    assert_eq!(
        retained
            .confirmations
            .iter()
            .filter(|confirmation| confirmation.action == ConfirmationAction::RecoverPrompt)
            .count(),
        1
    );
    assert!(!String::from_utf8(first.1)
        .unwrap()
        .contains("idempotency_key_hash"));
}

#[tokio::test]
async fn repeated_diff_review_is_idempotent_and_only_records_read_only_work() {
    let mut fixture = Fixture::new();
    fixture.make_ready();
    let launch_id = fixture.launch.id.0.clone();
    let body = json!({
        "expected_revision": fixture.launch.revision_number,
        "snapshot_digest": fixture.launch.effective.digest,
    });
    let first = send(
        &fixture.router(),
        Method::POST,
        &format!("/api/v1/launches/{launch_id}/contributions/review"),
        body.clone(),
        "one-diff-review",
    )
    .await;
    let replay = send(
        &fixture.router(),
        Method::POST,
        &format!("/api/v1/launches/{launch_id}/contributions/review"),
        body,
        "one-diff-review",
    )
    .await;

    assert_eq!(first.0, StatusCode::ACCEPTED);
    assert_eq!(replay.0, StatusCode::ACCEPTED);
    assert_eq!(first.1, replay.1);
    assert_eq!(fixture.store.writes.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.artifacts.reads.load(Ordering::SeqCst), 0);
    let retained = fixture.store.launch.lock().unwrap().clone().unwrap();
    assert_eq!(retained.state, LaunchState::Contributing);
    assert_eq!(retained.contribution_checkpoint_version, Some(1));
    assert!(retained.diff.is_none());
    assert!(retained.confirmations.iter().any(|confirmation| {
        confirmation.action == ConfirmationAction::ReviewDiff
            && confirmation.subject == retained.owner
    }));
}

#[tokio::test]
async fn explicit_branch_publication_is_digest_bound_idempotent_and_projection_safe() {
    let mut fixture = Fixture::new();
    fixture.make_ready();
    let artifact = fixture.install_diff(b"canonical retained review".to_vec());
    let launch_id = fixture.launch.id.0.clone();
    let uri = format!("/api/v1/launches/{launch_id}/contributions/publish");
    let body = json!({
        "expected_revision": fixture.launch.revision_number,
        "snapshot_digest": artifact.digest,
    });

    let first = send(
        &fixture.router(),
        Method::POST,
        &uri,
        body.clone(),
        "one-branch-publication",
    )
    .await;
    let replay = send(
        &fixture.router(),
        Method::POST,
        &uri,
        body,
        "one-branch-publication",
    )
    .await;

    assert_eq!(first.0, StatusCode::ACCEPTED);
    assert_eq!(replay.0, StatusCode::ACCEPTED);
    assert_eq!(first.1, replay.1);
    assert_eq!(fixture.store.writes.load(Ordering::SeqCst), 1);
    let response: Value = serde_json::from_slice(&first.1).unwrap();
    assert_eq!(response["state"], "pr_pending");
    assert_eq!(response["contribution"]["state"], "intent_recorded");
    assert_eq!(
        response["contribution"]["confirmation_snapshot_digest"],
        artifact.digest
    );
    assert!(response["pull_request"].is_null());
    let wire = String::from_utf8(first.1).unwrap();
    for forbidden in [
        "object_reference",
        "baseline_object_reference",
        "backing_key_id",
        "idempotency_key_hash",
        "intent_key",
        "provider_request_id",
    ] {
        assert!(!wire.contains(forbidden), "leaked {forbidden}");
    }

    let retained = fixture.store.launch.lock().unwrap().clone().unwrap();
    assert_eq!(retained.state, LaunchState::PrPending);
    assert_eq!(
        retained.contribution.as_ref().unwrap().state,
        layer2_core::PublicationState::IntentRecorded
    );
    assert!(retained.effects.iter().all(|effect| {
        effect.kind != layer2_core::LaunchEffectKind::BranchPublication
            && effect.kind != layer2_core::LaunchEffectKind::PullRequest
    }));
}

#[tokio::test]
async fn pull_request_review_is_read_only_and_open_confirmation_is_idempotent() {
    let mut fixture = Fixture::new();
    let artifact = fixture.install_published_contribution();
    let launch_id = fixture.launch.id.0.clone();
    let review_uri = format!("/api/v1/launches/{launch_id}/pull-request/review");

    let review = send(
        &fixture.router(),
        Method::POST,
        &review_uri,
        json!({ "operation": "open" }),
        "read-only-pr-review",
    )
    .await;
    assert_eq!(
        review.0,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&review.1)
    );
    assert_eq!(fixture.store.writes.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.operations.writes.load(Ordering::SeqCst), 0);
    let draft: Value = serde_json::from_slice(&review.1).unwrap();
    assert_eq!(draft["operation"], "open");
    assert_eq!(draft["launch_id"], launch_id);
    assert_eq!(draft["expected_head_revision"], "e".repeat(40));
    assert_eq!(draft["base_ref"], "main");
    assert_eq!(draft["head_owner"], "contributor");
    assert_eq!(draft["body_digest"].as_str().unwrap().len(), 64);
    assert_eq!(draft["intent_digest"].as_str().unwrap().len(), 64);

    let confirm_uri = format!("/api/v1/launches/{launch_id}/pull-request/open");
    let body = json!({
        "expected_revision": fixture.launch.revision_number,
        "snapshot_digest": draft["intent_digest"],
    });
    let first = send(
        &fixture.router(),
        Method::POST,
        &confirm_uri,
        body.clone(),
        "one-pr-open",
    )
    .await;
    let replay = send(
        &fixture.router(),
        Method::POST,
        &confirm_uri,
        body,
        "one-pr-open",
    )
    .await;

    assert_eq!(first.0, StatusCode::ACCEPTED);
    assert_eq!(replay.0, StatusCode::ACCEPTED);
    assert_eq!(first.1, replay.1);
    let response: Value = serde_json::from_slice(&first.1).unwrap();
    assert_eq!(response["pull_request"]["state"], "creating");
    assert_eq!(
        response["pull_request"]["confirmation_snapshot_digest"],
        draft["intent_digest"]
    );
    assert_eq!(response["diff"]["digest"], artifact.digest);
    let wire = String::from_utf8(first.1).unwrap();
    assert!(!wire.contains("intent_key"));
    assert!(!wire.contains("idempotency_key_hash"));
    assert_eq!(fixture.store.writes.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn contribution_read_is_owner_only_and_has_no_write_side_effect() {
    let mut fixture = Fixture::new();
    let artifact = fixture.install_published_contribution();
    let uri = format!("/api/v1/launches/{}/diff", fixture.launch.id.0);

    let response = fixture
        .router()
        .oneshot(authenticated_request(Method::GET, &uri, OWNER))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let value: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["artifact"]["digest"], artifact.digest);
    assert_eq!(value["contribution"]["state"], "published");
    assert_eq!(fixture.store.writes.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.operations.writes.load(Ordering::SeqCst), 0);

    let response = fixture
        .router()
        .oneshot(authenticated_request(Method::GET, &uri, "other-owner"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn branch_publication_refuses_wrong_owner_stale_revision_and_changed_review() {
    let mut fixture = Fixture::new();
    fixture.make_ready();
    let artifact = fixture.install_diff(b"canonical retained review".to_vec());
    let uri = format!(
        "/api/v1/launches/{}/contributions/publish",
        fixture.launch.id.0
    );
    let base = json!({
        "expected_revision": fixture.launch.revision_number,
        "snapshot_digest": artifact.digest,
    });

    let wrong_owner = send_as(
        &fixture.router(),
        Method::POST,
        &uri,
        base.clone(),
        "wrong-owner-publication",
        "another-subject",
    )
    .await;
    assert_eq!(wrong_owner.0, StatusCode::NOT_FOUND);

    let stale = send(
        &fixture.router(),
        Method::POST,
        &uri,
        json!({
            "expected_revision": fixture.launch.revision_number + 1,
            "snapshot_digest": artifact.digest,
        }),
        "stale-publication",
    )
    .await;
    assert_eq!(stale.0, StatusCode::CONFLICT);

    let changed = send(
        &fixture.router(),
        Method::POST,
        &uri,
        json!({
            "expected_revision": fixture.launch.revision_number,
            "snapshot_digest": "f".repeat(64),
        }),
        "changed-review-publication",
    )
    .await;
    assert_eq!(changed.0, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(fixture.store.writes.load(Ordering::SeqCst), 0);
    assert!(fixture
        .store
        .launch
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .contribution
        .is_none());
}

#[tokio::test]
async fn diff_download_is_tenant_isolated_integrity_checked_and_hardened() {
    let mut fixture = Fixture::new();
    let bytes = br#"{"cma_trigger_diff":1,"safe":"review"}"#.to_vec();
    let artifact = fixture.install_diff(bytes.clone());
    let uri = format!(
        "/api/v1/launches/{}/diff-artifacts/{}/download",
        fixture.launch.id.0, artifact.id.0
    );

    let response = fixture
        .router()
        .oneshot(authenticated_request(Method::GET, &uri, OWNER))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "application/octet-stream"
    );
    assert_eq!(response.headers()["x-content-type-options"], "nosniff");
    assert_eq!(
        response.headers()["content-security-policy"],
        "sandbox; default-src 'none'"
    );
    assert_eq!(
        response.headers()[header::CACHE_CONTROL],
        "private, no-store"
    );
    assert_eq!(response.headers()[header::REFERRER_POLICY], "no-referrer");
    let disposition = response.headers()[header::CONTENT_DISPOSITION]
        .to_str()
        .unwrap();
    assert_eq!(
        disposition,
        format!(
            "attachment; filename=\"cma-trigger-diff-{}.json\"",
            artifact.id.0
        )
    );
    assert!(!disposition.contains("project"));
    assert_eq!(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
        bytes
    );
    assert_eq!(fixture.artifacts.reads.load(Ordering::SeqCst), 1);

    let projection = serde_json::to_value(LaunchView::from(fixture.launch.clone())).unwrap();
    assert_eq!(
        projection["diff"]["download_url"],
        format!(
            "/api/v1/launches/{}/diff-artifacts/{}/download",
            fixture.launch.id.0, artifact.id.0
        )
    );
    assert!(projection["diff"].get("object_reference").is_none());

    let other = fixture
        .router()
        .oneshot(authenticated_request(Method::GET, &uri, "other-subject"))
        .await
        .unwrap();
    assert_eq!(other.status(), StatusCode::NOT_FOUND);
    assert_eq!(fixture.artifacts.reads.load(Ordering::SeqCst), 1);

    let wrong = format!(
        "/api/v1/launches/{}/diff-artifacts/{}/download",
        fixture.launch.id.0,
        DiffArtifactId::new().0
    );
    let wrong = fixture
        .router()
        .oneshot(authenticated_request(Method::GET, &wrong, OWNER))
        .await
        .unwrap();
    assert_eq!(wrong.status(), StatusCode::NOT_FOUND);
    assert_eq!(fixture.artifacts.reads.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn diff_download_refuses_bytes_that_do_not_match_the_launch_digest() {
    let mut fixture = Fixture::new();
    let artifact = fixture.install_diff(b"expected artifact".to_vec());
    fixture
        .artifacts
        .replace_bytes(b"tampered artifact".to_vec());
    let uri = format!(
        "/api/v1/launches/{}/diff-artifacts/{}/download",
        fixture.launch.id.0, artifact.id.0
    );

    let response = fixture
        .router()
        .oneshot(authenticated_request(Method::GET, &uri, OWNER))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert!(!body
        .windows("tampered artifact".len())
        .any(|window| { window == b"tampered artifact" }));
}

#[test]
fn unknown_fields_in_nested_trigger_policy_are_refused() {
    let mut body = serde_json::to_value(content(SubjectId(OWNER.into()))).unwrap();
    body["task"]["unexpected_secret_switch"] = Value::Bool(true);
    let error = serde_json::from_value::<TriggerRevisionContent>(body).unwrap_err();
    assert!(error.to_string().contains("unknown field"));
}

#[tokio::test]
async fn publisher_repository_locator_is_authenticated_canonical_and_read_only() {
    let fixture = Fixture::new();
    let uri = "/api/v1/trigger-connections/service-1/repositories/resolve?owner=upstream-owner&name=project";
    let anonymous = fixture
        .router()
        .oneshot(Request::get(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);

    let response = fixture
        .router()
        .oneshot(authenticated_request(Method::GET, uri, OWNER))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let value: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["repository"]["repository_id"], 10);
    assert_eq!(value["repository"]["repository_node_id"], "R_upstream");
    assert_eq!(value["repository"]["selected_ref"], "main");
    assert_eq!(
        value["source_sha"],
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    );
    assert!(!String::from_utf8_lossy(&body).contains("backing-key"));
    assert_eq!(fixture.store.writes.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.operations.writes.load(Ordering::SeqCst), 0);
}

#[test]
fn trigger_openapi_catalogue_is_complete_concrete_and_only_preview_is_anonymous() {
    let fixture = Fixture::new();
    let documented = documented_router(fixture.state());
    let openapi = documented.get_openapi();
    let paths = &openapi.paths.paths;
    let expected = [
        ("get", "/api/v1/triggers/public/{public_id}"),
        ("get", "/api/v1/triggers"),
        ("post", "/api/v1/triggers"),
        ("get", "/api/v1/triggers/{trigger_id}"),
        ("put", "/api/v1/triggers/{trigger_id}"),
        ("post", "/api/v1/triggers/{trigger_id}/publish"),
        ("get", "/api/v1/trigger-revisions"),
        ("get", "/api/v1/trigger-revisions/{revision_id}"),
        ("post", "/api/v1/trigger-revisions/{revision_id}/retire"),
        ("get", "/api/v1/trigger-connections"),
        ("post", "/api/v1/trigger-connections"),
        ("post", "/api/v1/trigger-connections/{service_id}/reconnect"),
        ("get", "/api/v1/trigger-connections/{service_id}/preflight"),
        (
            "get",
            "/api/v1/trigger-connections/{service_id}/repositories/resolve",
        ),
        ("post", "/api/v1/triggers/public/{public_id}/launches"),
        ("post", "/api/v1/triggers/public/{public_id}/launches/new"),
        ("get", "/api/v1/launches"),
        ("get", "/api/v1/launches/{launch_id}"),
        ("patch", "/api/v1/launches/{launch_id}/overrides"),
        ("put", "/api/v1/launches/{launch_id}/connection"),
        ("post", "/api/v1/launches/{launch_id}/start"),
        ("post", "/api/v1/launches/{launch_id}/contributions/review"),
        ("get", "/api/v1/launches/{launch_id}/diff"),
        ("post", "/api/v1/launches/{launch_id}/contributions/publish"),
        ("post", "/api/v1/launches/{launch_id}/pull-request/review"),
        ("post", "/api/v1/launches/{launch_id}/pull-request/open"),
        ("post", "/api/v1/launches/{launch_id}/pull-request/update"),
        (
            "get",
            "/api/v1/launches/{launch_id}/diff-artifacts/{artifact_id}/download",
        ),
        (
            "post",
            "/api/v1/launches/{launch_id}/recover-initial-prompt",
        ),
        ("post", "/api/v1/launches/{launch_id}/cancel"),
        ("get", "/api/v1/launches/{launch_id}/events"),
    ];

    for (method, path) in expected {
        let item = paths
            .get(path)
            .unwrap_or_else(|| panic!("missing Trigger path {path}"));
        let present = match method {
            "get" => item.get.is_some(),
            "post" => item.post.is_some(),
            "put" => item.put.is_some(),
            "patch" => item.patch.is_some(),
            _ => unreachable!("the Trigger catalogue uses a known method"),
        };
        assert!(present, "missing {method} operation for {path}");
    }

    let document = serde_json::to_value(openapi).expect("OpenAPI serializes");
    let schemas = document["components"]["schemas"]
        .as_object()
        .expect("Trigger OpenAPI has component schemas");
    for name in [
        "TriggerTask",
        "TriggerSessionPolicy",
        "RepositoryAccessPolicy",
        "SetupPolicy",
        "ContributionPolicy",
        "CommercialPolicy",
        "OverridePolicy",
        "PublicProjectionPolicy",
        "TriggerRevisionContent",
        "LaunchOverrides",
        "EffectiveConfiguration",
        "LaunchBlockedReason",
        "LaunchFailure",
        "PricingSnapshot",
        "ForkRecord",
        "BranchRecord",
        "SessionLink",
        "ReachStatus",
        "PromptDelivery",
        "DiffFile",
        "CheckResult",
        "CommitIdentity",
        "WorkspaceManifest",
    ] {
        assert!(schemas.contains_key(name), "missing concrete schema {name}");
    }
    for (owner, field, target) in [
        ("TriggerContentRequest", "task", "TriggerTask"),
        (
            "TriggerContentRequest",
            "repository_access",
            "RepositoryAccessPolicy",
        ),
        ("TriggerDraftView", "content", "TriggerRevisionContent"),
        ("LaunchView", "revision_content", "TriggerRevisionContent"),
        ("LaunchView", "overrides", "LaunchOverrides"),
        ("LaunchView", "effective", "EffectiveConfiguration"),
        ("WorkspaceRecordView", "baseline", "WorkspaceManifest"),
    ] {
        assert_eq!(
            schemas[owner]["properties"][field]["$ref"],
            format!("#/components/schemas/{target}"),
            "{owner}.{field} must reference a concrete schema"
        );
    }
    assert_eq!(
        schemas["DiffArtifactView"]["properties"]["files"]["items"]["$ref"],
        "#/components/schemas/DiffFile"
    );
    for (field, target) in [
        ("blocked_reason", "LaunchBlockedReason"),
        ("failure", "LaunchFailure"),
        ("pricing", "PricingSnapshot"),
        ("fork", "ForkRecord"),
        ("branch", "BranchRecord"),
        ("session", "SessionLink"),
        ("reach", "ReachStatus"),
        ("delivery", "PromptDelivery"),
    ] {
        let schema = &schemas["LaunchView"]["properties"][field];
        let wire = schema.to_string();
        assert!(
            wire.contains(&format!("#/components/schemas/{target}")),
            "LaunchView.{field} lost its concrete {target} schema: {schema}"
        );
        assert!(
            wire.contains("null"),
            "LaunchView.{field} must admit the null value emitted before the resource exists: {schema}"
        );
    }

    for path in paths.keys() {
        assert_eq!(
            crate::rest::openapi::anonymous_path(path),
            path == "/api/v1/triggers/public/{public_id}",
            "unexpected anonymous contract at {path}"
        );
    }
}

#[test]
fn launch_projection_excludes_credential_and_worker_internals() {
    let mut fixture = Fixture::new();
    fixture.make_ready();
    fixture.install_diff(b"retained review".to_vec());
    let wire = serde_json::to_string(&LaunchView::from(fixture.launch.clone())).unwrap();
    for forbidden in [
        "backing_key_id",
        "worker_fencing_token",
        "fencing_counter",
        "subject_token_generation",
        "session_credential_generation",
        "idempotency_key_hash",
        "provider_request_id",
        "object_reference",
        "baseline_object_reference",
    ] {
        assert!(!wire.contains(forbidden), "leaked {forbidden}");
    }
}

#[tokio::test]
async fn progress_projection_excludes_worker_and_credential_internals() {
    let fixture = Fixture::new();
    let mut retained = fixture.store.launch.lock().unwrap().clone().unwrap();
    let sequence = retained.events.last().map_or(1, |event| event.sequence + 1);
    retained.events.push(LaunchEvent {
        sequence,
        launch_id: retained.id.clone(),
        occurred_at: NOW.into(),
        correlation_id: "progress-projection".into(),
        kind: layer2_core::LaunchEventKind::StepChanged {
            step: retained.steps[0].clone(),
        },
    });
    *fixture.store.launch.lock().unwrap() = Some(retained);
    let view = load_launch_progress(
        &fixture.state(),
        &SubjectId(OWNER.into()),
        fixture.launch.id.0.clone(),
        0,
        100,
    )
    .await
    .unwrap();
    let wire = serde_json::to_string(&view).unwrap();
    assert!(wire.contains("step_changed"));
    for forbidden in [
        "worker_fencing_token",
        "backing_key_id",
        "subject_token_generation",
        "idempotency_key_hash",
        "provider_request_id",
        "object_reference",
    ] {
        assert!(!wire.contains(forbidden), "leaked {forbidden}");
    }
}

async fn trigger_sdk_fixture_values() -> Vec<(&'static str, Value)> {
    let draft_fixture = Fixture::new();
    let mut fixture = Fixture::new();
    fixture.install_published_contribution();
    let mut events = fixture.launch.events.clone();
    events.push(LaunchEvent {
        sequence: events.last().map_or(1, |event| event.sequence + 1),
        launch_id: fixture.launch.id.clone(),
        occurred_at: NOW.into(),
        correlation_id: "fixture-progress".into(),
        kind: layer2_core::LaunchEventKind::StepChanged {
            step: fixture.launch.steps[0].clone(),
        },
    });
    let draft = fixture
        .launch
        .pull_request_draft(PullRequestOperation::Open, None, None)
        .unwrap();
    vec![
        (
            "trigger_preview.json",
            serde_json::to_value(
                load_public_preview(&fixture.state(), fixture.revision.public_id.0.clone())
                    .await
                    .unwrap(),
            )
            .unwrap(),
        ),
        (
            "repository_selection.json",
            serde_json::to_value(TriggerRepositorySelectionView::from(
                TriggerRepositorySelection {
                    repository: upstream(),
                    source_sha: "a".repeat(40),
                },
            ))
            .unwrap(),
        ),
        (
            "launch.json",
            serde_json::to_value(LaunchView::from(draft_fixture.launch.clone())).unwrap(),
        ),
        (
            "launch_progress.json",
            serde_json::to_value(LaunchProgressView {
                launch_id: fixture.launch.id.0.clone(),
                next_cursor: events.last().unwrap().sequence,
                events: events.into_iter().map(Into::into).collect(),
            })
            .unwrap(),
        ),
        (
            "diff_review.json",
            serde_json::to_value(DiffReviewView::from_launch(fixture.launch.clone())).unwrap(),
        ),
        (
            "pull_request_draft.json",
            serde_json::to_value(PullRequestDraftView::from(draft)).unwrap(),
        ),
        (
            "launch_start_request.json",
            json!({
                "expected_revision": fixture.launch.revision_number,
                "snapshot_digest": fixture.launch.effective.digest,
                "acknowledge_workflow_risk": false
            }),
        ),
        (
            "pull_request_review_request.json",
            json!({ "operation": "open" }),
        ),
        (
            "pull_request_confirm_request.json",
            json!({
                "expected_revision": fixture.launch.revision_number,
                "snapshot_digest": fixture
                    .launch
                    .pull_request_draft(PullRequestOperation::Open, None, None)
                    .unwrap()
                    .intent_digest
            }),
        ),
    ]
}

#[tokio::test]
async fn trigger_sdk_fixtures_are_produced_and_parsed_by_server_types() {
    let expected = [
        ("trigger_preview.json", SDK_TRIGGER_PREVIEW_FIXTURE),
        (
            "repository_selection.json",
            SDK_REPOSITORY_SELECTION_FIXTURE,
        ),
        ("launch.json", SDK_LAUNCH_FIXTURE),
        ("launch_progress.json", SDK_LAUNCH_PROGRESS_FIXTURE),
        ("diff_review.json", SDK_DIFF_REVIEW_FIXTURE),
        ("pull_request_draft.json", SDK_PULL_REQUEST_DRAFT_FIXTURE),
        (
            "launch_start_request.json",
            SDK_LAUNCH_START_REQUEST_FIXTURE,
        ),
        (
            "pull_request_review_request.json",
            SDK_PULL_REQUEST_REVIEW_REQUEST_FIXTURE,
        ),
        (
            "pull_request_confirm_request.json",
            SDK_PULL_REQUEST_CONFIRM_REQUEST_FIXTURE,
        ),
    ];
    for ((name, produced), (expected_name, source)) in
        trigger_sdk_fixture_values().await.into_iter().zip(expected)
    {
        assert_eq!(name, expected_name);
        assert_eq!(
            produced,
            serde_json::from_str::<Value>(source).unwrap(),
            "{name}"
        );
    }

    let start: StartLaunchRequest = serde_json::from_str(SDK_LAUNCH_START_REQUEST_FIXTURE).unwrap();
    assert_eq!(start.expected_revision, 0);
    assert!(!start.acknowledge_workflow_risk);
    let review: PullRequestReviewRequest =
        serde_json::from_str(SDK_PULL_REQUEST_REVIEW_REQUEST_FIXTURE).unwrap();
    assert!(matches!(
        review.operation,
        PullRequestOperationRequest::Open
    ));
    let confirm: ConfirmPullRequestRequest =
        serde_json::from_str(SDK_PULL_REQUEST_CONFIRM_REQUEST_FIXTURE).unwrap();
    assert_eq!(confirm.expected_revision, 0);
}

#[tokio::test]
#[ignore = "developer aid for refreshing the checked-in SDK corpus"]
async fn dump_trigger_sdk_fixtures() {
    for (name, value) in trigger_sdk_fixture_values().await {
        println!(
            "\n=== {name} ===\n{}",
            serde_json::to_string_pretty(&value).unwrap()
        );
    }
}

#[tokio::test]
async fn authority_failures_keep_auth_drift_rate_limit_and_outage_distinct() {
    use layer2_core::TriggerAuthorityError as Authority;

    for (error, status, code) in [
        (
            Authority::OwnerCredentialUnavailable,
            StatusCode::UNAUTHORIZED,
            "trigger_owner_credential_unavailable",
        ),
        (
            Authority::ConnectionInactive,
            StatusCode::CONFLICT,
            "trigger_connection_inactive",
        ),
        (
            Authority::AuthorityDrift,
            StatusCode::CONFLICT,
            "trigger_authority_drift",
        ),
        (
            Authority::Transient,
            StatusCode::SERVICE_UNAVAILABLE,
            "trigger_provider_unavailable",
        ),
        (
            Authority::MalformedEvidence,
            StatusCode::BAD_GATEWAY,
            "trigger_provider_response_invalid",
        ),
        (
            Authority::UpstreamNotForkable,
            StatusCode::UNPROCESSABLE_ENTITY,
            "upstream_not_forkable",
        ),
        (
            Authority::ForkNetworkOccupied,
            StatusCode::UNPROCESSABLE_ENTITY,
            "fork_network_occupied",
        ),
    ] {
        let response = TriggerProblem(TriggerServiceError::Authority(error)).into_response();
        assert_eq!(response.status(), status);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap()["code"],
            code
        );
    }

    let response = TriggerProblem(TriggerServiceError::Authority(Authority::RateLimited {
        retry_after: Some("17".into()),
    }))
    .into_response();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(response.headers().get(header::RETRY_AFTER).unwrap(), "17");
}

#[tokio::test]
async fn rollout_denials_are_typed_without_exposing_policy_configuration() {
    use layer2_core::TriggerRolloutDenial;

    for denial in [
        TriggerRolloutDenial::EnvironmentDisabled,
        TriggerRolloutDenial::OrchestrationPaused,
        TriggerRolloutDenial::PublisherNotAllowlisted,
        TriggerRolloutDenial::RepositoryNotAllowlisted,
        TriggerRolloutDenial::UserOutsideCohort,
    ] {
        let expected_reason = denial.code();
        let response = TriggerProblem(TriggerServiceError::Rollout(denial)).into_response();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body = serde_json::from_slice::<Value>(&body).unwrap();
        assert_eq!(body["code"], "trigger_feature_paused");
        assert_eq!(body["rollout_reason"], expected_reason);
        assert!(!body.to_string().contains("allowlist_values"));
    }
}

async fn send(
    app: &axum::Router,
    method: Method,
    uri: &str,
    body: Value,
    idempotency_key: &str,
) -> (StatusCode, Vec<u8>) {
    send_as(app, method, uri, body, idempotency_key, OWNER).await
}

async fn send_as(
    app: &axum::Router,
    method: Method,
    uri: &str,
    body: Value,
    idempotency_key: &str,
    subject: &str,
) -> (StatusCode, Vec<u8>) {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .header("idempotency-key", idempotency_key)
        .body(Body::from(body.to_string()))
        .unwrap();
    request
        .extensions_mut()
        .insert(AuthenticatedSubject(SubjectId(subject.into())));
    request
        .extensions_mut()
        .insert(Authorization::from_capabilities([
            Capability::CreateSession,
        ]));
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec();
    (status, body)
}

fn authenticated_request(method: Method, uri: &str, subject: &str) -> Request<Body> {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .body(Body::empty())
        .unwrap();
    request
        .extensions_mut()
        .insert(AuthenticatedSubject(SubjectId(subject.into())));
    request
        .extensions_mut()
        .insert(Authorization::from_capabilities([
            Capability::CreateSession,
        ]));
    request
}

struct Fixture {
    revision: TriggerRevision,
    launch: Launch,
    store: Arc<FixtureStore>,
    operations: Arc<MemoryOperations>,
    artifacts: Arc<MemoryDiffArtifacts>,
}

impl Fixture {
    fn new() -> Self {
        let owner = SubjectId(OWNER.into());
        let revision = revision(SubjectId("publisher".into()));
        let mut launch = Launch::new(owner, revision.clone(), NOW, "create").unwrap();
        launch.id = LaunchId(LAUNCH_ID.into());
        for event in &mut launch.events {
            event.launch_id = launch.id.clone();
        }
        launch
            .select_connection(connection(), NOW, "connection")
            .unwrap();
        launch.authority = Some(provenance());
        let store = Arc::new(FixtureStore {
            revision: revision.clone(),
            launch: Mutex::new(Some(launch.clone())),
            writes: AtomicUsize::new(0),
        });
        Self {
            revision,
            launch,
            store,
            operations: Arc::new(MemoryOperations::default()),
            artifacts: Arc::new(MemoryDiffArtifacts::default()),
        }
    }

    fn make_ready(&mut self) {
        let source_sha = self.launch.source_sha.clone();
        let upstream_id = self
            .launch
            .revision
            .content
            .upstream
            .repository
            .repository_id;
        let account = self.launch.connection.as_ref().unwrap().account.clone();
        let mut fork_repository = self.launch.revision.content.upstream.repository.clone();
        fork_repository.repository_id = 11;
        fork_repository.repository_node_id = "R_contributor_fork".into();
        fork_repository.owner_id = account.account_id;
        fork_repository.owner_node_id = account.account_node_id;
        fork_repository.canonical_owner = account.login;
        fork_repository.fork_parent_id = Some(upstream_id);
        fork_repository.fork_source_id = Some(upstream_id);
        self.launch.fork = Some(ForkRecord {
            repository: fork_repository,
            reused: false,
            source_revision: source_sha.clone(),
            canonical_url: "https://github.com/contributor/project".into(),
            ready_at: NOW.into(),
        });
        self.launch.branch = Some(BranchRecord {
            name: format!("cma/{}", self.launch.id.0),
            expected_head: source_sha.clone(),
            current_head: source_sha.clone(),
            canonical_url: "https://github.com/contributor/project/tree/cma/test".into(),
            created_at: NOW.into(),
        });
        self.launch.workspace = Some(WorkspaceRecord {
            baseline: WorkspaceManifest::seal(source_sha, Vec::new()).unwrap(),
            baseline_object_reference: "internal/baseline.pack".into(),
            baseline_archive_sha256: "a".repeat(64),
            latest_checkpoint: None,
            previous_checkpoint: None,
            workspace_epoch: 1,
            retention_policy_id: "test-retention-v1".into(),
            delete_after: "2026-09-26T12:00:00Z".into(),
        });
        self.launch.session = Some(SessionLink {
            session_id: SessionId("sesn_rest_ready".into()),
            environment_profile_id: "envp_cma_trigger".into(),
            runtime_generation: 1,
            created_at: NOW.into(),
        });
        self.launch.reach = Some(ReachStatus {
            service_id: "service-1".into(),
            state: CapabilityRevocationState::Active,
            policy_generation: 1,
            policy_fence: 1,
            runtime_audience: "runtime-rest-ready".into(),
            expires_at_epoch_seconds: 4_000_000_000,
            numeric_binding_observed_at: NOW.into(),
            refreshed_at: NOW.into(),
            revocation_requested_at: None,
            revocation_effective_at: None,
            last_refusal: None,
            tombstone_acknowledged: false,
            agent_key_revocation_confirmed: false,
        });
        self.launch.delivery = Some(PromptDelivery {
            launch_id: self.launch.id.clone(),
            session_id: self.launch.session.as_ref().unwrap().session_id.clone(),
            revision_digest: self.launch.revision_digest.clone(),
            origin_key: format!("cma-trigger:{}:initial", self.launch.id.0),
            payload_hash: "c".repeat(64),
            state: PromptDeliveryState::Completed,
            attempt: 1,
            recovery_generation: 0,
            dispatch_fence_sequence: Some(1),
            queued_at: NOW.into(),
            updated_at: NOW.into(),
        });
        self.launch.state = LaunchState::Ready;
        *self.store.launch.lock().unwrap() = Some(self.launch.clone());
    }

    fn make_prompt_recovery_required(&mut self) {
        self.make_ready();
        let failure = LaunchFailure {
            class: FailureClass::RecoveryRequired,
            code: "effect_outcome_unknown".into(),
            detail: "CMA cannot prove the external outcome.".into(),
            remediation: "Review retained evidence before authorizing recovery.".into(),
            correlation_id: "prompt-ambiguity".into(),
            retry_at: None,
            observed_at: NOW.into(),
        };
        self.launch.state = LaunchState::FailedRetryable;
        self.launch.blocked_reason = Some(LaunchBlockedReason::RecoveryRequired);
        self.launch.resume_state = Some(LaunchState::PromptPending);
        self.launch.failure = Some(failure.clone());
        self.launch.delivery.as_mut().unwrap().state = PromptDeliveryState::RecoveryRequired;
        self.launch.effects.push(EffectRecord {
            kind: LaunchEffectKind::InitialDispatch,
            state: EffectState::RecoveryRequired,
            intent_key: format!("cma-trigger:{}:initial-dispatch", self.launch.id.0),
            attempt: 1,
            reconciliation_attempt: 0,
            fence: 1,
            provider_request_id: None,
            external_resource_ids: Default::default(),
            next_attempt_at: None,
            last_failure: Some(failure),
            updated_at: NOW.into(),
        });
        *self.store.launch.lock().unwrap() = Some(self.launch.clone());
    }

    fn install_diff(&mut self, bytes: Vec<u8>) -> DiffArtifact {
        let source_sha = self.launch.source_sha.clone();
        let content = b"reviewed change\n";
        let content_sha256 = format!("{:x}", Sha256::digest(content));
        let baseline = WorkspaceManifest::seal(source_sha.clone(), Vec::new()).unwrap();
        let checkpoint_manifest = WorkspaceManifest::seal(
            source_sha.clone(),
            vec![WorkspaceEntry {
                path: "README.md".into(),
                kind: WorkspaceEntryKind::Regular,
                mode: 0o100644,
                size: content.len() as u64,
                content_sha256: content_sha256.clone(),
                provider_object_id: None,
                symlink_target: None,
            }],
        )
        .unwrap();
        let checkpoint = WorkspaceCheckpoint {
            id: CheckpointId(CHECKPOINT_ID.into()),
            launch_id: self.launch.id.clone(),
            version: 1,
            workspace_epoch: 1,
            session_id: SessionId("sesn_rest_diff".into()),
            runtime_generation: 1,
            object_reference: "internal/checkpoint.pack".into(),
            archive_sha256: "b".repeat(64),
            manifest: checkpoint_manifest,
            integrity: CheckpointIntegrity::Complete,
            created_at: NOW.into(),
            delete_after: Some("2026-09-26T12:00:00Z".into()),
        };
        self.launch.branch = Some(BranchRecord {
            name: format!("cma/{}", self.launch.id.0),
            expected_head: source_sha.clone(),
            current_head: source_sha.clone(),
            canonical_url: "https://github.com/contributor/project/tree/cma/test".into(),
            created_at: NOW.into(),
        });
        self.launch.workspace = Some(WorkspaceRecord {
            baseline: baseline.clone(),
            baseline_object_reference: "internal/project-baseline.pack".into(),
            baseline_archive_sha256: "a".repeat(64),
            latest_checkpoint: Some(checkpoint.clone()),
            previous_checkpoint: None,
            workspace_epoch: 1,
            retention_policy_id: "test-retention-v1".into(),
            delete_after: "2026-09-26T12:00:00Z".into(),
        });
        self.launch.contribution_checkpoint_version = Some(1);
        self.launch.state = LaunchState::Contributing;
        let object_sha256 = format!("{:x}", Sha256::digest(&bytes));
        let artifact = DiffArtifact {
            id: DiffArtifactId(DIFF_ID.into()),
            launch_id: self.launch.id.clone(),
            checkpoint_id: checkpoint.id,
            baseline_digest: baseline.digest,
            checkpoint_digest: checkpoint.manifest.digest,
            expected_branch_head: source_sha.clone(),
            observed_branch_head: source_sha,
            changed_file_limit: 1_000,
            total_changed_files: 1,
            files: vec![DiffFile {
                path: "README.md".into(),
                change: DiffChangeKind::Added,
                binary: false,
                old_mode: None,
                new_mode: Some(0o100644),
                old_sha256: None,
                new_sha256: Some(content_sha256),
                old_provider_object_id: None,
                new_provider_object_id: None,
                old_bytes: 0,
                new_bytes: content.len() as u64,
                patch: Some(
                    "--- baseline\n+++ checkpoint\n@@ -0,0 +1 @@\n+reviewed change\n".into(),
                ),
                patch_complete: true,
            }],
            changed_bytes: content.len() as u64,
            digest: object_sha256.clone(),
            object_reference: "internal/project-diff.json".into(),
            object_sha256,
            object_bytes: bytes.len() as u64,
            author: CommitIdentity {
                name: "contributor".into(),
                email: "21+contributor@users.noreply.github.com".into(),
            },
            committer: CommitIdentity {
                name: "CMA".into(),
                email: "cma@chrono.ai".into(),
            },
            workflow_paths: Vec::new(),
            validation: DiffValidationOutcome::Valid,
            checks: Vec::new(),
            created_at: NOW.into(),
            delete_after: "2026-09-26T12:00:00Z".into(),
        };
        artifact.validate_for_review(&self.launch).unwrap();
        self.launch.diff = Some(artifact.clone());
        *self.store.launch.lock().unwrap() = Some(self.launch.clone());
        self.artifacts
            .install(self.launch.owner.clone(), artifact.clone(), bytes);
        artifact
    }

    fn install_published_contribution(&mut self) -> DiffArtifact {
        self.make_ready();
        let artifact = self.install_diff(b"canonical retained review".to_vec());
        let contribution = ContributionRecord {
            id: ContributionId(CONTRIBUTION_ID.into()),
            launch_id: self.launch.id.clone(),
            diff_artifact_id: artifact.id.clone(),
            revision_digest: self.launch.revision_digest.clone(),
            baseline_digest: artifact.baseline_digest.clone(),
            checkpoint_digest: artifact.checkpoint_digest.clone(),
            diff_digest: artifact.digest.clone(),
            branch_name: self.launch.branch.as_ref().unwrap().name.clone(),
            state: PublicationState::Published,
            expected_head: artifact.expected_branch_head.clone(),
            base_tree_id: Some("c".repeat(40)),
            tree_id: Some("d".repeat(40)),
            commit_id: Some("e".repeat(40)),
            resulting_head: Some("e".repeat(40)),
            object_ids: vec!["d".repeat(40), "e".repeat(40)],
            author: artifact.author.clone(),
            committer: artifact.committer.clone(),
            confirmation: Confirmation {
                action: ConfirmationAction::PublishBranch,
                subject: self.launch.owner.clone(),
                snapshot_digest: artifact.digest.clone(),
                confirmed_at: NOW.into(),
                idempotency_key_hash: "test-publication-confirmation".into(),
            },
            intent_key: "test-publication-intent".into(),
            created_at: NOW.into(),
            updated_at: NOW.into(),
        };
        contribution.validate_intent(&self.launch.id).unwrap();
        self.launch.branch.as_mut().unwrap().current_head = "e".repeat(40);
        self.launch.contribution = Some(contribution);
        self.launch.state = LaunchState::PrPending;
        *self.store.launch.lock().unwrap() = Some(self.launch.clone());
        artifact
    }

    fn state(&self) -> TriggersState {
        TriggersState {
            triggers: Arc::new(TriggerService::new(
                self.store.clone(),
                self.store.clone(),
                Arc::new(StaticAuthority),
            )),
            connections: Arc::new(StaticConnections),
            operations: self.operations.clone(),
            diff_artifacts: Some(self.artifacts.clone()),
        }
    }

    fn router(&self) -> axum::Router {
        documented_router(self.state()).into()
    }
}

#[derive(Default)]
struct MemoryDiffArtifacts {
    retained: Mutex<Option<(SubjectId, DiffArtifact, Vec<u8>)>>,
    reads: AtomicUsize,
}

impl MemoryDiffArtifacts {
    fn install(&self, owner: SubjectId, artifact: DiffArtifact, bytes: Vec<u8>) {
        *self.retained.lock().unwrap() = Some((owner, artifact, bytes));
    }

    fn replace_bytes(&self, bytes: Vec<u8>) {
        let mut retained = self.retained.lock().unwrap();
        retained.as_mut().unwrap().2 = bytes;
    }
}

#[async_trait]
impl DiffArtifactObjectStore for MemoryDiffArtifacts {
    async fn put_diff_artifact(
        &self,
        _owner: &SubjectId,
        _launch: &LaunchId,
        _artifact: &DiffArtifactId,
        _sha256: &str,
        _bytes: &[u8],
    ) -> Result<String, TriggerStoreError> {
        Err(TriggerStoreError::Unavailable(
            "test artifact writes are not configured".into(),
        ))
    }

    async fn get_diff_artifact(
        &self,
        owner: &SubjectId,
        artifact: &DiffArtifact,
    ) -> Result<Option<Vec<u8>>, TriggerStoreError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        Ok(self
            .retained
            .lock()
            .unwrap()
            .as_ref()
            .filter(|(retained_owner, retained_artifact, _)| {
                retained_owner == owner && retained_artifact == artifact
            })
            .map(|(_, _, bytes)| bytes.clone()))
    }

    async fn delete_diff_artifact(
        &self,
        _owner: &SubjectId,
        _artifact: &DiffArtifact,
    ) -> Result<(), TriggerStoreError> {
        Ok(())
    }
}

struct FixtureStore {
    revision: TriggerRevision,
    launch: Mutex<Option<Launch>>,
    writes: AtomicUsize,
}

#[async_trait]
impl TriggerStore for FixtureStore {
    async fn insert_draft(&self, _draft: &TriggerDraft) -> Result<(), TriggerStoreError> {
        self.write_unimplemented()
    }

    async fn get_draft(&self, _id: &TriggerId) -> Result<Option<TriggerDraft>, TriggerStoreError> {
        Ok(None)
    }

    async fn list_drafts_for(
        &self,
        _owner: &SubjectId,
        _page: &TriggerPageRequest,
    ) -> Result<TriggerPage<TriggerDraft>, TriggerStoreError> {
        Ok(TriggerPage {
            items: Vec::new(),
            next: None,
        })
    }

    async fn replace_draft(
        &self,
        _expected_version: u64,
        _draft: &TriggerDraft,
    ) -> Result<TriggerDraft, TriggerStoreError> {
        self.write_unimplemented()
    }

    async fn publish_revision(
        &self,
        _expected_draft_version: u64,
        _draft: &TriggerDraft,
        _revision: &TriggerRevision,
    ) -> Result<PublishedRevision, TriggerStoreError> {
        self.write_unimplemented()
    }

    async fn get_revision(
        &self,
        id: &TriggerRevisionId,
    ) -> Result<Option<TriggerRevision>, TriggerStoreError> {
        Ok((id == &self.revision.id).then(|| self.revision.clone()))
    }

    async fn get_public_revision(
        &self,
        id: &PublicTriggerId,
    ) -> Result<Option<TriggerRevision>, TriggerStoreError> {
        Ok((id == &self.revision.public_id).then(|| self.revision.clone()))
    }

    async fn list_revisions_for(
        &self,
        _owner: &SubjectId,
        _page: &TriggerPageRequest,
    ) -> Result<TriggerPage<TriggerRevision>, TriggerStoreError> {
        Ok(TriggerPage {
            items: vec![self.revision.clone()],
            next: None,
        })
    }

    async fn retire_revision(
        &self,
        _id: &TriggerRevisionId,
        _owner: &SubjectId,
        _retired_at: &str,
    ) -> Result<Option<TriggerRevision>, TriggerStoreError> {
        self.write_unimplemented()
    }
}

#[async_trait]
impl LaunchStore for FixtureStore {
    async fn insert_or_get_current(&self, _launch: &Launch) -> Result<Launch, TriggerStoreError> {
        self.write_unimplemented()
    }

    async fn get_current_launch(
        &self,
        owner: &SubjectId,
        revision: &TriggerRevisionId,
    ) -> Result<Option<Launch>, TriggerStoreError> {
        Ok(self
            .launch
            .lock()
            .unwrap()
            .as_ref()
            .filter(|launch| &launch.owner == owner && &launch.revision.id == revision)
            .cloned())
    }

    async fn insert_new(&self, _launch: &Launch) -> Result<(), TriggerStoreError> {
        self.write_unimplemented()
    }

    async fn get_launch(&self, id: &LaunchId) -> Result<Option<Launch>, TriggerStoreError> {
        Ok(self
            .launch
            .lock()
            .unwrap()
            .as_ref()
            .filter(|launch| &launch.id == id)
            .cloned())
    }

    async fn list_launches(
        &self,
        _query: &LaunchQuery,
    ) -> Result<TriggerPage<Launch>, TriggerStoreError> {
        Ok(TriggerPage {
            items: self.launch.lock().unwrap().iter().cloned().collect(),
            next: None,
        })
    }

    async fn replace_launch(
        &self,
        expected_revision: u64,
        expected_fence: Option<u64>,
        launch: &Launch,
    ) -> Result<Launch, TriggerStoreError> {
        let mut current = self.launch.lock().unwrap();
        let stored = current
            .as_ref()
            .ok_or_else(|| TriggerStoreError::Malformed("fixture Launch disappeared".into()))?;
        if stored.revision_number != expected_revision {
            return Err(TriggerStoreError::RevisionConflict {
                expected: expected_revision,
                actual: stored.revision_number,
            });
        }
        if expected_fence.is_some_and(|fence| fence != stored.fencing_counter) {
            return Err(TriggerStoreError::StaleFence {
                expected: expected_fence.unwrap(),
                actual: stored.fencing_counter,
            });
        }
        self.writes.fetch_add(1, Ordering::SeqCst);
        *current = Some(launch.clone());
        Ok(launch.clone())
    }

    async fn acquire_lease(
        &self,
        _id: &LaunchId,
        _expected_revision: u64,
        _holder: &str,
        _acquired_at: &str,
        _expires_at: &str,
    ) -> Result<Option<Launch>, TriggerStoreError> {
        self.write_unimplemented()
    }

    async fn release_lease(
        &self,
        _id: &LaunchId,
        _expected_revision: u64,
        _lease: &WorkerLease,
    ) -> Result<Launch, TriggerStoreError> {
        self.write_unimplemented()
    }
}

impl FixtureStore {
    fn write_unimplemented<T>(&self) -> Result<T, TriggerStoreError> {
        self.writes.fetch_add(1, Ordering::SeqCst);
        Err(TriggerStoreError::Unavailable(
            "fixture write is not implemented".into(),
        ))
    }
}

#[derive(Default)]
struct MemoryOperations {
    entries: Mutex<HashMap<OperationKey, OperationEntry>>,
    writes: AtomicUsize,
}

type OperationKey = (String, String, String);
type OperationEntry = ([u8; 32], Option<Vec<u8>>);

#[async_trait]
impl OperationStore for MemoryOperations {
    async fn reserve(&self, claim: &OperationClaim) -> Result<OperationReservation, StoreError> {
        let key = (
            claim.subject().0.clone(),
            claim.operation().to_string(),
            claim.key().to_string(),
        );
        let mut entries = self.entries.lock().unwrap();
        match entries.get(&key) {
            Some((fingerprint, _)) if fingerprint != claim.fingerprint() => {
                Ok(OperationReservation::FingerprintMismatch)
            }
            Some((_, Some(outcome))) => Ok(OperationReservation::Complete(outcome.clone())),
            Some((_, None)) => Ok(OperationReservation::Pending),
            None => {
                entries.insert(key, (*claim.fingerprint(), None));
                self.writes.fetch_add(1, Ordering::SeqCst);
                Ok(OperationReservation::Acquired)
            }
        }
    }

    async fn complete(&self, claim: &OperationClaim, outcome: Vec<u8>) -> Result<(), StoreError> {
        let key = (
            claim.subject().0.clone(),
            claim.operation().to_string(),
            claim.key().to_string(),
        );
        let mut entries = self.entries.lock().unwrap();
        let entry = entries
            .get_mut(&key)
            .ok_or_else(|| StoreError::Malformed("operation claim disappeared".into()))?;
        entry.1 = Some(outcome);
        self.writes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

struct StaticAuthority;

#[async_trait]
impl TriggerAuthorityVerifier for StaticAuthority {
    async fn verify_publisher(
        &self,
        subject: &SubjectId,
        content: &TriggerRevisionContent,
        selected_service_id: &str,
    ) -> Result<PublisherAuthority, TriggerServiceError> {
        let mut value = publisher_authority(subject, &content.upstream.repository);
        value.connection.service_id = selected_service_id.into();
        Ok(value)
    }

    async fn verify_launch(
        &self,
        subject: &SubjectId,
        _revision: &TriggerRevision,
        selected_service_id: &str,
        _previous: Option<&ConnectionSnapshot>,
    ) -> Result<LaunchAuthority, TriggerServiceError> {
        let mut selected = connection();
        selected.service_id = selected_service_id.into();
        let mut authority = provenance();
        authority.subject = subject.clone();
        authority.service_id = selected_service_id.into();
        Ok(LaunchAuthority {
            connection: selected,
            provenance: authority,
            source_sha: "a".repeat(40),
            workflow_paths: Vec::new(),
        })
    }
}

struct StaticConnections;

#[async_trait]
impl TriggerConnectionManager for StaticConnections {
    async fn list_connections(
        &self,
        _subject: &SubjectId,
    ) -> Result<Vec<TriggerConnection>, TriggerServiceError> {
        Ok(Vec::new())
    }

    async fn begin_connection(
        &self,
        _subject: &SubjectId,
        _intent_id: &str,
    ) -> Result<TriggerConnectionAuthorization, TriggerServiceError> {
        Err(TriggerServiceError::NotFound)
    }

    async fn reconnect(
        &self,
        _subject: &SubjectId,
        _service_id: &str,
    ) -> Result<TriggerConnectionAuthorization, TriggerServiceError> {
        Err(TriggerServiceError::NotFound)
    }

    async fn preflight_connection(
        &self,
        _subject: &SubjectId,
        _service_id: &str,
    ) -> Result<TriggerConnection, TriggerServiceError> {
        Err(TriggerServiceError::NotFound)
    }

    async fn resolve_repository(
        &self,
        _subject: &SubjectId,
        _service_id: &str,
        _repository_id: u64,
        _selected_ref: &str,
    ) -> Result<TriggerRepositorySelection, TriggerServiceError> {
        Err(TriggerServiceError::NotFound)
    }

    async fn resolve_repository_locator(
        &self,
        _subject: &SubjectId,
        _service_id: &str,
        owner: &str,
        name: &str,
    ) -> Result<TriggerRepositorySelection, TriggerServiceError> {
        if owner != "upstream-owner" || name != "project" {
            return Err(TriggerServiceError::NotFound);
        }
        Ok(TriggerRepositorySelection {
            repository: upstream(),
            source_sha: "a".repeat(40),
        })
    }
}

fn revision(owner: SubjectId) -> TriggerRevision {
    let content = content(owner.clone());
    TriggerRevision {
        id: TriggerRevisionId(REVISION_ID.into()),
        public_id: PublicTriggerId(PUBLIC_ID.into()),
        digest: content.digest().unwrap(),
        publisher_authority: publisher_authority(&owner, &content.upstream.repository),
        content,
        availability: TriggerAvailability::Published,
        published_at: NOW.into(),
        retired_at: None,
    }
}

fn content(owner: SubjectId) -> TriggerRevisionContent {
    TriggerRevisionContent {
        schema_version: TRIGGER_SCHEMA_VERSION,
        trigger_id: TriggerId(TRIGGER_ID.into()),
        revision_number: 1,
        publisher_subject_id: owner,
        publisher_display_identity: "Chrono Maintainer".into(),
        upstream: UpstreamRevision {
            repository: upstream(),
            source_mode: SourceMode::PinnedCommit,
            observed_source_sha: Some("a".repeat(40)),
            contribution_base_ref: "main".into(),
        },
        task: TriggerTask {
            public_summary: "Fix one bounded issue".into(),
            prompt_template: PRIVATE_PROMPT.into(),
            renderer_version: layer2_core::PROMPT_RENDERER_VERSION,
            input_fields: vec![InputFieldDefinition {
                key: "topic".into(),
                kind: InputFieldKind::Text {
                    min_bytes: 1,
                    max_bytes: 100,
                },
                required: true,
                default: Some(json!("the selected issue")),
                editable: true,
            }],
        },
        session: TriggerSessionPolicy {
            required_capabilities: vec![
                RequiredCapability::Conversation,
                RequiredCapability::DurableConversation,
                RequiredCapability::WorkspaceCheckpoint,
                RequiredCapability::RestrictedServiceGateway,
                RequiredCapability::CredentialRefresh,
                RequiredCapability::CredentialRevocation,
            ],
            permission_ceiling: PermissionMode::Autonomous,
            sandbox_profile: "cma-trigger-v1".into(),
            resources: ResourcePolicy {
                default: SessionSize::new(500, 1_024).unwrap(),
                ceiling: SessionSize::new(1_000, 2_048).unwrap(),
            },
            timeout_default_seconds: 3_600,
            timeout_ceiling_seconds: 7_200,
        },
        repository_access: RepositoryAccessPolicy {
            credential_route: CredentialRoute::ContributorConnection,
            required_scope_classes: vec![ScopeClass::PublicRepository],
            allowed_operation_classes: vec![RepositoryOperationClass::RepositoryRead],
            workflow_change_policy: WorkflowChangePolicy::Refuse,
        },
        setup: SetupPolicy {
            executor_policy: ExecutorPolicy::Disabled,
            steps: Vec::new(),
        },
        contribution: ContributionPolicy {
            branch_prefix: "cma/".into(),
            commit_attribution: CommitAttributionPolicy::ContributorNoReply,
            pull_request: PullRequestDefaults {
                title_template: "CMA contribution".into(),
                body_template: "Created by CMA".into(),
            },
        },
        commercial_policy: CommercialPolicy {
            billing_mode: BillingMode::ZeroCost,
            policy_reference: "public-zero-cost-v1".into(),
        },
        override_policy: OverridePolicy {
            editable_fields: vec![EditableField::TaskInput],
            optional_checks: Vec::new(),
        },
        public_projection: PublicProjectionPolicy { version: 1 },
    }
}

fn upstream() -> RepositoryIdentity {
    RepositoryIdentity {
        repository_id: 10,
        repository_node_id: "R_upstream".into(),
        owner_id: 20,
        owner_node_id: "U_upstream_owner".into(),
        canonical_owner: "upstream-owner".into(),
        canonical_name: "project".into(),
        visibility: RepositoryVisibility::Public,
        allow_forking: true,
        default_branch: "main".into(),
        selected_ref: "main".into(),
        fork_parent_id: None,
        fork_source_id: None,
    }
}

fn connection() -> ConnectionSnapshot {
    ConnectionSnapshot {
        service_id: "service-1".into(),
        service_state_version: 2,
        backing_key_id: "backing-key".into(),
        account: ConnectedAccount {
            account_id: 21,
            account_node_id: "U_contributor".into(),
            login: OWNER.into(),
            avatar_url: None,
        },
        state: ConnectionState::Active,
        granted_scopes: vec!["public_repo".into()],
        satisfied_scope_classes: vec![ScopeClass::PublicRepository],
        consent_observed_at: NOW.into(),
        preflight_observed_at: NOW.into(),
    }
}

fn provenance() -> AuthorityProvenance {
    AuthorityProvenance {
        subject: SubjectId(OWNER.into()),
        subject_token_generation: 1,
        service_id: "service-1".into(),
        service_state_version: 2,
        backing_key_id: "backing-key".into(),
        session_credential_generation: None,
        state: DelegationState::Available,
        observed_at: NOW.into(),
    }
}

fn publisher_authority(subject: &SubjectId, repository: &RepositoryIdentity) -> PublisherAuthority {
    PublisherAuthority {
        subject: subject.clone(),
        subject_token_generation: 1,
        connection: connection(),
        repository: repository.clone(),
        role: PublisherRepositoryRole::Owner,
        verified_at: NOW.into(),
    }
}
