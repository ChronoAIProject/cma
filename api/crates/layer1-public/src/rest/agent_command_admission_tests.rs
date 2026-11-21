use super::*;
use crate::rest::{agents::agent_router, agents_tests, resource_access_tests::context};
use axum::http::{Method, StatusCode};
use layer2_core::resource_access::*;
use layer2_core::sandbox::{
    agent_io::*,
    store::{AgentLogStore, AgentStore, InMemorySandboxStore, PageRequest},
    AgentLogKind, AgentStatus,
};
use layer2_core::{
    ids::{AgentCommandId, AgentResponseId},
    Caller, OperationClaim, OperationReservation, OperationStore, SandboxId, StoreError, SubjectId,
};
use serde_json::{json, Value};
use std::{
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, SystemTime},
};

#[path = "agent_command_admission_tests/fixtures.rs"]
mod fixtures;
use fixtures::*;
#[path = "agent_command_admission_tests/contextual_suggestions.rs"]
mod contextual_suggestions;

#[path = "agent_command_admission_tests/lookup_failures.rs"]
mod lookup_failures;

#[tokio::test]
async fn shared_admin_routes_admit_once_with_actual_actor_and_bound_control_turn() {
    let test = fixture().await;
    let before = test
        .store
        .agent_log(&AgentId("agt_common".into()), 0, 30)
        .await
        .unwrap();
    for (route, body) in [
        ("input", Some(json!({"text":"review"}))),
        ("steer", Some(json!({"text":"focus"}))),
        ("interrupt", None),
        ("approvals/approval-one", Some(json!({"decision":"allow"}))),
    ] {
        let (status, first) =
            agents_tests::response(&test.router, request("admin", route, "one", body.clone()))
                .await;
        assert_eq!(
            status,
            StatusCode::ACCEPTED,
            "{route}: {}",
            String::from_utf8_lossy(&first)
        );
        assert_eq!(
            first.as_ref(),
            if route == "input" {
                br#"{"queued":true}"#.as_slice()
            } else {
                b""
            }
        );
        let retry =
            agents_tests::response(&test.router, request("admin", route, "one", body)).await;
        assert_eq!(retry, (status, first));
    }
    let records = records(&test).await;
    assert_eq!(records.len(), 4);
    assert_eq!(test.commands.reserves.load(Ordering::SeqCst), 4);
    for record in records {
        assert_eq!(record.receipt.envelope.actor, context("admin").actor);
        assert_eq!(
            record.receipt.envelope.owner,
            AgentOwner::Person {
                subject: SubjectId::new("owner-a")
            }
        );
        assert_eq!(record.receipt.envelope.source.kind, AgentSourceKind::Api);
        assert_eq!(
            record.target_turn,
            if matches!(
                record.receipt.envelope.command,
                AgentCommand::Message { .. }
            ) {
                None
            } else {
                Some(3)
            }
        );
    }
    assert_eq!(
        test.store
            .agent_log(&AgentId("agt_common".into()), 0, 30)
            .await
            .unwrap(),
        before
    );
}

#[tokio::test]
async fn current_roles_and_share_revocation_block_new_commands_and_receipt_recovery() {
    let test = fixture().await;
    let body = Some(json!({"text":"review"}));
    assert_eq!(
        agents_tests::response(&test.router, request("admin", "input", "one", body.clone()))
            .await
            .0,
        StatusCode::ACCEPTED
    );
    for role in [OrganizationRole::Member, OrganizationRole::Viewer] {
        *test.authority.membership.lock().unwrap() = Ok(role);
        for key in ["one", "new"] {
            assert_ne!(
                agents_tests::response(&test.router, request("admin", "input", key, body.clone()))
                    .await
                    .0,
                StatusCode::ACCEPTED
            );
        }
    }
    *test.authority.membership.lock().unwrap() = Ok(OrganizationRole::Admin);
    test.access
        .change_share(
            &context("owner-a"),
            Some("request-credential"),
            ShareCommand {
                selector: test.grant.selector.clone(),
                change: ShareChange::Revoke {
                    share_id: test.grant.id.clone(),
                },
                expected_share_revision: Some(test.grant.revision),
                idempotency_key: "revoke".into(),
            },
        )
        .await
        .unwrap();
    assert_ne!(
        agents_tests::response(&test.router, request("admin", "input", "one", body.clone()))
            .await
            .0,
        StatusCode::ACCEPTED
    );
    assert_eq!(
        agents_tests::response(&test.router, request("owner-a", "input", "owner", body))
            .await
            .0,
        StatusCode::ACCEPTED
    );
    assert_eq!(test.commands.reserves.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn lost_outer_completion_recovers_only_the_existing_receipt_for_pending_and_expired_claims() {
    for expired in [false, true] {
        let test = fixture().await;
        test.outer.fail_complete.store(true, Ordering::SeqCst);
        test.outer.recovery.store(expired, Ordering::SeqCst);
        let body = Some(json!({"text":"one"}));
        assert_eq!(
            agents_tests::response(&test.router, request("admin", "input", "one", body.clone()))
                .await
                .0,
            StatusCode::SERVICE_UNAVAILABLE
        );
        let original = records(&test).await;
        test.outer.fail_complete.store(false, Ordering::SeqCst);
        let (status, recovered) =
            agents_tests::json_response(&test.router, request("admin", "input", "one", body)).await;
        assert_eq!(
            (status, recovered),
            (StatusCode::ACCEPTED, json!({"queued":true}))
        );
        assert_eq!(records(&test).await, original);
        assert_eq!(test.commands.reserves.load(Ordering::SeqCst), 1);
        let (status, error) = agents_tests::json_response(
            &test.router,
            request("admin", "input", "one", Some(json!({"text":"different"}))),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(error["code"], "idempotency_key_reused");
    }
}

#[tokio::test]
async fn unknown_common_commit_recovers_without_a_second_reservation() {
    let test = fixture().await;
    test.commands.unknown.store(true, Ordering::SeqCst);
    let body = Some(json!({"text":"one"}));
    assert_eq!(
        agents_tests::response(&test.router, request("admin", "input", "one", body.clone()))
            .await
            .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    let original = records(&test).await;
    assert_eq!(
        agents_tests::json_response(&test.router, request("admin", "input", "one", body)).await,
        (StatusCode::ACCEPTED, json!({"queued":true}))
    );
    assert_eq!(records(&test).await, original);
    assert_eq!(test.commands.reserves.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn ambiguous_pre_migration_claim_never_creates_a_common_receipt() {
    for expired in [false, true] {
        let test = fixture().await;
        let body = json!({"text":"old input"});
        assert_eq!(
            test.outer
                .reserve(&outer_claim("old", &body))
                .await
                .unwrap(),
            OperationReservation::Acquired
        );
        test.outer.recovery.store(expired, Ordering::SeqCst);
        let (status, error) =
            agents_tests::json_response(&test.router, request("admin", "input", "old", Some(body)))
                .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(
            error["code"],
            if expired {
                "idempotency_recovery_required"
            } else {
                "idempotency_in_flight"
            }
        );
        assert!(records(&test).await.is_empty());
        assert_eq!(test.commands.reserves.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn previous_completed_receipt_replays_its_original_status_headers_and_body_verbatim() {
    use tower::ServiceExt;
    let test = fixture().await;
    let body = json!({"text":"old input"});
    let claim = outer_claim("old", &body);
    test.outer.reserve(&claim).await.unwrap();
    let original = br#"{"queued":false}"#;
    test.outer.complete(&claim, serde_json::to_vec(&json!({
        "status":202, "headers":[["content-type", b"application/json".to_vec()], ["x-original", b"legacy".to_vec()]], "body": original.to_vec()
    })).unwrap()).await.unwrap();
    test.authority.custody_revoked.store(true, Ordering::SeqCst);
    let response = test
        .router
        .clone()
        .oneshot(request("admin", "input", "old", Some(body)))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    assert_eq!(response.headers()["x-original"], "legacy");
    assert_eq!(
        axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap()
            .as_ref(),
        original
    );
    assert!(records(&test).await.is_empty());
}

#[tokio::test]
async fn common_and_legacy_keys_are_distinct_and_human_source_cannot_become_channel() {
    let test = fixture().await;
    let body = Some(json!({"text":"one"}));
    let mut human = request("admin", "input", "one", body);
    let mut ctx = context("admin");
    ctx.actor = AgentActor::Human {
        subject: ctx.caller.subject.clone(),
    };
    human.extensions_mut().insert(ctx.clone());
    assert_eq!(
        agents_tests::response(&test.router, human).await.0,
        StatusCode::ACCEPTED
    );
    test.service
        .admit(
            &ctx,
            "request-credential",
            CommandSubmissionSource::Api,
            CommandSubmission {
                agent_id: AgentId("agt_common".into()),
                command: AgentCommand::message("one"),
                idempotency_key: "one".into(),
                expected_agent_revision: None,
                requested_model: None,
            },
        )
        .await
        .unwrap();
    let records = records(&test).await;
    assert_eq!(records.len(), 2);
    assert!(records
        .iter()
        .all(|r| r.receipt.envelope.actor == ctx.actor));
    assert!(records
        .iter()
        .any(|r| r.receipt.envelope.source.kind == AgentSourceKind::UserInterface));
    assert!(records
        .iter()
        .all(|r| r.receipt.envelope.source.kind != AgentSourceKind::Channel));
    assert_ne!(records[0].key_sha256, records[1].key_sha256);
}

#[tokio::test]
async fn absent_resource_context_cannot_fall_back_to_legacy_execution_in_production_composition() {
    let test = fixture().await;
    let old = agents_tests::request(
        Method::POST,
        "/api/v1/agents/agt_common/input",
        "owner-a",
        Some("one"),
        Some(json!({"text":"one"})),
    );
    assert_eq!(
        agents_tests::response(&test.router, old).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert!(records(&test).await.is_empty());
}

#[tokio::test]
async fn legacy_raw_body_fingerprint_remains_stricter_than_canonical_common_input() {
    let test = fixture().await;
    assert_eq!(
        agents_tests::response(
            &test.router,
            request("admin", "input", "same", Some(json!({"text":"one"})))
        )
        .await
        .0,
        StatusCode::ACCEPTED
    );
    let mut retry = request("admin", "input", "same", Some(json!({"text":"one"})));
    *retry.body_mut() = axum::body::Body::from("{ \"text\" : \"one\" }");
    let (status, problem) = agents_tests::json_response(&test.router, retry).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(problem["code"], "idempotency_key_reused");
    assert_eq!(test.commands.reserves.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn controls_without_an_active_turn_are_refused_and_cannot_target_a_future_turn() {
    let test = fixture().await;
    let mut agent = test
        .store
        .get_agent(&AgentId("agt_common".into()))
        .await
        .unwrap()
        .unwrap();
    let revision = agent.revision;
    agent.revision += 1;
    agent.status = AgentStatus::Ready;
    agent.conversation.active_turn = None;
    test.store.save_agent(revision, agent).await.unwrap();
    assert_eq!(
        agents_tests::response(&test.router, request("admin", "interrupt", "none", None))
            .await
            .0,
        StatusCode::CONFLICT
    );
    assert!(records(&test).await.is_empty());
}

#[tokio::test]
async fn historical_validation_receipt_precedes_current_input_validation() {
    let test = fixture().await;
    let body = json!({"text":""});
    let claim = outer_claim("invalid", &body);
    test.outer.reserve(&claim).await.unwrap();
    test.outer
        .complete(
            &claim,
            serde_json::to_vec(&json!({
                "status":422, "headers":[["content-type", b"application/json".to_vec()]],
                "body": br#"{"code":"original_validation_error"}"#.to_vec()
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    let (status, result) = agents_tests::json_response(
        &test.router,
        request("admin", "input", "invalid", Some(body)),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(result["code"], "original_validation_error");
    assert_eq!(test.commands.reserves.load(Ordering::SeqCst), 0);
}
