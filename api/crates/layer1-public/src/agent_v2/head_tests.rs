use super::{cursor, fixtures::*, head, snapshot};
use crate::AuthenticatedResourceCaller;
use axum::{
    body::to_bytes,
    http::{Method, StatusCode},
};
use layer2_core::{
    resource_access::*,
    sandbox::{
        store::AgentStore, AgentEventStreamAuthorization, AgentEventStreamHeader, AgentLogKind,
    },
    AgentId, SubjectId,
};
use serde_json::Value;
use tower::ServiceExt;

const HEAD: &str = "/api/v2/agents/agt_fixture/events/head";

async fn captured(
    f: &Fixture,
    subject: &str,
) -> (AgentEventStreamAuthorization, AgentEventStreamHeader) {
    let auth = AuthenticatedResourceCaller {
        context: context(subject),
        credential: Some(crate::DelegatedCredential::new("fixture-credential".into())),
    };
    let held = snapshot::authorize(&f.state, &auth, &AgentId("agt_fixture".into()))
        .await
        .unwrap();
    let header = f.state.reads.event_stream_header(&held).await.unwrap();
    (held, header)
}

async fn set_active(f: &Fixture, turn: Option<u64>) {
    let mut agent = f
        .store
        .get_agent(&AgentId("agt_fixture".into()))
        .await
        .unwrap()
        .unwrap();
    let revision = agent.revision;
    agent.conversation.active_turn = turn;
    agent.status = if turn.is_some() {
        layer2_core::sandbox::AgentStatus::Working
    } else {
        layer2_core::sandbox::AgentStatus::Ready
    };
    agent.revision += 1;
    f.store.save_agent(revision, agent).await.unwrap();
}

#[tokio::test]
async fn idle_head_does_not_read_or_bind_historical_turns() {
    let f = fixture(false, false).await;
    append(
        &f.store,
        AgentLogKind::TurnStarted {
            model: None,
            turn_sequence: 3,
            origin_key: Some("command:cmd_missing_historical".into()),
        },
    )
    .await;
    append(&f.store, AgentLogKind::TurnCompleted { turn_sequence: 3 }).await;
    let response = f
        .router
        .clone()
        .oneshot(request(Method::GET, HEAD, "owner-a", None, None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["cache-control"], "private, no-store");
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(body["object"], "cma.agent.event_head");
    assert_eq!(body["schema_version"], 1);
    assert_eq!(body["agent_id"], "agt_fixture");
    assert!(body["active_turn"].is_null());
    assert_eq!(
        body["through_cursor"],
        cursor::event(body["incarnation"].as_str().unwrap(), 4)
    );
    // A rich replay still rejects the corrupt old binding; head never walks it.
    assert_eq!(
        call(&f.router, Method::GET, SNAPSHOT, "owner-a", None, None)
            .await
            .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
}

#[tokio::test]
async fn active_head_preserves_exact_verified_turn_without_private_metadata() {
    let f = fixture(true, false).await;
    let (_, header) = captured(&f, "owner-a").await;
    let (status, body) = call(&f.router, Method::GET, HEAD, "owner-a", None, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let turn = &body["active_turn"];
    assert_eq!(turn["sequence"], header.active_turn.unwrap());
    assert_eq!(turn["kind"]["turn_sequence"], turn["sequence"]);
    assert_eq!(turn["kind"]["type"], "turn_started");
    assert_eq!(turn["response_binding"]["command_id"], "cmd_control_source");
    assert_eq!(
        turn["response_binding"]["response_id"],
        "resp_control_source"
    );
    assert_eq!(turn["response_binding"]["turn_sequence"], turn["sequence"]);
    assert_eq!(
        body["through_cursor"],
        cursor::event(&cursor::incarnation(&header), header.head)
    );
    for private in [
        "origin_key",
        "runtime_locator",
        "private-source-reference",
        "private-correlation",
        "fixture-custody",
    ] {
        assert!(!body.to_string().contains(private));
    }
}

#[tokio::test]
async fn legacy_active_head_remains_unbound_but_missing_common_command_fails_closed() {
    for (origin, expected) in [
        ("legacy-key", StatusCode::OK),
        ("command:cmd_missing", StatusCode::SERVICE_UNAVAILABLE),
    ] {
        let f = fixture(false, false).await;
        append(
            &f.store,
            AgentLogKind::TurnStarted {
                model: None,
                turn_sequence: 3,
                origin_key: Some(origin.into()),
            },
        )
        .await;
        set_active(&f, Some(3)).await;
        let (status, body) = call(&f.router, Method::GET, HEAD, "owner-a", None, None).await;
        assert_eq!(status, expected, "{body}");
        if status.is_success() {
            assert!(body["active_turn"]["response_binding"].is_null());
            assert_eq!(body["active_turn"]["sequence"], 3);
        }
    }
}

#[tokio::test]
async fn raced_completion_and_new_turn_keep_captured_baseline_and_replay_once() {
    let f = fixture(true, false).await;
    let (held, header) = captured(&f, "owner-a").await;
    let old_head = header.head;
    let old_turn = header.active_turn.unwrap();
    append(
        &f.store,
        AgentLogKind::TurnCompleted {
            turn_sequence: old_turn,
        },
    )
    .await;
    append(
        &f.store,
        AgentLogKind::TurnStarted {
            model: None,
            turn_sequence: old_head + 2,
            origin_key: None,
        },
    )
    .await;
    set_active(&f, Some(old_head + 2)).await;
    let baseline = head::project(&f.state, &held, header).await.unwrap();
    assert_eq!(baseline.active_turn.unwrap().sequence, old_turn);
    assert_eq!(
        baseline.through_cursor,
        cursor::event(&baseline.incarnation, old_head)
    );
    let response = f
        .router
        .clone()
        .oneshot(request(
            Method::GET,
            &format!(
                "{EVENTS}?follow=false&after_cursor={}",
                baseline.through_cursor
            ),
            "owner-a",
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), 8192).await.unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    assert_eq!(text.matches("event: cma.agent.event").count(), 2);
    let events: Vec<Value> = text
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(events[0]["sequence"], old_head + 1);
    assert_eq!(events[0]["kind"]["type"], "turn_completed");
    assert_eq!(events[1]["sequence"], old_head + 2);
    assert_eq!(events[1]["kind"]["type"], "turn_started");
}

#[tokio::test]
async fn invalid_active_pointer_is_not_published_as_a_live_baseline() {
    for turn in [0, 1, 999] {
        let f = fixture(false, false).await;
        set_active(&f, Some(turn)).await;
        let (status, body) = call(&f.router, Method::GET, HEAD, "owner-a", None, None).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
        assert!(matches!(
            body["code"].as_str(),
            Some("agent_history_unavailable" | "agent_read_unavailable")
        ));
    }
}

pub(super) async fn share(f: &Fixture) -> ResourceShare {
    f.access
        .change_share(
            &context("owner-a"),
            Some("fixture-credential"),
            ShareCommand {
                selector: ResourceSelector::Agent {
                    agent_id: AgentId("agt_fixture".into()),
                },
                change: ShareChange::Create {
                    target_org_id: SubjectId::new("org"),
                    mode: ShareMode::View,
                },
                expected_share_revision: None,
                idempotency_key: "head-share".into(),
            },
        )
        .await
        .unwrap()
        .share
}

#[tokio::test]
async fn shared_head_uses_current_view_history_authority_and_refuses_revocation_before_publication()
{
    for active in [false, true] {
        let f = fixture(active, false).await;
        let share = share(&f).await;
        let (status, body) = call(&f.router, Method::GET, HEAD, "reader", None, None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["active_turn"].is_null(), !active);
        let (held, header) = captured(&f, "reader").await;
        f.access
            .change_share(
                &context("owner-a"),
                None,
                ShareCommand {
                    selector: share.selector,
                    change: ShareChange::Revoke { share_id: share.id },
                    expected_share_revision: Some(share.revision),
                    idempotency_key: "revoke-head".into(),
                },
            )
            .await
            .unwrap();
        assert!(head::project(&f.state, &held, header).await.is_err());
        assert_eq!(
            call(&f.router, Method::GET, HEAD, "reader", None, None)
                .await
                .0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            call(&f.router, Method::GET, HEAD, "owner-a", None, None)
                .await
                .0,
            StatusCode::OK
        );
    }
}

#[tokio::test(start_paused = true)]
async fn expired_membership_cannot_publish_even_an_idle_baseline() {
    let f = fixture(false, false).await;
    share(&f).await;
    let (held, header) = captured(&f, "reader").await;
    tokio::time::advance(std::time::Duration::from_secs(60)).await;
    assert!(head::project(&f.state, &held, header).await.is_err());
}

#[tokio::test]
async fn head_rejects_foreign_missing_unauthenticated_and_caller_selected_coordinates() {
    let f = fixture(false, false).await;
    for (path, subject, expected) in [
        (HEAD.to_owned(), "foreign", StatusCode::NOT_FOUND),
        (
            HEAD.replace("agt_fixture", "agt_missing"),
            "owner-a",
            StatusCode::NOT_FOUND,
        ),
        (
            format!("{HEAD}?after_cursor=1"),
            "owner-a",
            StatusCode::BAD_REQUEST,
        ),
        (
            format!("{HEAD}?limit=1"),
            "owner-a",
            StatusCode::BAD_REQUEST,
        ),
    ] {
        assert_eq!(
            call(&f.router, Method::GET, &path, subject, None, None)
                .await
                .0,
            expected
        );
    }
    let request = axum::http::Request::builder()
        .uri(HEAD)
        .body(axum::body::Body::empty())
        .unwrap();
    assert_eq!(
        f.router.oneshot(request).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn recreated_agent_cannot_publish_an_old_captured_baseline() {
    let f = fixture(false, false).await;
    let (held, header) = captured(&f, "owner-a").await;
    let mut agent = f
        .store
        .get_agent(&AgentId("agt_fixture".into()))
        .await
        .unwrap()
        .unwrap();
    let revision = agent.revision;
    agent.created_at = "2026-09-06T00:00:00Z".into();
    agent.revision += 1;
    f.store.save_agent(revision, agent).await.unwrap();
    assert!(head::project(&f.state, &held, header).await.is_err());
}
