use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use futures_util::StreamExt;
use layer2_core::{
    AuthorityProvenance, Authorization, BillingMode, BranchRecord, Capability,
    CapabilityRevocationState, CheckpointId, CheckpointIntegrity, CommercialPolicy,
    CommitAttributionPolicy, CommitIdentity, Confirmation, ConfirmationAction, ConnectedAccount,
    ConnectionSnapshot, ConnectionState, ContributionId, ContributionPolicy, ContributionRecord,
    CredentialRoute, DelegationState, DiffArtifact, DiffArtifactId, DiffArtifactObjectStore,
    DiffChangeKind, DiffFile, DiffValidationOutcome, EditableField, EffectRecord, EffectState,
    ExecutorPolicy, FailureClass, ForkRecord, InputFieldDefinition, InputFieldKind, Launch,
    LaunchAuthority, LaunchBlockedReason, LaunchEffectKind, LaunchFailure, LaunchId, LaunchQuery,
    LaunchReviewAdmission, LaunchState, LaunchStore, OperationClaim, OperationReservation,
    OperationStore, OverridePolicy, PermissionMode, PricingSnapshot, PromptDelivery,
    PromptDeliveryState, PublicProjectionPolicy, PublicTriggerId, PublicationState,
    PublishedRevision, PublisherAuthority, PublisherRepositoryRole, PullRequestDefaults,
    QuotaSnapshot, ReachStatus, RepositoryAccessPolicy, RepositoryIdentity,
    RepositoryOperationClass, RepositoryVisibility, RequiredCapability, ResourcePolicy,
    RuntimeAdmissionSnapshot, ScopeClass, SessionCredentialAuthority, SessionCredentialRetirement,
    SessionId, SessionLink, SessionSize, SetupPolicy, SourceMode, StartAdmissionSnapshot,
    StoreError, SubjectId, TriggerAdmissionError, TriggerAuthorityVerifier, TriggerConnection,
    TriggerConnectionAuthorization, TriggerConnectionManager, TriggerDraft, TriggerId,
    TriggerLaunchAdmission, TriggerPage, TriggerPageRequest, TriggerRepositorySelection,
    TriggerRevision, TriggerRevisionContent, TriggerRevisionId, TriggerService,
    TriggerServiceError, TriggerSessionPolicy, TriggerStore, TriggerStoreError, TriggerTask,
    UpstreamRevision, WorkerLease, WorkflowChangePolicy, WorkspaceCheckpoint, WorkspaceEntry,
    WorkspaceEntryKind, WorkspaceManifest, WorkspaceRecord, TRIGGER_SCHEMA_VERSION,
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
const MISSING_TRIGGER_ID: &str = "88888888-8888-4888-8888-888888888888";
const MISSING_REVISION_ID: &str = "99999999-9999-4999-8999-999999999999";
const MISSING_LAUNCH_ID: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
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
const SDK_PULL_REQUEST_STATUS_REFRESH_REQUEST_FIXTURE: &str =
    include_str!("../../tests/fixtures/rest-sdk/pull_request_status_refresh_request.json");

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
async fn launch_inventory_filters_one_owner_by_exact_session_id() {
    let mut fixture = Fixture::new();
    fixture.make_ready();
    let app = fixture.router();

    let matching = app
        .clone()
        .oneshot(authenticated_request(
            Method::GET,
            "/api/v1/launches?session_id=sesn_rest_ready&limit=1",
            OWNER,
        ))
        .await
        .unwrap();
    assert_eq!(matching.status(), StatusCode::OK);
    let matching: Value =
        serde_json::from_slice(&to_bytes(matching.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(matching["items"].as_array().unwrap().len(), 1);
    assert_eq!(matching["items"][0]["id"], LAUNCH_ID);

    let unrelated = app
        .oneshot(authenticated_request(
            Method::GET,
            "/api/v1/launches?session_id=sesn_unrelated&limit=1",
            OWNER,
        ))
        .await
        .unwrap();
    assert_eq!(unrelated.status(), StatusCode::OK);
    let unrelated: Value =
        serde_json::from_slice(&to_bytes(unrelated.into_body(), usize::MAX).await.unwrap())
            .unwrap();
    assert!(unrelated["items"].as_array().unwrap().is_empty());
    assert_eq!(fixture.store.writes.load(Ordering::SeqCst), 0);
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
    assert_eq!(
        preview.headers()[header::CACHE_CONTROL],
        "public, max-age=60"
    );
    assert_eq!(preview.headers()[header::REFERRER_POLICY], "no-referrer");
    let preview = to_bytes(preview.into_body(), usize::MAX).await.unwrap();
    assert!(!String::from_utf8_lossy(&preview).contains(PRIVATE_PROMPT));

    for suffix in [
        "",
        "/events?starting_after=0&limit=10&follow=false",
        "/diff",
    ] {
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
        if suffix.starts_with("/events") {
            assert!(wire.contains("id: "), "{wire:?}");
            assert!(wire.contains("data: {\"sequence\":"), "{wire:?}");
        }
        assert!(!wire.contains("threadId"));
        assert!(!wire.contains("runId"));
        assert!(!wire.contains("idempotency_key_hash"));
        assert!(!wire.contains("provider_request_id"));
    }

    assert_eq!(fixture.store.writes.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.operations.writes.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn agui_launch_events_are_bounded_resumable_sse_with_header_precedence() {
    let fixture = Fixture::new();
    let launch_id = fixture.launch.id.0.clone();
    let first_sequence = fixture.launch.events.first().unwrap().sequence;
    let second_sequence = fixture.launch.events.get(1).unwrap().sequence;
    let app = crate::agui::trigger::router(fixture.state());

    let initial = app
        .clone()
        .oneshot(authenticated_request(
            Method::GET,
            &format!(
                "/api/v1/agui/launches/{launch_id}/events?starting_after=0&limit=500&follow=false"
            ),
            OWNER,
        ))
        .await
        .unwrap();
    assert_eq!(initial.status(), StatusCode::OK);
    assert_eq!(
        initial.headers().get(header::CONTENT_TYPE).unwrap(),
        "text/event-stream"
    );
    let initial = to_bytes(initial.into_body(), usize::MAX).await.unwrap();
    let initial = String::from_utf8(initial.to_vec()).unwrap();
    assert!(
        initial.contains(&format!("id: {first_sequence}")),
        "{initial:?}"
    );
    assert!(initial.contains("data: {\"sequence\":"), "{initial:?}");
    assert!(!initial.contains("provider_request_id"));
    assert!(!initial.contains("worker_fencing_token"));

    let mut resumed = authenticated_request(
        Method::GET,
        &format!("/api/v1/agui/launches/{launch_id}/events?after=0&limit=500&follow=false"),
        OWNER,
    );
    resumed
        .headers_mut()
        .insert("last-event-id", first_sequence.to_string().parse().unwrap());
    let resumed = app.oneshot(resumed).await.unwrap();
    assert_eq!(resumed.status(), StatusCode::OK);
    let resumed = to_bytes(resumed.into_body(), usize::MAX).await.unwrap();
    let resumed = String::from_utf8(resumed.to_vec()).unwrap();
    assert!(
        resumed.contains(&format!("id: {second_sequence}")),
        "{resumed:?}"
    );
    assert!(
        !resumed.contains(&format!("id: {first_sequence}\n")),
        "Last-Event-ID must take precedence over the query cursor: {resumed:?}"
    );
}

#[tokio::test]
async fn agui_launch_events_follow_new_durable_progress() {
    let fixture = Fixture::new();
    let launch_id = fixture.launch.id.0.clone();
    let head = fixture.launch.event_high_water();
    let response = crate::agui::trigger::router(fixture.state())
        .oneshot(authenticated_request(
            Method::GET,
            &format!("/api/v1/agui/launches/{launch_id}/events?starting_after={head}&follow=true"),
            OWNER,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut stream = response.into_body().into_data_stream();

    let mut retained = fixture.store.launch.lock().unwrap().clone().unwrap();
    let sequence = retained.event_high_water() + 1;
    let progress = layer2_core::LaunchProgressState::from_launch(&retained);
    retained.events.push(LaunchEvent {
        sequence,
        launch_id: retained.id.clone(),
        occurred_at: NOW.into(),
        correlation_id: "followed-progress".into(),
        progress,
        kind: layer2_core::LaunchEventKind::DraftUpdated,
    });
    *fixture.store.launch.lock().unwrap() = Some(retained);

    let wire = tokio::time::timeout(Duration::from_secs(2), async {
        let mut wire = String::new();
        while !wire.contains(&format!("id: {sequence}")) {
            let chunk = stream
                .next()
                .await
                .expect("the followed Launch stream stays open")
                .expect("the followed Launch stream stays readable");
            wire.push_str(&String::from_utf8_lossy(&chunk));
        }
        wire
    })
    .await
    .expect("new durable Launch progress is observed within the polling bound");
    assert!(wire.contains("\"type\":\"draft_updated\""), "{wire:?}");
}

#[tokio::test]
async fn launch_progress_rejects_future_cursors_after_owner_concealment() {
    let fixture = Fixture::new();
    let launch_id = fixture.launch.id.0.clone();
    let head = fixture.launch.event_high_water();

    for (app, uri) in [
        (
            fixture.router(),
            format!(
                "/api/v1/launches/{launch_id}/events?after={}&limit=10",
                head + 1
            ),
        ),
        (
            crate::agui::trigger::router(fixture.state()),
            format!(
                "/api/v1/agui/launches/{launch_id}/events?starting_after={}&follow=false",
                head + 1
            ),
        ),
    ] {
        let response = app
            .oneshot(authenticated_request(Method::GET, &uri, OWNER))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{uri}");
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let problem: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(problem["code"], "cursor_beyond_end", "{uri}");
        assert_eq!(problem["head"], head, "{uri}");
    }

    let foreign = fixture
        .router()
        .oneshot(authenticated_request(
            Method::GET,
            &format!("/api/v1/launches/{launch_id}/events?after={}", head + 1),
            "foreign-subject",
        ))
        .await
        .unwrap();
    assert_eq!(foreign.status(), StatusCode::NOT_FOUND);
    let body = to_bytes(foreign.into_body(), usize::MAX).await.unwrap();
    let problem: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(problem["code"], "trigger_resource_not_found");
}

#[tokio::test]
async fn launch_progress_rejects_sequence_gaps_and_conflicting_duplicates() {
    for corruption in ["gap", "conflict"] {
        let fixture = Fixture::new();
        let launch_id = fixture.launch.id.0.clone();
        let mut retained = fixture.store.launch.lock().unwrap().clone().unwrap();
        if corruption == "gap" {
            let missing = retained.events.get(1).unwrap().sequence;
            retained.events.retain(|event| event.sequence != missing);
        } else {
            let mut duplicate = retained.events.first().unwrap().clone();
            duplicate.correlation_id = "conflicting-duplicate".into();
            retained.events.push(duplicate);
        }
        *fixture.store.launch.lock().unwrap() = Some(retained);

        let response = fixture
            .router()
            .oneshot(authenticated_request(
                Method::GET,
                &format!("/api/v1/launches/{launch_id}/events?after=0&limit=500"),
                OWNER,
            ))
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::SERVICE_UNAVAILABLE,
            "{corruption}"
        );
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let problem: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(problem["code"], "trigger_store_unavailable", "{corruption}");
    }
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
async fn private_rest_resources_conceal_wrong_owners_before_any_effect() {
    struct Case {
        label: &'static str,
        method: Method,
        owned_uri: String,
        missing_uri: String,
        body: Value,
    }

    let mut fixture = Fixture::new();
    let artifact = fixture.install_published_contribution();
    let trigger_id = fixture.store.draft.id.0.clone();
    let revision_id = fixture.revision.id.0.clone();
    let launch_id = fixture.launch.id.0.clone();
    let revision_number = fixture.launch.revision_number;
    let effective_digest = fixture.launch.effective.digest.clone();
    let artifact_digest = artifact.digest.clone();
    let artifact_id = artifact.id.0.clone();
    let update_content = trigger_content_request_value(&fixture.store.draft.content);
    let pull_request_digest = fixture
        .launch
        .pull_request_draft(PullRequestOperation::Open, None, None)
        .unwrap()
        .intent_digest;
    let cases = vec![
        Case {
            label: "draft-read",
            method: Method::GET,
            owned_uri: format!("/api/v1/triggers/{trigger_id}"),
            missing_uri: format!("/api/v1/triggers/{MISSING_TRIGGER_ID}"),
            body: json!({}),
        },
        Case {
            label: "draft-update",
            method: Method::PUT,
            owned_uri: format!("/api/v1/triggers/{trigger_id}"),
            missing_uri: format!("/api/v1/triggers/{MISSING_TRIGGER_ID}"),
            body: json!({
                "expected_version": fixture.store.draft.version,
                "selected_service_id": fixture.store.draft.selected_service_id.clone(),
                "content": update_content,
            }),
        },
        Case {
            label: "draft-publish",
            method: Method::POST,
            owned_uri: format!("/api/v1/triggers/{trigger_id}/publish"),
            missing_uri: format!("/api/v1/triggers/{MISSING_TRIGGER_ID}/publish"),
            body: json!({
                "expected_version": fixture.store.draft.version,
                "selected_service_id": fixture.store.draft.selected_service_id.clone(),
            }),
        },
        Case {
            label: "revision-read",
            method: Method::GET,
            owned_uri: format!("/api/v1/trigger-revisions/{revision_id}"),
            missing_uri: format!("/api/v1/trigger-revisions/{MISSING_REVISION_ID}"),
            body: json!({}),
        },
        Case {
            label: "revision-retire",
            method: Method::POST,
            owned_uri: format!("/api/v1/trigger-revisions/{revision_id}/retire"),
            missing_uri: format!("/api/v1/trigger-revisions/{MISSING_REVISION_ID}/retire"),
            body: json!({}),
        },
        Case {
            label: "launch-read",
            method: Method::GET,
            owned_uri: format!("/api/v1/launches/{launch_id}"),
            missing_uri: format!("/api/v1/launches/{MISSING_LAUNCH_ID}"),
            body: json!({}),
        },
        Case {
            label: "launch-overrides",
            method: Method::PATCH,
            owned_uri: format!("/api/v1/launches/{launch_id}/overrides"),
            missing_uri: format!("/api/v1/launches/{MISSING_LAUNCH_ID}/overrides"),
            body: json!({ "expected_revision": revision_number, "overrides": {} }),
        },
        Case {
            label: "launch-connection",
            method: Method::PUT,
            owned_uri: format!("/api/v1/launches/{launch_id}/connection"),
            missing_uri: format!("/api/v1/launches/{MISSING_LAUNCH_ID}/connection"),
            body: json!({
                "expected_revision": revision_number,
                "service_id": "service-1",
            }),
        },
        Case {
            label: "launch-start",
            method: Method::POST,
            owned_uri: format!("/api/v1/launches/{launch_id}/start"),
            missing_uri: format!("/api/v1/launches/{MISSING_LAUNCH_ID}/start"),
            body: json!({
                "expected_revision": revision_number,
                "snapshot_digest": effective_digest.clone(),
                "acknowledge_workflow_risk": false,
            }),
        },
        Case {
            label: "contribution-review",
            method: Method::POST,
            owned_uri: format!("/api/v1/launches/{launch_id}/contributions/review"),
            missing_uri: format!("/api/v1/launches/{MISSING_LAUNCH_ID}/contributions/review"),
            body: json!({
                "expected_revision": revision_number,
                "snapshot_digest": artifact_digest.clone(),
            }),
        },
        Case {
            label: "diff-read",
            method: Method::GET,
            owned_uri: format!("/api/v1/launches/{launch_id}/diff"),
            missing_uri: format!("/api/v1/launches/{MISSING_LAUNCH_ID}/diff"),
            body: json!({}),
        },
        Case {
            label: "contribution-publish",
            method: Method::POST,
            owned_uri: format!("/api/v1/launches/{launch_id}/contributions/publish"),
            missing_uri: format!("/api/v1/launches/{MISSING_LAUNCH_ID}/contributions/publish"),
            body: json!({
                "expected_revision": revision_number,
                "snapshot_digest": artifact_digest,
            }),
        },
        Case {
            label: "pull-request-review",
            method: Method::POST,
            owned_uri: format!("/api/v1/launches/{launch_id}/pull-request/review"),
            missing_uri: format!("/api/v1/launches/{MISSING_LAUNCH_ID}/pull-request/review"),
            body: json!({ "operation": "open" }),
        },
        Case {
            label: "pull-request-open",
            method: Method::POST,
            owned_uri: format!("/api/v1/launches/{launch_id}/pull-request/open"),
            missing_uri: format!("/api/v1/launches/{MISSING_LAUNCH_ID}/pull-request/open"),
            body: json!({
                "expected_revision": revision_number,
                "snapshot_digest": pull_request_digest.clone(),
            }),
        },
        Case {
            label: "pull-request-update",
            method: Method::POST,
            owned_uri: format!("/api/v1/launches/{launch_id}/pull-request/update"),
            missing_uri: format!("/api/v1/launches/{MISSING_LAUNCH_ID}/pull-request/update"),
            body: json!({
                "expected_revision": revision_number,
                "snapshot_digest": pull_request_digest,
            }),
        },
        Case {
            label: "pull-request-status-refresh",
            method: Method::POST,
            owned_uri: format!("/api/v1/launches/{launch_id}/pull-request/refresh"),
            missing_uri: format!("/api/v1/launches/{MISSING_LAUNCH_ID}/pull-request/refresh"),
            body: json!({ "expected_revision": revision_number }),
        },
        Case {
            label: "artifact-download",
            method: Method::GET,
            owned_uri: format!(
                "/api/v1/launches/{launch_id}/diff-artifacts/{artifact_id}/download"
            ),
            missing_uri: format!(
                "/api/v1/launches/{MISSING_LAUNCH_ID}/diff-artifacts/{artifact_id}/download"
            ),
            body: json!({}),
        },
        Case {
            label: "launch-cancel",
            method: Method::POST,
            owned_uri: format!("/api/v1/launches/{launch_id}/cancel"),
            missing_uri: format!("/api/v1/launches/{MISSING_LAUNCH_ID}/cancel"),
            body: json!({
                "expected_revision": revision_number,
                "snapshot_digest": effective_digest.clone(),
            }),
        },
        Case {
            label: "launch-complete",
            method: Method::POST,
            owned_uri: format!("/api/v1/launches/{launch_id}/complete"),
            missing_uri: format!("/api/v1/launches/{MISSING_LAUNCH_ID}/complete"),
            body: json!({
                "expected_revision": revision_number,
                "snapshot_digest": effective_digest.clone(),
            }),
        },
        Case {
            label: "initial-prompt-recovery",
            method: Method::POST,
            owned_uri: format!("/api/v1/launches/{launch_id}/recover-initial-prompt"),
            missing_uri: format!("/api/v1/launches/{MISSING_LAUNCH_ID}/recover-initial-prompt"),
            body: json!({
                "expected_revision": revision_number,
                "snapshot_digest": effective_digest,
                "acknowledge_duplicate_execution_risk": true,
            }),
        },
        Case {
            label: "launch-progress",
            method: Method::GET,
            owned_uri: format!("/api/v1/launches/{launch_id}/events?after=0&limit=10"),
            missing_uri: format!("/api/v1/launches/{MISSING_LAUNCH_ID}/events?after=0&limit=10"),
            body: json!({}),
        },
    ];
    let app = fixture.router();

    for (index, case) in cases.into_iter().enumerate() {
        let foreign = send_as(
            &app,
            case.method.clone(),
            &case.owned_uri,
            case.body.clone(),
            &format!("wrong-owner-{}-{index}", case.label),
            "foreign-subject",
        )
        .await;
        let missing = send_as(
            &app,
            case.method,
            &case.missing_uri,
            case.body,
            &format!("missing-resource-{}-{index}", case.label),
            "foreign-subject",
        )
        .await;

        assert_eq!(foreign.0, StatusCode::NOT_FOUND, "{}", case.label);
        assert_eq!(foreign, missing, "{} leaked resource existence", case.label);
        let problem: Value = serde_json::from_slice(&foreign.1).unwrap();
        assert_eq!(
            problem["code"], "trigger_resource_not_found",
            "{}",
            case.label
        );
        assert_eq!(
            fixture.store.writes.load(Ordering::SeqCst),
            0,
            "{}",
            case.label
        );
        assert_eq!(
            fixture.operations.writes.load(Ordering::SeqCst),
            0,
            "{}",
            case.label
        );
        assert_eq!(
            fixture.provider_calls.load(Ordering::SeqCst),
            0,
            "{}",
            case.label
        );
        assert_eq!(
            fixture.artifacts.reads.load(Ordering::SeqCst),
            0,
            "{}",
            case.label
        );
    }
}

#[tokio::test]
async fn private_agui_launch_resources_conceal_wrong_owners_without_effects() {
    let mut fixture = Fixture::new();
    fixture.install_diff(b"AG-UI owner concealment".to_vec());
    let launch_id = fixture.launch.id.0.clone();
    let app = crate::agui::trigger::router(fixture.state());

    for (index, suffix) in [
        "",
        "/events?starting_after=0&limit=10&follow=false",
        "/diff",
    ]
    .into_iter()
    .enumerate()
    {
        let foreign = send_as(
            &app,
            Method::GET,
            &format!("/api/v1/agui/launches/{launch_id}{suffix}"),
            json!({}),
            &format!("agui-wrong-owner-{index}"),
            "foreign-subject",
        )
        .await;
        let missing = send_as(
            &app,
            Method::GET,
            &format!("/api/v1/agui/launches/{MISSING_LAUNCH_ID}{suffix}"),
            json!({}),
            &format!("agui-missing-resource-{index}"),
            "foreign-subject",
        )
        .await;

        assert_eq!(foreign.0, StatusCode::NOT_FOUND, "{suffix}");
        assert_eq!(foreign, missing, "{suffix} leaked resource existence");
        let problem: Value = serde_json::from_slice(&foreign.1).unwrap();
        assert_eq!(problem["code"], "trigger_resource_not_found", "{suffix}");
    }

    assert_eq!(fixture.store.writes.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.operations.writes.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.provider_calls.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.artifacts.reads.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn repeated_start_replays_the_exact_response_and_advances_once() {
    let mut fixture = Fixture::new();
    // This route consults the real clock; keep this deterministic fixture live
    // without changing the checked-in wire corpus built from `NOW`.
    fixture.launch.draft_expires_at = "2099-08-27T12:00:00Z".into();
    let admission = review_admission(&fixture.launch, NOW);
    fixture.launch.apply_review_admission(admission).unwrap();
    *fixture.store.launch.lock().unwrap() = Some(fixture.launch.clone());
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

    assert_eq!(
        first.0,
        StatusCode::ACCEPTED,
        "first Start returned {}",
        String::from_utf8_lossy(&first.1)
    );
    assert_eq!(
        replay.0,
        StatusCode::ACCEPTED,
        "replayed Start returned {}",
        String::from_utf8_lossy(&replay.1)
    );
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
async fn concurrent_start_requests_admit_one_attempt_and_replay_its_exact_response() {
    let mut fixture = Fixture::new();
    fixture.launch.draft_expires_at = "2099-08-27T12:00:00Z".into();
    let admission = review_admission(&fixture.launch, NOW);
    fixture.launch.apply_review_admission(admission).unwrap();
    *fixture.store.launch.lock().unwrap() = Some(fixture.launch.clone());
    fixture.operations.synchronize_next_reservations(2);

    let launch_id = fixture.launch.id.0.clone();
    let uri = format!("/api/v1/launches/{launch_id}/start");
    let body = json!({
        "expected_revision": 0,
        "snapshot_digest": fixture.launch.effective.digest,
    });
    let app = fixture.router();
    let (first, second) = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(
            send(&app, Method::POST, &uri, body.clone(), "concurrent-start"),
            send(&app, Method::POST, &uri, body.clone(), "concurrent-start"),
        )
    })
    .await
    .expect("both browser-tab Start requests cross the operation reservation");
    fixture.operations.clear_reservation_barrier();

    let (accepted, pending) = if first.0 == StatusCode::ACCEPTED {
        (first, second)
    } else {
        (second, first)
    };
    assert_eq!(accepted.0, StatusCode::ACCEPTED);
    assert_eq!(pending.0, StatusCode::CONFLICT);
    let pending_problem: Value = serde_json::from_slice(&pending.1).unwrap();
    assert_eq!(pending_problem["code"], "idempotency_in_flight");
    assert_eq!(fixture.store.writes.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.operations.writes.load(Ordering::SeqCst), 2);
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

    let replay = send(&app, Method::POST, &uri, body, "concurrent-start").await;
    assert_eq!(replay, accepted);
    assert_eq!(fixture.store.writes.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.operations.writes.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn replayed_publisher_request_returns_the_exact_immutable_revision_once() {
    let fixture = Fixture::new();
    let trigger_id = fixture.store.draft.id.0.clone();
    let uri = format!("/api/v1/triggers/{trigger_id}/publish");
    let body = json!({
        "expected_version": fixture.store.draft.version,
        "selected_service_id": fixture.store.draft.selected_service_id.clone(),
    });
    let app = fixture.router();

    let first = send_as(
        &app,
        Method::POST,
        &uri,
        body.clone(),
        "one-publisher-request",
        "publisher",
    )
    .await;
    let replay = send_as(
        &app,
        Method::POST,
        &uri,
        body,
        "one-publisher-request",
        "publisher",
    )
    .await;

    assert_eq!(first.0, StatusCode::CREATED);
    assert_eq!(replay, first);
    assert_eq!(fixture.store.writes.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.operations.writes.load(Ordering::SeqCst), 2);
    assert_eq!(fixture.provider_calls.load(Ordering::SeqCst), 1);
    assert!(fixture.store.published.lock().unwrap().is_some());
}

#[tokio::test]
async fn same_connection_revalidation_resumes_the_original_failed_effect() {
    let mut fixture = Fixture::new();
    fixture.launch.state = LaunchState::Preflight;
    fixture.launch.started_at = Some(NOW.into());
    let lease = WorkerLease {
        holder: "expired-credential-worker".into(),
        fencing_token: 1,
        acquired_at: NOW.into(),
        expires_at: "2026-08-26T12:10:00Z".into(),
    };
    fixture.launch.fencing_counter = lease.fencing_token;
    fixture.launch.lease = Some(lease.clone());
    let claim = fixture
        .launch
        .claim_next_effect(&lease, NOW, "owner-credential-claim")
        .unwrap()
        .unwrap();
    let original_intent = claim.intent_key.clone();
    fixture
        .launch
        .fail_effect(
            &lease,
            &claim,
            LaunchFailure {
                class: FailureClass::OwnerCredentialUnavailable,
                code: "owner_credential_unavailable".into(),
                detail: "CMA no longer holds usable sign-in authority for this operation.".into(),
                remediation: "Sign in again to resume the same Launch.".into(),
                correlation_id: "owner-credential-expired".into(),
                retry_at: None,
                observed_at: "2026-08-26T12:00:01Z".into(),
            },
            BTreeMap::new(),
            None,
            "2026-08-26T12:00:01Z",
        )
        .unwrap();
    fixture.launch.lease = None;
    *fixture.store.launch.lock().unwrap() = Some(fixture.launch.clone());
    let launch_id = fixture.launch.id.0.clone();
    let app = fixture.router();

    let blocked = app
        .clone()
        .oneshot(authenticated_request(
            Method::GET,
            &format!("/api/v1/launches/{launch_id}"),
            OWNER,
        ))
        .await
        .unwrap();
    assert_eq!(blocked.status(), StatusCode::OK);
    let blocked = to_bytes(blocked.into_body(), usize::MAX).await.unwrap();
    let blocked: Value = serde_json::from_slice(&blocked).unwrap();
    assert_eq!(blocked["state"], "auth_required");
    assert_eq!(blocked["resume_state"], "preflight");
    assert_eq!(blocked["blocked_reason"], "owner_credential_unavailable");
    assert_eq!(
        blocked["connection"]["service_id"],
        fixture.launch.connection.as_ref().unwrap().service_id
    );

    let body = json!({
        "expected_revision": fixture.launch.revision_number,
        "service_id": fixture.launch.connection.as_ref().unwrap().service_id,
    });
    let first = send(
        &app,
        Method::PUT,
        &format!("/api/v1/launches/{launch_id}/connection"),
        body.clone(),
        "owner-sign-in-recovery",
    )
    .await;
    let replay = send(
        &app,
        Method::PUT,
        &format!("/api/v1/launches/{launch_id}/connection"),
        body,
        "owner-sign-in-recovery",
    )
    .await;
    assert_eq!(first.0, StatusCode::OK);
    assert_eq!(replay, first);
    let resumed: Value = serde_json::from_slice(&first.1).unwrap();
    assert_eq!(resumed["state"], "preflight");
    assert!(resumed["resume_state"].is_null());
    assert_eq!(resumed["connection"]["service_id"], "service-1");
    assert_eq!(fixture.store.writes.load(Ordering::SeqCst), 1);

    let retained = fixture.store.launch.lock().unwrap().clone().unwrap();
    assert_eq!(retained.effects.len(), 1);
    assert_eq!(retained.effects[0].state, EffectState::Reconciling);
    assert_eq!(retained.effects[0].intent_key, original_intent);
    assert_eq!(retained.effects[0].attempt, 1);
    assert_eq!(retained.resume_state, None);
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
async fn pull_request_status_refresh_is_revisioned_idempotent_and_agui_read_only() {
    let mut fixture = Fixture::new();
    fixture.install_open_pull_request();
    let launch_id = fixture.launch.id.0.clone();
    let expected_revision = fixture.launch.revision_number;
    let before_event = fixture.launch.event_high_water();
    let uri = format!("/api/v1/launches/{launch_id}/pull-request/refresh");
    let body = json!({ "expected_revision": expected_revision });

    let first = send(
        &fixture.router(),
        Method::POST,
        &uri,
        body.clone(),
        "refresh-open-pr-status",
    )
    .await;
    let replay = send(
        &fixture.router(),
        Method::POST,
        &uri,
        body.clone(),
        "refresh-open-pr-status",
    )
    .await;

    assert_eq!(first.0, StatusCode::ACCEPTED);
    assert_eq!(replay, first);
    assert_eq!(fixture.store.writes.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.provider_calls.load(Ordering::SeqCst), 0);
    let response: Value = serde_json::from_slice(&first.1).unwrap();
    assert_eq!(response["revision_number"], expected_revision + 1);
    assert_eq!(response["state"], "pr_open");
    assert!(response["pull_request"]["status_failure"].is_null());
    assert_eq!(
        response["pull_request"]["provider_request_id"],
        "github-request-fixture"
    );
    assert_eq!(response["pull_request"]["head_matches_launch"], false);

    let retained = fixture.store.launch.lock().unwrap().clone().unwrap();
    assert_eq!(retained.revision_number, expected_revision + 1);
    assert!(retained
        .pull_request
        .as_ref()
        .unwrap()
        .status_sync
        .next_attempt_at
        .is_none());
    assert!(matches!(
        retained.events.last().map(|event| &event.kind),
        Some(layer2_core::LaunchEventKind::PullRequestStatusRefreshRequested)
    ));

    let stale = send(
        &fixture.router(),
        Method::POST,
        &uri,
        body,
        "refresh-open-pr-status-stale-revision",
    )
    .await;
    assert_eq!(stale.0, StatusCode::CONFLICT);
    assert_eq!(fixture.store.writes.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.provider_calls.load(Ordering::SeqCst), 0);

    let agui = crate::agui::trigger::router(fixture.state());
    let progress = agui
        .clone()
        .oneshot(authenticated_request(
            Method::GET,
            &format!(
                "/api/v1/agui/launches/{launch_id}/events?starting_after={before_event}&follow=false"
            ),
            OWNER,
        ))
        .await
        .unwrap();
    assert_eq!(progress.status(), StatusCode::OK);
    let progress = to_bytes(progress.into_body(), usize::MAX).await.unwrap();
    let progress = String::from_utf8(progress.to_vec()).unwrap();
    assert!(
        progress.contains("\"type\":\"pull_request_status_refresh_requested\""),
        "{progress:?}"
    );

    let diff = agui
        .oneshot(authenticated_request(
            Method::GET,
            &format!("/api/v1/agui/launches/{launch_id}/diff"),
            OWNER,
        ))
        .await
        .unwrap();
    assert_eq!(diff.status(), StatusCode::OK);
    let diff: Value =
        serde_json::from_slice(&to_bytes(diff.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(
        diff["pull_request"]["provider_request_id"],
        "github-request-fixture"
    );
    assert_eq!(diff["pull_request"]["head_matches_launch"], false);
}

#[tokio::test]
async fn contributor_completion_is_idempotent_and_leaves_the_provider_pull_request_open() {
    let mut fixture = Fixture::new();
    fixture.install_open_pull_request();
    let launch_id = fixture.launch.id.0.clone();
    let expected_revision = fixture.launch.revision_number;
    let snapshot_digest = fixture.launch.effective.digest.clone();
    let uri = format!("/api/v1/launches/{launch_id}/complete");
    let body = json!({
        "expected_revision": expected_revision,
        "snapshot_digest": snapshot_digest,
    });

    let first = send(
        &fixture.router(),
        Method::POST,
        &uri,
        body.clone(),
        "finish-open-pr-handoff",
    )
    .await;
    let replay = send(
        &fixture.router(),
        Method::POST,
        &uri,
        body,
        "finish-open-pr-handoff",
    )
    .await;

    assert_eq!(first.0, StatusCode::OK);
    assert_eq!(replay, first);
    assert_eq!(fixture.store.writes.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.provider_calls.load(Ordering::SeqCst), 0);
    let response: Value = serde_json::from_slice(&first.1).unwrap();
    assert_eq!(response["state"], "completed");
    assert_eq!(response["pull_request"]["state"], "open");
    assert_eq!(
        response["pull_request"]["canonical_url"],
        "https://github.com/upstream-owner/project/pull/17"
    );
    assert_eq!(response["reach"]["state"], "revocation_pending");

    let retained = fixture.store.launch.lock().unwrap().clone().unwrap();
    assert_eq!(retained.state, LaunchState::Completed);
    assert_eq!(
        retained.pull_request.as_ref().unwrap().state,
        layer2_core::PullRequestState::Open
    );
    assert!(retained
        .confirmations
        .iter()
        .any(|confirmation| { confirmation.action == ConfirmationAction::Complete }));
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
    assert_eq!(value["artifact"]["files"][0]["additions"], 1);
    assert_eq!(value["artifact"]["files"][0]["deletions"], 0);
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

    let projection =
        serde_json::to_value(LaunchView::try_from(fixture.launch.clone()).unwrap()).unwrap();
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
async fn invalid_runtime_admissions_are_typed_and_absent_from_public_inventory() {
    for (corruption, expected_field) in [
        ("missing", "runtime_admission"),
        ("unknown-kernel", "runtime_admission.kernel"),
        ("missing-version", "runtime_admission.runtime_version"),
        (
            "legacy-missing-version",
            "runtime_admission.runtime_version",
        ),
        ("malformed-time", "runtime_admission.observed_at"),
        (
            "missing-capability",
            "runtime_admission.supported_capabilities",
        ),
        (
            "duplicate-capability",
            "runtime_admission.supported_capabilities",
        ),
    ] {
        let fixture = Fixture::new();
        {
            let mut retained = fixture.store.launch.lock().unwrap();
            let launch = retained.as_mut().unwrap();
            match corruption {
                "missing" => launch.runtime_admission = None,
                "unknown-kernel" => {
                    launch.runtime_admission.as_mut().unwrap().kernel =
                        layer2_core::KernelId::new("unknown-trigger-route");
                }
                "missing-version" => {
                    launch.runtime_admission.as_mut().unwrap().runtime_version = " \t".into();
                }
                "legacy-missing-version" => {
                    let runtime = launch.runtime_admission.as_mut().unwrap();
                    runtime.kernel = layer2_core::KernelId::new(LEGACY_PUBLIC_TRIGGER_KERNEL_ID);
                    runtime.runtime_version = " \t".into();
                }
                "malformed-time" => {
                    launch.runtime_admission.as_mut().unwrap().observed_at =
                        "not-a-timestamp".into();
                }
                "missing-capability" => {
                    launch
                        .runtime_admission
                        .as_mut()
                        .unwrap()
                        .supported_capabilities
                        .pop();
                }
                "duplicate-capability" => {
                    let runtime = launch.runtime_admission.as_mut().unwrap();
                    runtime
                        .supported_capabilities
                        .push(runtime.supported_capabilities[0]);
                }
                _ => unreachable!("the table contains only declared corruptions"),
            }
        }

        let direct = fixture
            .router()
            .oneshot(authenticated_request(
                Method::GET,
                &format!("/api/v1/launches/{LAUNCH_ID}"),
                OWNER,
            ))
            .await
            .unwrap();
        assert_eq!(direct.status(), StatusCode::CONFLICT, "{corruption}");
        let body = to_bytes(direct.into_body(), usize::MAX).await.unwrap();
        let body: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            body["code"], "trigger_runtime_route_unavailable",
            "{corruption}"
        );
        assert_eq!(body["field"], expected_field, "{corruption}");
        assert!(
            body["remediation"]
                .as_str()
                .unwrap()
                .contains("Open a New Launch"),
            "{corruption}"
        );

        let page = fixture
            .router()
            .oneshot(authenticated_request(
                Method::GET,
                "/api/v1/launches",
                OWNER,
            ))
            .await
            .unwrap();
        assert_eq!(page.status(), StatusCode::OK, "{corruption}");
        let body = to_bytes(page.into_body(), usize::MAX).await.unwrap();
        let body: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["items"], json!([]), "{corruption}");
        assert_eq!(fixture.store.writes.load(Ordering::SeqCst), 0);
        assert_eq!(fixture.operations.writes.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn exact_legacy_runtime_is_readable_reclickable_cancellable_and_otherwise_read_only() {
    let fixture = Fixture::new();
    let (expected_revision, snapshot_digest) = {
        let mut retained = fixture.store.launch.lock().unwrap();
        let launch = retained.as_mut().unwrap();
        launch.runtime_admission.as_mut().unwrap().kernel =
            layer2_core::KernelId::new(LEGACY_PUBLIC_TRIGGER_KERNEL_ID);
        launch.draft_expires_at = "2026-08-27T00:00:00Z".into();
        (launch.revision_number, launch.effective.digest.clone())
    };
    let launch_id = fixture.launch.id.0.clone();
    let public_id = fixture.revision.public_id.0.clone();
    let rest = fixture.router();

    let (status, body) = send(
        &rest,
        Method::POST,
        &format!("/api/v1/triggers/public/{public_id}/launches"),
        json!({}),
        "legacy-readme-reclick",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["id"], launch_id);
    assert_eq!(body["runtime_admission"]["kernel"], "codex");
    assert_eq!(body["draft_expires_at"], "2026-08-27T00:00:00Z");
    assert_eq!(fixture.store.writes.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.provider_calls.load(Ordering::SeqCst), 0);

    for uri in [
        format!("/api/v1/launches/{launch_id}"),
        format!("/api/v1/launches/{launch_id}/events?after=0&limit=10"),
        format!("/api/v1/launches/{launch_id}/diff"),
    ] {
        let (status, _) = send(&rest, Method::GET, &uri, json!({}), "legacy-rest-read").await;
        assert_eq!(status, StatusCode::OK, "{uri}");
    }

    let page = rest
        .clone()
        .oneshot(authenticated_request(
            Method::GET,
            "/api/v1/launches",
            OWNER,
        ))
        .await
        .unwrap();
    assert_eq!(page.status(), StatusCode::OK);
    let page: Value =
        serde_json::from_slice(&to_bytes(page.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(page["items"][0]["id"], launch_id);
    assert_eq!(page["items"][0]["runtime_admission"]["kernel"], "codex");

    let agui = crate::agui::trigger::router(fixture.state());
    for suffix in [
        "",
        "/events?starting_after=0&limit=10&follow=false",
        "/diff",
    ] {
        let uri = format!("/api/v1/agui/launches/{launch_id}{suffix}");
        let (status, _) = send(&agui, Method::GET, &uri, json!({}), "legacy-agui-read").await;
        assert_eq!(status, StatusCode::OK, "{uri}");
    }

    for (index, (method, uri)) in [
        (
            Method::PATCH,
            format!("/api/v1/launches/{launch_id}/overrides"),
        ),
        (
            Method::PUT,
            format!("/api/v1/launches/{launch_id}/connection"),
        ),
        (Method::POST, format!("/api/v1/launches/{launch_id}/start")),
        (
            Method::POST,
            format!("/api/v1/launches/{launch_id}/contributions/review"),
        ),
        (
            Method::POST,
            format!("/api/v1/launches/{launch_id}/contributions/publish"),
        ),
        (
            Method::POST,
            format!("/api/v1/launches/{launch_id}/pull-request/review"),
        ),
        (
            Method::POST,
            format!("/api/v1/launches/{launch_id}/pull-request/open"),
        ),
        (
            Method::POST,
            format!("/api/v1/launches/{launch_id}/pull-request/update"),
        ),
        (
            Method::POST,
            format!("/api/v1/launches/{launch_id}/pull-request/refresh"),
        ),
        (
            Method::POST,
            format!("/api/v1/launches/{launch_id}/complete"),
        ),
        (
            Method::POST,
            format!("/api/v1/launches/{launch_id}/recover-initial-prompt"),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let response = send(
            &rest,
            method,
            &uri,
            json!({}),
            &format!("legacy-current-command-{index}"),
        )
        .await;
        assert_runtime_quarantine(response, "runtime_admission.kernel", &uri);
    }
    assert_eq!(fixture.store.writes.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.provider_calls.load(Ordering::SeqCst), 0);

    let (status, body) = send(
        &rest,
        Method::POST,
        &format!("/api/v1/launches/{launch_id}/cancel"),
        json!({
            "expected_revision": expected_revision,
            "snapshot_digest": snapshot_digest,
        }),
        "legacy-cancel",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["state"], "cancelled");
    assert_eq!(body["runtime_admission"]["kernel"], "codex");
    assert_eq!(fixture.store.writes.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.provider_calls.load(Ordering::SeqCst), 0);

    let mut artifact_fixture = Fixture::new();
    let artifact = artifact_fixture.install_diff(b"legacy diff artifact".to_vec());
    artifact_fixture
        .store
        .launch
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .runtime_admission
        .as_mut()
        .unwrap()
        .kernel = layer2_core::KernelId::new(LEGACY_PUBLIC_TRIGGER_KERNEL_ID);
    let artifact_uri = format!(
        "/api/v1/launches/{}/diff-artifacts/{}/download",
        artifact_fixture.launch.id.0, artifact.id.0
    );
    let response = artifact_fixture
        .router()
        .oneshot(authenticated_request(Method::GET, &artifact_uri, OWNER))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(artifact_fixture.artifacts.reads.load(Ordering::SeqCst), 1);
    assert_eq!(artifact_fixture.store.writes.load(Ordering::SeqCst), 0);
    assert_eq!(artifact_fixture.provider_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn malformed_native_runtime_is_quarantined_before_every_public_surface_or_mutation() {
    let fixture = Fixture::new();
    fixture
        .store
        .launch
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .runtime_admission
        .as_mut()
        .unwrap()
        .runtime_version = String::new();
    let launch_id = fixture.launch.id.0.clone();
    let public_id = fixture.revision.public_id.0.clone();
    let rest = fixture.router();

    let rest_reads = [
        format!("/api/v1/launches/{launch_id}"),
        format!("/api/v1/launches/{launch_id}/events?after=0&limit=10"),
        format!("/api/v1/launches/{launch_id}/diff"),
        format!("/api/v1/launches/{launch_id}/diff-artifacts/{DIFF_ID}/download"),
    ];
    for (index, uri) in rest_reads.into_iter().enumerate() {
        let response = send(
            &rest,
            Method::GET,
            &uri,
            json!({}),
            &format!("quarantined-read-{index}"),
        )
        .await;
        assert_runtime_quarantine(response, "runtime_admission.runtime_version", &uri);
    }

    let agui = crate::agui::trigger::router(fixture.state());
    for suffix in [
        "",
        "/events?starting_after=0&limit=10&follow=false",
        "/diff",
    ] {
        let uri = format!("/api/v1/agui/launches/{launch_id}{suffix}");
        let response = send(
            &agui,
            Method::GET,
            &uri,
            json!({}),
            &format!("quarantined-agui-{suffix}"),
        )
        .await;
        assert_runtime_quarantine(response, "runtime_admission.runtime_version", &uri);
    }

    let mutations = [
        (
            Method::PATCH,
            format!("/api/v1/launches/{launch_id}/overrides"),
        ),
        (
            Method::PUT,
            format!("/api/v1/launches/{launch_id}/connection"),
        ),
        (Method::POST, format!("/api/v1/launches/{launch_id}/start")),
        (
            Method::POST,
            format!("/api/v1/launches/{launch_id}/contributions/review"),
        ),
        (
            Method::POST,
            format!("/api/v1/launches/{launch_id}/contributions/publish"),
        ),
        (
            Method::POST,
            format!("/api/v1/launches/{launch_id}/pull-request/review"),
        ),
        (
            Method::POST,
            format!("/api/v1/launches/{launch_id}/pull-request/open"),
        ),
        (
            Method::POST,
            format!("/api/v1/launches/{launch_id}/pull-request/update"),
        ),
        (
            Method::POST,
            format!("/api/v1/launches/{launch_id}/pull-request/refresh"),
        ),
        (Method::POST, format!("/api/v1/launches/{launch_id}/cancel")),
        (
            Method::POST,
            format!("/api/v1/launches/{launch_id}/complete"),
        ),
        (
            Method::POST,
            format!("/api/v1/launches/{launch_id}/recover-initial-prompt"),
        ),
        (
            Method::POST,
            format!("/api/v1/triggers/public/{public_id}/launches"),
        ),
    ];
    for (index, (method, uri)) in mutations.into_iter().enumerate() {
        let response = send(
            &rest,
            method,
            &uri,
            json!({}),
            &format!("quarantined-mutation-{index}"),
        )
        .await;
        assert_runtime_quarantine(response, "runtime_admission.runtime_version", &uri);
    }

    assert_eq!(fixture.store.writes.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.operations.writes.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.provider_calls.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.artifacts.reads.load(Ordering::SeqCst), 0);
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

#[tokio::test]
async fn connection_revocation_is_owner_scoped_idempotent_and_convergent() {
    let fixture = Fixture::new();
    let connections = Arc::new(RevokingConnections::new(false));
    let app = fixture.router_with_connections(connections.clone());
    let uri = "/api/v1/trigger-connections/service-exact";

    let mut missing_key = Request::builder()
        .method(Method::DELETE)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from("{}"))
        .unwrap();
    missing_key
        .extensions_mut()
        .insert(AuthenticatedSubject(SubjectId(OWNER.into())));
    missing_key
        .extensions_mut()
        .insert(Authorization::from_capabilities([
            Capability::CreateSession,
        ]));
    let missing_key = app.clone().oneshot(missing_key).await.unwrap();
    assert_eq!(missing_key.status(), StatusCode::BAD_REQUEST);
    assert_eq!(connections.revoke_calls.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.operations.writes.load(Ordering::SeqCst), 0);

    let (unknown, _) = send(
        &app,
        Method::DELETE,
        "/api/v1/trigger-connections/service-unknown",
        json!({}),
        "revoke-unknown",
    )
    .await;
    assert_eq!(unknown, StatusCode::NOT_FOUND);
    let (foreign, _) = send_as(
        &app,
        Method::DELETE,
        uri,
        json!({}),
        "revoke-foreign",
        "another-subject",
    )
    .await;
    assert_eq!(foreign, StatusCode::NOT_FOUND);
    assert_eq!(connections.revoke_calls.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.operations.writes.load(Ordering::SeqCst), 0);

    let (first_status, first_body) =
        send(&app, Method::DELETE, uri, json!({}), "revoke-exact").await;
    assert_eq!(first_status, StatusCode::OK);
    let first: Value = serde_json::from_slice(&first_body).unwrap();
    assert_eq!(first["service_id"], "service-exact");
    assert_eq!(first["state"], "inactive");
    assert_eq!(first["state_version"], 8);
    let wire = String::from_utf8_lossy(&first_body);
    for forbidden in [
        "access_token",
        "refresh_token",
        "backing_key",
        "session_credential_generation",
        "session_credential_authority",
        "custody_generation",
        "provider_key_id",
        "provider_state_version",
        "grant_fingerprint",
        "provider_body",
        "idempotency",
    ] {
        assert!(!wire.contains(forbidden), "revocation leaked {forbidden}");
    }
    assert_eq!(connections.revoke_calls.load(Ordering::SeqCst), 1);

    let (replay_status, replay_body) =
        send(&app, Method::DELETE, uri, json!({}), "revoke-exact").await;
    assert_eq!(replay_status, StatusCode::OK);
    assert_eq!(replay_body, first_body);
    assert_eq!(connections.revoke_calls.load(Ordering::SeqCst), 1);

    let (body_conflict, _) = send(
        &app,
        Method::DELETE,
        uri,
        json!({ "unexpected": true }),
        "revoke-exact",
    )
    .await;
    assert_eq!(body_conflict, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(connections.revoke_calls.load(Ordering::SeqCst), 1);

    let (repeated_status, repeated_body) =
        send(&app, Method::DELETE, uri, json!({}), "revoke-again").await;
    assert_eq!(repeated_status, StatusCode::OK);
    let repeated: Value = serde_json::from_slice(&repeated_body).unwrap();
    assert_eq!(repeated["service_id"], "service-exact");
    assert_eq!(repeated["state"], "inactive");
    assert_eq!(repeated["state_version"], 8);
    assert_eq!(connections.revoke_calls.load(Ordering::SeqCst), 2);
    assert_eq!(connections.list_calls.load(Ordering::SeqCst), 0);

    let wrong_method = app
        .oneshot(authenticated_request(Method::POST, uri, OWNER))
        .await
        .unwrap();
    assert_eq!(wrong_method.status(), StatusCode::METHOD_NOT_ALLOWED);
}

#[tokio::test]
async fn connection_revocation_reconciles_a_lost_response_after_router_restart() {
    let fixture = Fixture::new();
    let connections = Arc::new(RevokingConnections::new(false));
    let uri = "/api/v1/trigger-connections/service-exact";

    let first_app = fixture.router_with_connections(connections.clone());
    let (first_status, _) = send(
        &first_app,
        Method::DELETE,
        uri,
        json!({}),
        "revoke-lost-response",
    )
    .await;
    assert_eq!(first_status, StatusCode::OK);
    assert_eq!(connections.revoke_calls.load(Ordering::SeqCst), 1);
    drop(first_app);

    let restarted_app = fixture.router_with_connections(connections.clone());
    let (replay_status, replay_body) = send(
        &restarted_app,
        Method::DELETE,
        uri,
        json!({}),
        "revoke-lost-response",
    )
    .await;
    assert_eq!(replay_status, StatusCode::OK);
    let replay: Value = serde_json::from_slice(&replay_body).unwrap();
    assert_eq!(replay["service_id"], "service-exact");
    assert_eq!(replay["state"], "inactive");
    assert_eq!(replay["state_version"], 8);
    assert_eq!(connections.revoke_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        connections.revocation_lookup_calls.load(Ordering::SeqCst),
        2
    );
    assert_eq!(connections.list_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn connection_revocation_rejects_a_mismatched_public_projection() {
    let fixture = Fixture::new();
    let connections = Arc::new(RevokingConnections::new(true));
    let app = fixture.router_with_connections(connections.clone());

    let (status, body) = send(
        &app,
        Method::DELETE,
        "/api/v1/trigger-connections/service-exact",
        json!({}),
        "revoke-mismatched-projection",
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["code"], "trigger_authority_drift");
    assert!(!body.to_string().contains("service-other"));
    assert_eq!(connections.revoke_calls.load(Ordering::SeqCst), 1);
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
        ("delete", "/api/v1/trigger-connections/{service_id}"),
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
        ("post", "/api/v1/launches/{launch_id}/pull-request/refresh"),
        (
            "get",
            "/api/v1/launches/{launch_id}/diff-artifacts/{artifact_id}/download",
        ),
        (
            "post",
            "/api/v1/launches/{launch_id}/recover-initial-prompt",
        ),
        ("post", "/api/v1/launches/{launch_id}/cancel"),
        ("post", "/api/v1/launches/{launch_id}/complete"),
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
            "delete" => item.delete.is_some(),
            _ => unreachable!("the Trigger catalogue uses a known method"),
        };
        assert!(present, "missing {method} operation for {path}");
    }

    let document = serde_json::to_value(openapi).expect("OpenAPI serializes");
    let launch_list_parameters = document["paths"]["/api/v1/launches"]["get"]["parameters"]
        .as_array()
        .expect("Launch inventory has query parameters");
    assert!(launch_list_parameters.iter().any(|parameter| {
        parameter["name"] == "session_id"
            && parameter["in"] == "query"
            && parameter["required"] == false
    }));
    let schemas = document["components"]["schemas"]
        .as_object()
        .expect("Trigger OpenAPI has component schemas");
    for name in [
        "TriggerTask",
        "TriggerSessionPolicy",
        "TriggerAvailability",
        "TriggerPreviewAvailability",
        "TriggerPreviewUnavailableReason",
        "RepositoryVisibility",
        "PublisherRepositoryRole",
        "ConnectionState",
        "PermissionMode",
        "RequiredCapability",
        "BillingMode",
        "TriggerPreviewCostView",
        "ProblemDocument",
        "EditableField",
        "RepositoryAccessPolicy",
        "SetupPolicy",
        "ContributionPolicy",
        "CommercialPolicy",
        "OverridePolicy",
        "PublicProjectionPolicy",
        "TriggerRevisionContent",
        "LaunchOverrides",
        "EffectiveConfiguration",
        "LaunchState",
        "LaunchStepKey",
        "LaunchStepStatus",
        "LaunchBlockedReason",
        "LaunchFailure",
        "LaunchStepView",
        "LaunchProgressStateView",
        "LaunchEventKindView",
        "LaunchEventView",
        "LaunchProgressView",
        "PricingSnapshot",
        "Rate",
        "TriggerKernel",
        "RuntimeAdmissionSnapshot",
        "QuotaSnapshot",
        "StartAdmissionSnapshot",
        "ForkRecord",
        "BranchRecord",
        "SessionLink",
        "SessionCredentialRetirement",
        "ReachStatus",
        "PromptDelivery",
        "PromptDeliveryState",
        "LaunchEffectKind",
        "ConfirmationAction",
        "DiffValidationOutcome",
        "CheckpointIntegrity",
        "PublicationState",
        "PullRequestOperation",
        "PullRequestState",
        "Mergeability",
        "CheckSummaryState",
        "DiffFile",
        "CheckResult",
        "CommitIdentity",
        "WorkspaceManifest",
        "RefreshPullRequestStatusRequest",
    ] {
        assert!(schemas.contains_key(name), "missing concrete schema {name}");
    }
    assert_eq!(
        schemas["PermissionMode"]["enum"],
        json!(["guarded", "autonomous"]),
        "the public MVP must not advertise unrestricted permission"
    );
    assert_eq!(
        schemas["EditableField"]["enum"],
        json!(["task_input", "work_label", "resource_size", "timeout"]),
        "optional_checks remains an empty compatibility field in the public MVP"
    );
    assert_eq!(
        schemas["TriggerKernel"]["enum"],
        json!(["codex", "codex-trigger-native-v1"])
    );
    assert_eq!(
        schemas["RuntimeAdmissionSnapshot"]["properties"]["kernel"]["$ref"],
        "#/components/schemas/TriggerKernel"
    );
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
        ("LaunchEventView", "progress", "LaunchProgressStateView"),
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
    let diff_file_required = schemas["DiffFile"]["required"]
        .as_array()
        .expect("DiffFile has required fields");
    for field in ["additions", "deletions"] {
        assert!(
            diff_file_required.iter().any(|required| required == field),
            "DiffFile.{field} must always appear on the wire"
        );
        let schema = &schemas["DiffFile"]["properties"][field];
        let wire = schema.to_string();
        assert!(
            wire.contains("integer") && wire.contains("null"),
            "DiffFile.{field} must be a nullable integer: {schema}"
        );
    }
    for (owner, field, target) in [
        (
            "TriggerPreviewView",
            "availability",
            "TriggerPreviewAvailability",
        ),
        ("TriggerPreviewView", "visibility", "RepositoryVisibility"),
        ("TriggerPreviewView", "permission_ceiling", "PermissionMode"),
        ("TriggerPreviewView", "billing_mode", "BillingMode"),
        (
            "TriggerPreviewView",
            "estimated_cost",
            "TriggerPreviewCostView",
        ),
        ("PublisherAuthorityView", "role", "PublisherRepositoryRole"),
        ("TriggerRevisionView", "availability", "TriggerAvailability"),
        ("TriggerConnectionView", "state", "ConnectionState"),
        ("LaunchView", "state", "LaunchState"),
        ("LaunchStepView", "key", "LaunchStepKey"),
        ("LaunchStepView", "status", "LaunchStepStatus"),
        ("DiffArtifactView", "validation", "DiffValidationOutcome"),
        (
            "WorkspaceCheckpointView",
            "integrity",
            "CheckpointIntegrity",
        ),
        ("ContributionRecordView", "state", "PublicationState"),
        ("PullRequestRecordView", "operation", "PullRequestOperation"),
        ("PullRequestRecordView", "state", "PullRequestState"),
        ("PullRequestRecordView", "mergeability", "Mergeability"),
        ("PullRequestRecordView", "checks", "CheckSummaryState"),
    ] {
        assert_eq!(
            schemas[owner]["properties"][field]["$ref"],
            format!("#/components/schemas/{target}"),
            "{owner}.{field} must reference its closed wire vocabulary"
        );
    }
    assert_eq!(
        schemas["TriggerPreviewAvailability"]["enum"],
        json!(["published", "retired", "unavailable"])
    );
    assert_eq!(
        schemas["TriggerPreviewUnavailableReason"]["enum"],
        json!([
            "upstream_not_forkable",
            "upstream_unavailable",
            "upstream_identity_changed",
            "repository_policy_unavailable"
        ])
    );
    let unavailable_reason = &schemas["TriggerPreviewView"]["properties"]["unavailable_reason"];
    let unavailable_reason_wire = unavailable_reason.to_string();
    assert!(
        unavailable_reason_wire.contains("#/components/schemas/TriggerPreviewUnavailableReason")
            && unavailable_reason_wire.contains("null"),
        "preview unavailable_reason must be nullable and closed: {unavailable_reason}"
    );
    let status_failure = &schemas["PullRequestRecordView"]["properties"]["status_failure"];
    let status_failure_wire = status_failure.to_string();
    assert!(
        status_failure_wire.contains("#/components/schemas/LaunchFailure")
            && status_failure_wire.contains("null"),
        "pull-request status failure must be a nullable concrete LaunchFailure: {status_failure}"
    );
    for (field, wire_type) in [
        ("provider_request_id", "string"),
        ("head_matches_launch", "boolean"),
    ] {
        let schema = &schemas["PullRequestRecordView"]["properties"][field];
        let wire = schema.to_string();
        assert!(
            wire.contains(wire_type) && wire.contains("null"),
            "PullRequestRecordView.{field} must be nullable {wire_type}: {schema}"
        );
    }
    let event_wire = schemas["LaunchEventKindView"].to_string();
    for kind in [
        "pull_request_status_changed",
        "pull_request_status_refresh_requested",
    ] {
        assert!(
            event_wire.contains(kind),
            "LaunchEventKindView omitted {kind}: {event_wire}"
        );
    }
    assert_eq!(
        schemas["RefreshPullRequestStatusRequest"]["required"],
        json!(["expected_revision"])
    );
    let refresh = &document["paths"]["/api/v1/launches/{launch_id}/pull-request/refresh"]["post"];
    assert_eq!(
        refresh["requestBody"]["content"]["application/json"]["schema"]["$ref"],
        "#/components/schemas/RefreshPullRequestStatusRequest"
    );
    assert!(refresh["responses"].get("202").is_some());
    assert!(refresh["parameters"].as_array().is_some_and(|parameters| {
        parameters.iter().any(|parameter| {
            parameter["name"] == "Idempotency-Key"
                && parameter["in"] == "header"
                && parameter["required"] == true
        })
    }));
    assert_eq!(
        schemas["TriggerPreviewView"]["properties"]["required_capabilities"]["items"]["$ref"],
        "#/components/schemas/RequiredCapability"
    );
    let preview_cost = schemas["TriggerPreviewCostView"].to_string();
    for required in [
        "exact_zero",
        "contributor_estimate",
        "quote_unavailable",
        "amount_micros",
        "quoted_at",
    ] {
        assert!(
            preview_cost.contains(required),
            "public Trigger cost schema omitted {required}: {preview_cost}"
        );
    }
    let problem = &schemas["ProblemDocument"]["properties"];
    assert!(problem.get("remediation").is_some());
    let validation_category = problem["validation_category"].to_string();
    for category in [
        "repository_identity",
        "repository_policy",
        "repository_ref",
        "scope_capability",
        "commercial_policy",
        "trigger_policy",
    ] {
        assert!(
            validation_category.contains(category),
            "ProblemDocument omitted validation category {category}: {validation_category}"
        );
    }
    for (method, path) in [
        ("post", "/api/v1/triggers"),
        ("put", "/api/v1/triggers/{trigger_id}"),
        ("post", "/api/v1/triggers/{trigger_id}/publish"),
    ] {
        let response = &document["paths"][path][method]["responses"]["422"];
        assert_eq!(
            response["content"]["application/problem+json"]["schema"]["$ref"],
            "#/components/schemas/ProblemDocument",
            "{method} {path} must document actionable Trigger validation"
        );
    }
    for (field, target) in [
        ("blocked_reason", "LaunchBlockedReason"),
        ("failure", "LaunchFailure"),
        ("pricing", "PricingSnapshot"),
        ("runtime_admission", "RuntimeAdmissionSnapshot"),
        ("review_quota", "QuotaSnapshot"),
        ("start_admission", "StartAdmissionSnapshot"),
        ("fork", "ForkRecord"),
        ("branch", "BranchRecord"),
        ("session", "SessionLink"),
        (
            "session_credential_retirement",
            "SessionCredentialRetirement",
        ),
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
    for (owner, field) in [
        ("OverridePolicy", "optional_checks"),
        ("LaunchOverrides", "optional_checks"),
        ("EffectiveConfiguration", "optional_checks"),
    ] {
        assert_eq!(
            schemas[owner]["properties"][field]["maxItems"], 0,
            "{owner}.{field} must remain empty-only for the public MVP"
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
    const CREDENTIAL_MARKER: &str = "SENSITIVE_CREDENTIAL_SENTINEL_RELEASE_673";
    let mut fixture = Fixture::new();
    fixture.make_ready();
    fixture.install_diff(b"retained review".to_vec());
    let session_id = fixture.launch.session.as_ref().unwrap().session_id.clone();
    fixture
        .launch
        .authority
        .as_mut()
        .expect("the reviewed Launch retains authority")
        .session_credential_authority = Some(SessionCredentialAuthority {
        session_id,
        custody_generation: 9,
        provider_key_id: CREDENTIAL_MARKER.into(),
        provider_state_version: 11,
        provider_expires_at_epoch_milliseconds: 4_000_000_000_000,
        grant_fingerprint: "d".repeat(64),
    });
    fixture.launch.steps[0].external_resource_ids = std::collections::BTreeMap::from([
        ("session_id".into(), "public-session-id".into()),
        (
            "provider_request_id".into(),
            "private-provider-request".into(),
        ),
        (
            "workspace_object_reference".into(),
            "private/workspace/object".into(),
        ),
        (
            "checkpoint_object".into(),
            "private/checkpoint/object".into(),
        ),
        (
            "diff_artifact_object".into(),
            "private/diff-artifact/object".into(),
        ),
        ("prompt_origin_key".into(), "private-prompt-origin".into()),
    ]);
    let wire =
        serde_json::to_string(&LaunchView::try_from(fixture.launch.clone()).unwrap()).unwrap();
    assert!(wire.contains("public-session-id"));
    for forbidden in [
        "backing_key_id",
        "worker_fencing_token",
        "fencing_counter",
        "subject_token_generation",
        "session_credential_generation",
        "session_credential_authority",
        "custody_generation",
        "provider_key_id",
        "provider_state_version",
        "grant_fingerprint",
        "idempotency_key_hash",
        "provider_request_id",
        "object_reference",
        "baseline_object_reference",
        "checkpoint_object",
        "diff_artifact_object",
        "prompt_origin_key",
        "private-provider-request",
        CREDENTIAL_MARKER,
    ] {
        assert!(!wire.contains(forbidden), "leaked {forbidden}");
    }
}

#[test]
fn launch_projection_exposes_only_positive_session_credential_retirement_evidence() {
    let mut fixture = Fixture::new();
    fixture.make_ready();
    fixture.launch.state = LaunchState::Completed;
    fixture.launch.terminal_at = Some("2026-08-28T01:02:44Z".into());
    fixture.launch.reach.as_mut().unwrap().state = CapabilityRevocationState::Revoked;
    let session_id = fixture.launch.session.as_ref().unwrap().session_id.clone();
    fixture.launch.session_credential_retirement = Some(SessionCredentialRetirement {
        session_id: session_id.clone(),
        settled_at: "2026-08-28T01:02:45Z".into(),
    });

    let projection =
        serde_json::to_value(LaunchView::try_from(fixture.launch.clone()).unwrap()).unwrap();
    assert_eq!(
        projection["session_credential_retirement"],
        json!({
            "session_id": session_id.0,
            "settled_at": "2026-08-28T01:02:45Z",
        })
    );
}

#[tokio::test]
async fn progress_projection_excludes_worker_and_credential_internals() {
    let fixture = Fixture::new();
    let mut retained = fixture.store.launch.lock().unwrap().clone().unwrap();
    retained.steps[0].external_resource_ids = std::collections::BTreeMap::from([
        (
            "pull_request_url".into(),
            "https://example.test/pr/1".into(),
        ),
        (
            "provider_request_id".into(),
            "private-provider-request".into(),
        ),
        (
            "workspace_object_reference".into(),
            "private/workspace/object".into(),
        ),
        (
            "checkpoint_object".into(),
            "private/checkpoint/object".into(),
        ),
        ("prompt_payload_hash".into(), "private-prompt-hash".into()),
    ]);
    let sequence = retained.events.last().map_or(1, |event| event.sequence + 1);
    retained.events.push(LaunchEvent {
        sequence,
        launch_id: retained.id.clone(),
        occurred_at: NOW.into(),
        correlation_id: "progress-projection".into(),
        progress: layer2_core::LaunchProgressState::from_launch(&retained),
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
    assert!(wire.contains("https://example.test/pr/1"));
    for forbidden in [
        "worker_fencing_token",
        "backing_key_id",
        "subject_token_generation",
        "session_credential_generation",
        "session_credential_authority",
        "custody_generation",
        "provider_key_id",
        "provider_state_version",
        "grant_fingerprint",
        "idempotency_key_hash",
        "provider_request_id",
        "object_reference",
        "checkpoint_object",
        "prompt_payload_hash",
        "private-provider-request",
    ] {
        assert!(!wire.contains(forbidden), "leaked {forbidden}");
    }
}

async fn trigger_sdk_fixture_values() -> Vec<(&'static str, Value)> {
    let draft_fixture = Fixture::new();
    let mut fixture = Fixture::new();
    fixture.install_open_pull_request();
    let mut events = draft_fixture.launch.events.clone();
    events.push(LaunchEvent {
        sequence: events.last().map_or(1, |event| event.sequence + 1),
        launch_id: draft_fixture.launch.id.clone(),
        occurred_at: NOW.into(),
        correlation_id: "fixture-progress".into(),
        progress: layer2_core::LaunchProgressState::from_launch(&draft_fixture.launch),
        kind: layer2_core::LaunchEventKind::StepChanged {
            step: draft_fixture.launch.steps[0].clone(),
        },
    });
    for kind in [
        layer2_core::LaunchEventKind::PullRequestStatusRefreshRequested,
        layer2_core::LaunchEventKind::PullRequestStatusChanged {
            state: layer2_core::PullRequestState::Open,
            mergeability: layer2_core::Mergeability::Mergeable,
            checks: layer2_core::CheckSummaryState::Passing,
            failure: None,
        },
    ] {
        events.push(LaunchEvent {
            sequence: events.last().map_or(1, |event| event.sequence + 1),
            launch_id: fixture.launch.id.clone(),
            occurred_at: NOW.into(),
            correlation_id: "fixture-pr-status".into(),
            progress: layer2_core::LaunchProgressState::from_launch(&draft_fixture.launch),
            kind,
        });
    }
    let mut proposal_fixture = Fixture::new();
    proposal_fixture.install_published_contribution();
    let draft = proposal_fixture
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
            serde_json::to_value(LaunchView::try_from(draft_fixture.launch.clone()).unwrap())
                .unwrap(),
        ),
        (
            "launch_progress.json",
            serde_json::to_value(LaunchProgressView {
                launch_id: draft_fixture.launch.id.0.clone(),
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
                "snapshot_digest": proposal_fixture
                    .launch
                    .pull_request_draft(PullRequestOperation::Open, None, None)
                    .unwrap()
                    .intent_digest
            }),
        ),
        (
            "pull_request_status_refresh_request.json",
            json!({ "expected_revision": fixture.launch.revision_number }),
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
        (
            "pull_request_status_refresh_request.json",
            SDK_PULL_REQUEST_STATUS_REFRESH_REQUEST_FIXTURE,
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
    let refresh: RefreshPullRequestStatusRequest =
        serde_json::from_str(SDK_PULL_REQUEST_STATUS_REFRESH_REQUEST_FIXTURE).unwrap();
    assert_eq!(refresh.expected_revision, 0);
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
            Authority::UnapprovedScope {
                scope: "repo".into(),
            },
            StatusCode::UNPROCESSABLE_ENTITY,
            "trigger_scope_unapproved",
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
            Authority::ProviderAuthenticationUnavailable,
            StatusCode::SERVICE_UNAVAILABLE,
            "trigger_provider_authentication_unavailable",
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
async fn admission_failures_have_stable_status_codes_and_remediation() {
    use layer2_core::TriggerAdmissionError as Admission;

    for (error, status, code) in [
        (
            Admission::Unavailable("private deployment detail".into()),
            StatusCode::SERVICE_UNAVAILABLE,
            "trigger_admission_unavailable",
        ),
        (
            Admission::PricingUnavailable("private price detail".into()),
            StatusCode::SERVICE_UNAVAILABLE,
            "trigger_pricing_unavailable",
        ),
        (
            Admission::PriceChanged,
            StatusCode::CONFLICT,
            "trigger_price_changed",
        ),
        (
            Admission::QuoteExpired,
            StatusCode::CONFLICT,
            "trigger_quote_expired",
        ),
        (
            Admission::QuotaExceeded("owner has four live sessions".into()),
            StatusCode::CONFLICT,
            "trigger_quota_exhausted",
        ),
        (
            Admission::CapabilityUnsupported {
                capability: "workspace_checkpoint".into(),
            },
            StatusCode::UNPROCESSABLE_ENTITY,
            "trigger_capability_unsupported",
        ),
        (
            Admission::RuntimeChanged,
            StatusCode::CONFLICT,
            "trigger_runtime_changed",
        ),
    ] {
        let response = TriggerProblem(TriggerServiceError::Admission(error)).into_response();
        assert_eq!(response.status(), status);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body = serde_json::from_slice::<Value>(&body).unwrap();
        assert_eq!(body["code"], code);
        assert!(body["remediation"]
            .as_str()
            .is_some_and(|value| !value.is_empty()));
        assert!(!body.to_string().contains("private deployment detail"));
        assert!(!body.to_string().contains("private price detail"));
        if code == "trigger_capability_unsupported" {
            assert_eq!(body["capability"], "workspace_checkpoint");
        }
    }

    let response = TriggerProblem(TriggerServiceError::Validation(
        layer2_core::TriggerValidationError::field(
            "draft_expires_at",
            "private persistence detail",
        ),
    ))
    .into_response();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body = serde_json::from_slice::<Value>(&body).unwrap();
    assert_eq!(body["code"], "trigger_draft_expired");
    assert_eq!(body["field"], "draft_expires_at");
    assert!(body["remediation"].as_str().unwrap().contains("New Launch"));
    assert!(!body.to_string().contains("private persistence detail"));
}

#[tokio::test]
async fn trigger_validation_failures_have_safe_category_specific_remediation() {
    for (field, category, remediation_fragment) in [
        (
            "upstream.repository_node_id",
            "repository_identity",
            "canonical numeric and node identities",
        ),
        ("upstream", "repository_policy", "policy permits forking"),
        (
            "upstream.selected_ref",
            "repository_ref",
            "safe existing ref",
        ),
        (
            "repository_access.required_scope_classes",
            "scope_capability",
            "public-repository scope class",
        ),
        (
            "session.required_capabilities",
            "scope_capability",
            "capabilities supported",
        ),
        (
            "commercial_policy.policy_reference",
            "commercial_policy",
            "matching deployment-owned pricing policy",
        ),
    ] {
        let response = TriggerProblem(TriggerServiceError::Validation(
            layer2_core::TriggerValidationError::field(
                field,
                "private provider evidence and caller-controlled value",
            ),
        ))
        .into_response();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body = serde_json::from_slice::<Value>(&body).unwrap();
        assert_eq!(body["code"], "invalid_trigger_request");
        assert_eq!(body["field"], field);
        assert_eq!(body["validation_category"], category);
        assert!(body["remediation"]
            .as_str()
            .is_some_and(|value| value.contains(remediation_fragment)));
        assert!(!body.to_string().contains("private provider evidence"));
        assert!(!body.to_string().contains("caller-controlled value"));
    }
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

#[tokio::test]
async fn unconfigured_sponsorship_route_fails_closed_without_reflecting_credentials() {
    const CREDENTIAL_MARKER: &str = "sponsorship-secret-do-not-reflect";
    let fixture = Fixture::new();
    let app: axum::Router = crate::rest::sponsorships::documented_router(fixture.state()).into();
    let mut request = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/sponsorships/credentials")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({
                "stable_request_id": "33333333-3333-4333-8333-333333333333",
                "owner": {
                    "kind": "developer",
                    "display_name": "Sponsor"
                },
                "provider": "provider",
                "provider_policy_key": "provider-policy-v1",
                "credential": CREDENTIAL_MARKER
            })
            .to_string(),
        ))
        .unwrap();
    request
        .extensions_mut()
        .insert(AuthenticatedSubject(SubjectId(OWNER.into())));
    request
        .extensions_mut()
        .insert(Authorization::from_capabilities([
            Capability::ManageOwnSponsorships,
        ]));

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "application/problem+json"
    );
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["code"], "sponsorship_unavailable");
    assert!(!body.to_string().contains(CREDENTIAL_MARKER));
}

#[test]
fn sponsorship_openapi_is_complete_secret_aware_and_has_no_activation_command() {
    let documented = crate::rest::sponsorships::documented_router(Fixture::new().state());
    let document = serde_json::to_value(documented.get_openapi()).unwrap();
    let paths = document["paths"].as_object().unwrap();
    let expected = BTreeSet::from([
        "/api/v1/launches/{launch_id}/sponsorship/acceptance",
        "/api/v1/launches/{launch_id}/sponsorship/grant",
        "/api/v1/launches/{launch_id}/sponsorship/terms",
        "/api/v1/sponsorships/audit",
        "/api/v1/sponsorships/credentials",
        "/api/v1/sponsorships/credentials/{credential_id}",
        "/api/v1/sponsorships/credentials/{credential_id}/revoke",
        "/api/v1/sponsorships/credentials/{credential_id}/rotate",
        "/api/v1/sponsorships/grants/{grant_id}/close",
        "/api/v1/sponsorships/grants/{grant_id}/revoke",
        "/api/v1/sponsorships/kill-switch",
        "/api/v1/sponsorships/policies",
        "/api/v1/sponsorships/policies/{policy_id}",
        "/api/v1/sponsorships/policies/{policy_id}/revoke",
        "/api/v1/sponsorships/policies/{policy_id}/suspend",
        "/api/v1/sponsorships/usage/reconciliations",
        "/api/v1/sponsorships/usage/reservations",
    ]);
    assert_eq!(
        paths.keys().map(String::as_str).collect::<BTreeSet<_>>(),
        expected
    );
    assert!(paths.keys().all(|path| !path.contains("activate")));

    assert_eq!(
        document["components"]["schemas"]["RegisterCredentialRequest"]["properties"]["credential"]
            ["format"],
        "password"
    );
    assert_eq!(
        document["components"]["schemas"]["RotateCredentialRequest"]["properties"]["replacement"]
            ["format"],
        "password"
    );
    let reservation_responses =
        &document["paths"]["/api/v1/sponsorships/usage/reservations"]["post"]["responses"];
    for status in ["200", "201", "429", "503"] {
        assert!(
            reservation_responses.get(status).is_some(),
            "usage reservation is missing its {status} contract"
        );
    }
}

fn trigger_content_request_value(content: &TriggerRevisionContent) -> Value {
    let mut request = serde_json::to_value(content).unwrap();
    let upstream = request["upstream"].clone();
    let request = request.as_object_mut().unwrap();
    request.remove("trigger_id");
    request.remove("revision_number");
    request.remove("publisher_subject_id");
    request.remove("upstream");
    request.insert(
        "repository".into(),
        json!({
            "repository_id": upstream["repository"]["repository_id"],
            "selected_ref": upstream["repository"]["selected_ref"],
            "contribution_base_ref": upstream["contribution_base_ref"],
            "source_mode": upstream["source_mode"],
        }),
    );
    Value::Object(request.clone())
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

fn assert_runtime_quarantine(response: (StatusCode, Vec<u8>), expected_field: &str, context: &str) {
    assert_eq!(response.0, StatusCode::CONFLICT, "{context}");
    let problem: Value = serde_json::from_slice(&response.1).unwrap();
    assert_eq!(
        problem["code"], "trigger_runtime_route_unavailable",
        "{context}"
    );
    assert_eq!(problem["field"], expected_field, "{context}");
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
    provider_calls: Arc<AtomicUsize>,
}

impl Fixture {
    fn new() -> Self {
        let owner = SubjectId(OWNER.into());
        let revision = revision(SubjectId("publisher".into()));
        let draft = TriggerDraft {
            id: revision.content.trigger_id.clone(),
            owner: revision.content.publisher_subject_id.clone(),
            selected_service_id: "service-1".into(),
            version: 1,
            next_revision_number: revision.content.revision_number,
            content: revision.content.clone(),
            created_at: NOW.into(),
            updated_at: NOW.into(),
        };
        let mut launch = Launch::new(owner, revision.clone(), NOW, "create").unwrap();
        launch.id = LaunchId(LAUNCH_ID.into());
        for event in &mut launch.events {
            event.launch_id = launch.id.clone();
        }
        launch
            .select_connection(connection(), NOW, "connection")
            .unwrap();
        launch.authority = Some(provenance());
        launch
            .apply_review_admission(review_admission(&launch, NOW))
            .unwrap();
        let store = Arc::new(FixtureStore {
            draft,
            revision: revision.clone(),
            published: Mutex::new(None),
            launch: Mutex::new(Some(launch.clone())),
            writes: AtomicUsize::new(0),
        });
        Self {
            revision,
            launch,
            store,
            operations: Arc::new(MemoryOperations::default()),
            artifacts: Arc::new(MemoryDiffArtifacts::default()),
            provider_calls: Arc::new(AtomicUsize::new(0)),
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
            retired_checkpoint: None,
            sealed_contribution_checkpoint: None,
            checkpoint_intent: None,
            lifecycle_checkpoint: Default::default(),
            checkpoint_version: 0,
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
            authority_admission: None,
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
            recovery_pair: None,
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
            retired_checkpoint: None,
            sealed_contribution_checkpoint: Some(checkpoint.clone()),
            checkpoint_intent: None,
            lifecycle_checkpoint: Default::default(),
            checkpoint_version: 1,
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
                additions: Some(1),
                deletions: Some(0),
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
            object_write: None,
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

    fn install_open_pull_request(&mut self) -> DiffArtifact {
        let artifact = self.install_published_contribution();
        let draft = self
            .launch
            .pull_request_draft(PullRequestOperation::Open, None, None)
            .unwrap();
        self.launch
            .confirm_pull_request(
                Confirmation {
                    action: ConfirmationAction::OpenPullRequest,
                    subject: self.launch.owner.clone(),
                    snapshot_digest: draft.intent_digest,
                    confirmed_at: NOW.into(),
                    idempotency_key_hash: "fixture-open-pr".into(),
                },
                None,
                None,
                NOW,
                "fixture-open-pr",
            )
            .unwrap();
        let record = self.launch.pull_request.as_mut().unwrap();
        record.provider_id = Some(9001);
        record.provider_node_id = Some("PR_fixture".into());
        record.number = Some(17);
        record.canonical_url = Some("https://github.com/upstream-owner/project/pull/17".into());
        record.state = layer2_core::PullRequestState::Open;
        record.base_revision = Some(self.launch.source_sha.clone());
        record.head_revision = Some("f".repeat(40));
        record.observed_title = Some(record.intent.title.clone());
        record.observed_body_digest = Some(record.intent.body_digest.clone());
        record.mergeability = layer2_core::Mergeability::Conflicting;
        record.checks = layer2_core::CheckSummaryState::Failing;
        record.provider_created_at = Some(NOW.into());
        record.provider_updated_at = Some(NOW.into());
        record.status_sync = layer2_core::PullRequestStatusSync {
            consecutive_failures: 1,
            next_attempt_at: Some("2099-08-26T12:01:00Z".into()),
            last_failure: Some(LaunchFailure {
                class: FailureClass::RetryableTransient,
                code: "github_status_unavailable".into(),
                detail: "GitHub status could not be refreshed safely.".into(),
                remediation: "CMA will retry the read-only status observation.".into(),
                correlation_id: "fixture-pr-status".into(),
                retry_at: Some("2099-08-26T12:01:00Z".into()),
                observed_at: NOW.into(),
            }),
            provider_request_id: Some("github-request-fixture".into()),
            head_matches_launch: Some(false),
        };
        record.validate_intent(&self.launch.id).unwrap();
        self.launch.state = LaunchState::PrOpen;
        *self.store.launch.lock().unwrap() = Some(self.launch.clone());
        artifact
    }

    fn state(&self) -> TriggersState {
        TriggersState {
            triggers: Arc::new(
                TriggerService::new(
                    self.store.clone(),
                    self.store.clone(),
                    Arc::new(StaticAuthority {
                        calls: Arc::clone(&self.provider_calls),
                    }),
                )
                .with_launch_admission(Arc::new(StaticAdmission)),
            ),
            connections: Arc::new(StaticConnections {
                calls: Arc::clone(&self.provider_calls),
            }),
            operations: self.operations.clone(),
            diff_artifacts: Some(self.artifacts.clone()),
            sponsorships: None,
        }
    }

    fn router(&self) -> axum::Router {
        documented_router(self.state()).into()
    }

    fn router_with_connections(
        &self,
        connections: Arc<dyn TriggerConnectionManager>,
    ) -> axum::Router {
        let mut state = self.state();
        state.connections = connections;
        documented_router(state).into()
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
    draft: TriggerDraft,
    revision: TriggerRevision,
    published: Mutex<Option<PublishedRevision>>,
    launch: Mutex<Option<Launch>>,
    writes: AtomicUsize,
}

#[async_trait]
impl TriggerStore for FixtureStore {
    async fn insert_draft(&self, _draft: &TriggerDraft) -> Result<(), TriggerStoreError> {
        self.write_unimplemented()
    }

    async fn get_draft(&self, id: &TriggerId) -> Result<Option<TriggerDraft>, TriggerStoreError> {
        Ok((id == &self.draft.id).then(|| self.draft.clone()))
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
        expected_draft_version: u64,
        draft: &TriggerDraft,
        revision: &TriggerRevision,
    ) -> Result<PublishedRevision, TriggerStoreError> {
        if expected_draft_version != self.draft.version {
            return Err(TriggerStoreError::RevisionConflict {
                expected: expected_draft_version,
                actual: self.draft.version,
            });
        }
        let published = PublishedRevision {
            draft: draft.clone(),
            revision: revision.clone(),
        };
        let mut retained = self.published.lock().unwrap();
        if retained.is_some() {
            return Err(TriggerStoreError::AlreadyExists);
        }
        *retained = Some(published.clone());
        self.writes.fetch_add(1, Ordering::SeqCst);
        Ok(published)
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

    async fn expire_current_and_insert(
        &self,
        _expired: &Launch,
        _replacement: &Launch,
    ) -> Result<Launch, TriggerStoreError> {
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
        query: &LaunchQuery,
    ) -> Result<TriggerPage<Launch>, TriggerStoreError> {
        Ok(TriggerPage {
            items: self
                .launch
                .lock()
                .unwrap()
                .iter()
                .filter(|launch| {
                    query
                        .owner
                        .as_ref()
                        .is_none_or(|owner| &launch.owner == owner)
                        && query
                            .public_id
                            .as_ref()
                            .is_none_or(|public_id| &launch.revision.public_id == public_id)
                        && query.session_id.as_ref().is_none_or(|session_id| {
                            launch
                                .session
                                .as_ref()
                                .is_some_and(|session| &session.session_id == session_id)
                        })
                        && (query.states.is_empty() || query.states.contains(&launch.state))
                })
                .cloned()
                .collect(),
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
    reservation_barrier: Mutex<Option<Arc<tokio::sync::Barrier>>>,
}

type OperationKey = (String, String, String);

struct OperationEntry {
    claim_id: String,
    fingerprint: [u8; 32],
    outcome: Option<Vec<u8>>,
    lease_expires_at: std::time::SystemTime,
    recovery_required: bool,
    outcome_expires_at: Option<std::time::SystemTime>,
}

impl MemoryOperations {
    fn synchronize_next_reservations(&self, participants: usize) {
        *self.reservation_barrier.lock().unwrap() =
            Some(Arc::new(tokio::sync::Barrier::new(participants)));
    }

    fn clear_reservation_barrier(&self) {
        *self.reservation_barrier.lock().unwrap() = None;
    }
}

#[async_trait]
impl OperationStore for MemoryOperations {
    async fn reserve(&self, claim: &OperationClaim) -> Result<OperationReservation, StoreError> {
        let reservation_barrier = self.reservation_barrier.lock().unwrap().clone();
        if let Some(barrier) = reservation_barrier.as_ref() {
            barrier.wait().await;
        }
        let key = (
            claim.subject().0.clone(),
            claim.operation().to_string(),
            claim.key().to_string(),
        );
        let reservation = {
            let mut entries = self.entries.lock().unwrap();
            let now = std::time::SystemTime::now();
            if entries.get(&key).is_some_and(|entry| {
                entry.outcome.is_some()
                    && entry
                        .outcome_expires_at
                        .is_some_and(|expires_at| expires_at <= now)
            }) {
                entries.remove(&key);
            }
            match entries.get_mut(&key) {
                Some(entry) if &entry.fingerprint != claim.fingerprint() => {
                    OperationReservation::FingerprintMismatch
                }
                Some(entry) if entry.outcome.is_some() => OperationReservation::Complete(
                    entry.outcome.as_ref().expect("checked above").clone(),
                ),
                Some(entry) if entry.recovery_required => OperationReservation::RecoveryRequired,
                Some(entry) if entry.lease_expires_at <= now => {
                    entry.recovery_required = true;
                    OperationReservation::RecoveryRequired
                }
                Some(_) => OperationReservation::Pending,
                None => {
                    entries.insert(
                        key,
                        OperationEntry {
                            claim_id: claim.claim_id().to_string(),
                            fingerprint: *claim.fingerprint(),
                            outcome: None,
                            lease_expires_at: claim.lease_expires_at(),
                            recovery_required: false,
                            outcome_expires_at: None,
                        },
                    );
                    self.writes.fetch_add(1, Ordering::SeqCst);
                    OperationReservation::Acquired
                }
            }
        };
        if let Some(barrier) = reservation_barrier {
            barrier.wait().await;
        }
        Ok(reservation)
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
        if entry.claim_id != claim.claim_id() {
            return Err(StoreError::Malformed(
                "operation claim was replaced before completion".into(),
            ));
        }
        entry.outcome = Some(outcome);
        entry.recovery_required = false;
        entry.outcome_expires_at = Some(
            std::time::SystemTime::now()
                .checked_add(claim.outcome_retention())
                .ok_or_else(|| {
                    StoreError::Malformed("operation outcome retention overflowed".into())
                })?,
        );
        self.writes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

fn review_admission(launch: &Launch, now: &str) -> LaunchReviewAdmission {
    LaunchReviewAdmission {
        pricing: PricingSnapshot {
            policy_reference: launch
                .revision
                .content
                .commercial_policy
                .policy_reference
                .clone(),
            billing_mode: BillingMode::ZeroCost,
            rate_card: None,
            rate: None,
            currency: layer2_core::CURRENCY.into(),
            cpu_millis: launch.effective.resource_size.cpu_millis(),
            memory_mib: launch.effective.resource_size.memory_mib(),
            micros_per_hour: 0,
            estimated_micros: 0,
            quoted_at: now.into(),
            expires_at: launch.draft_expires_at.clone(),
        },
        runtime: RuntimeAdmissionSnapshot {
            kernel: layer2_core::KernelId::new("codex-trigger-native-v1"),
            runtime_version: "cma-trigger-test-runtime@sha256:admitted".into(),
            supported_capabilities: launch
                .revision
                .content
                .session
                .required_capabilities
                .clone(),
            observed_at: now.into(),
        },
        quota: QuotaSnapshot {
            live_sessions: 0,
            live_session_limit: Some(4),
            reserved_cpu_millis: 0,
            reserved_memory_mib: 0,
            free_cpu_millis: Some(4_000),
            free_memory_mib: Some(8_192),
            checked_at: now.into(),
        },
    }
}

struct StaticAdmission;

#[async_trait]
impl TriggerLaunchAdmission for StaticAdmission {
    async fn review(
        &self,
        _owner: &SubjectId,
        launch: &Launch,
        now: &str,
    ) -> Result<LaunchReviewAdmission, TriggerAdmissionError> {
        Ok(review_admission(launch, now))
    }

    async fn start(
        &self,
        _owner: &SubjectId,
        launch: &Launch,
        now: &str,
    ) -> Result<StartAdmissionSnapshot, TriggerAdmissionError> {
        let pricing = launch
            .pricing
            .as_ref()
            .ok_or_else(|| TriggerAdmissionError::PricingUnavailable("missing quote".into()))?;
        let runtime = launch.runtime_admission.as_ref().ok_or_else(|| {
            TriggerAdmissionError::CapabilityUnsupported {
                capability: "runtime_admission".into(),
            }
        })?;
        Ok(StartAdmissionSnapshot {
            runtime_version: runtime.runtime_version.clone(),
            pricing_policy_reference: pricing.policy_reference.clone(),
            rate_card: pricing.rate_card.clone(),
            quota: review_admission(launch, now).quota,
            checked_at: now.into(),
        })
    }
}

struct StaticAuthority {
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl TriggerAuthorityVerifier for StaticAuthority {
    async fn verify_publisher(
        &self,
        subject: &SubjectId,
        content: &TriggerRevisionContent,
        selected_service_id: &str,
    ) -> Result<PublisherAuthority, TriggerServiceError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
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
        self.calls.fetch_add(1, Ordering::SeqCst);
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

    async fn resolve_preview_repository_policy(
        &self,
        revision: &TriggerRevision,
    ) -> Result<layer2_core::TriggerPreviewRepositoryPolicy, TriggerServiceError> {
        Ok(layer2_core::TriggerPreviewRepositoryPolicy {
            repository: revision.content.upstream.repository.clone(),
            public_forkable: true,
        })
    }
}

struct StaticConnections {
    calls: Arc<AtomicUsize>,
}

struct RevokingConnections {
    owner: SubjectId,
    connection: Mutex<TriggerConnection>,
    list_calls: AtomicUsize,
    revocation_lookup_calls: AtomicUsize,
    revoke_calls: AtomicUsize,
    return_mismatched_service: bool,
}

impl RevokingConnections {
    fn new(return_mismatched_service: bool) -> Self {
        Self {
            owner: SubjectId(OWNER.into()),
            connection: Mutex::new(TriggerConnection {
                service_id: "service-exact".into(),
                state_version: 7,
                state: ConnectionState::Active,
                account: Some(ConnectedAccount {
                    account_id: 21,
                    account_node_id: "U_contributor".into(),
                    login: OWNER.into(),
                    avatar_url: None,
                }),
                granted_scopes: vec!["public_repo".into()],
                consent_observed_at: Some(NOW.into()),
            }),
            list_calls: AtomicUsize::new(0),
            revocation_lookup_calls: AtomicUsize::new(0),
            revoke_calls: AtomicUsize::new(0),
            return_mismatched_service,
        }
    }
}

#[async_trait]
impl TriggerConnectionManager for RevokingConnections {
    async fn list_connections(
        &self,
        _subject: &SubjectId,
    ) -> Result<Vec<TriggerConnection>, TriggerServiceError> {
        self.list_calls.fetch_add(1, Ordering::SeqCst);
        Err(TriggerServiceError::NotFound)
    }

    async fn connection_for_revocation(
        &self,
        subject: &SubjectId,
        service_id: &str,
    ) -> Result<Option<TriggerConnection>, TriggerServiceError> {
        self.revocation_lookup_calls.fetch_add(1, Ordering::SeqCst);
        let connection = self.connection.lock().unwrap();
        Ok(
            (subject == &self.owner && connection.service_id == service_id)
                .then(|| connection.clone()),
        )
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

    async fn revoke_connection(
        &self,
        subject: &SubjectId,
        service_id: &str,
    ) -> Result<TriggerConnection, TriggerServiceError> {
        if subject != &self.owner || self.connection.lock().unwrap().service_id != service_id {
            return Err(TriggerServiceError::NotFound);
        }
        self.revoke_calls.fetch_add(1, Ordering::SeqCst);
        let mut connection = self.connection.lock().unwrap();
        if connection.state != ConnectionState::Inactive {
            connection.state_version += 1;
            connection.state = ConnectionState::Inactive;
        }
        let mut result = connection.clone();
        if self.return_mismatched_service {
            result.service_id = "service-other".into();
        }
        Ok(result)
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
        _owner: &str,
        _name: &str,
    ) -> Result<TriggerRepositorySelection, TriggerServiceError> {
        Err(TriggerServiceError::NotFound)
    }
}

#[async_trait]
impl TriggerConnectionManager for StaticConnections {
    async fn list_connections(
        &self,
        _subject: &SubjectId,
    ) -> Result<Vec<TriggerConnection>, TriggerServiceError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(Vec::new())
    }

    async fn begin_connection(
        &self,
        _subject: &SubjectId,
        _intent_id: &str,
    ) -> Result<TriggerConnectionAuthorization, TriggerServiceError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(TriggerServiceError::NotFound)
    }

    async fn reconnect(
        &self,
        _subject: &SubjectId,
        _service_id: &str,
    ) -> Result<TriggerConnectionAuthorization, TriggerServiceError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(TriggerServiceError::NotFound)
    }

    async fn revoke_connection(
        &self,
        _subject: &SubjectId,
        _service_id: &str,
    ) -> Result<TriggerConnection, TriggerServiceError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(TriggerServiceError::NotFound)
    }

    async fn preflight_connection(
        &self,
        _subject: &SubjectId,
        _service_id: &str,
    ) -> Result<TriggerConnection, TriggerServiceError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(TriggerServiceError::NotFound)
    }

    async fn resolve_repository(
        &self,
        _subject: &SubjectId,
        _service_id: &str,
        _repository_id: u64,
        _selected_ref: &str,
    ) -> Result<TriggerRepositorySelection, TriggerServiceError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(TriggerServiceError::NotFound)
    }

    async fn resolve_repository_locator(
        &self,
        _subject: &SubjectId,
        _service_id: &str,
        owner: &str,
        name: &str,
    ) -> Result<TriggerRepositorySelection, TriggerServiceError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
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
            sponsorship: None,
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
        github_catalog_service_id: Some("catalog-github".into()),
        github_user_service_slug: Some("api-github-2".into()),
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
        github_catalog_service_id: Some("catalog-github".into()),
        github_user_service_slug: Some("api-github-2".into()),
        service_state_version: 2,
        backing_key_id: "backing-key".into(),
        session_credential_authority: None,
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
