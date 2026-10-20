use super::*;
use crate::agent_v2::fixtures::*;
use axum::http::{Method, StatusCode};
use futures_util::StreamExt;
use layer2_core::{
    resource_access::*,
    sandbox::{AgentLiveFragmentKind, AgentLogKind},
    SubjectId,
};
use tower::ServiceExt;

async fn share(f: &Fixture) -> ResourceShare {
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
                idempotency_key: "replay-share".into(),
            },
        )
        .await
        .unwrap()
        .share
}

#[tokio::test]
async fn revoked_shared_authority_discards_frames_waiting_under_backpressure() {
    let f = fixture(true, true).await;
    let share = share(&f).await;
    let response = f
        .router
        .clone()
        .oneshot(request(
            Method::GET,
            &format!("{EVENTS}?follow=false"),
            "reader",
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body().into_data_stream();
    let first = body.next().await.unwrap().unwrap();
    assert!(std::str::from_utf8(&first)
        .unwrap()
        .contains("\"kind\":null"));
    f.access
        .change_share(
            &context("owner-a"),
            None,
            ShareCommand {
                selector: share.selector,
                change: ShareChange::Revoke { share_id: share.id },
                expected_share_revision: Some(share.revision),
                idempotency_key: "revoke-replay".into(),
            },
        )
        .await
        .unwrap();
    assert!(body.next().await.is_none());
    let (status, _) = call(&f.router, Method::GET, SNAPSHOT, "owner-a", None, None).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test(start_paused = true)]
async fn buffered_shared_replay_does_not_renew_expired_membership() {
    let f = fixture(true, true).await;
    share(&f).await;
    let response = f
        .router
        .oneshot(request(Method::GET, EVENTS, "reader", None, None))
        .await
        .unwrap();
    let mut body = response.into_body().into_data_stream();
    assert!(body.next().await.is_some());
    tokio::time::advance(Duration::from_secs(60)).await;
    assert!(body.next().await.is_none());
}

async fn following() -> (Following, Fixture) {
    let f = fixture(true, true).await;
    let auth = AuthenticatedResourceCaller {
        context: context("owner-a"),
        credential: None,
    };
    let mut open = open(
        f.state.clone(),
        auth,
        AgentId("agt_fixture".into()),
        None,
        HeaderMap::new(),
    )
    .await
    .unwrap();
    open.pending.clear();
    open.initial_gap = false;
    (open, f)
}
fn fragment(open: &Following) -> AgentLiveFragment {
    AgentLiveFragment {
        agent_id: open.header.id.clone(),
        runtime_generation: open.header.runtime_generation.unwrap(),
        turn_sequence: open.header.active_turn.unwrap(),
        after_seq: open.header.head,
        message_id: "message-live".into(),
        kind: AgentLiveFragmentKind::Message,
        delta: "Live text".into(),
    }
}

#[tokio::test]
async fn live_fragments_require_exact_runtime_turn_and_caught_up_anchor() {
    let (mut open, _) = following().await;
    let valid = fragment(&open);
    for fault in ["foreign", "runtime", "turn", "committed", "future"] {
        let mut value = valid.clone();
        match fault {
            "foreign" => value.agent_id = AgentId("agt_other".into()),
            "runtime" => value.runtime_generation += 1,
            "turn" => value.turn_sequence += 1,
            "committed" => {
                open.committed_message = value.after_seq;
                value.after_seq -= 1;
            }
            "future" => value.after_seq += 1,
            _ => unreachable!(),
        }
        open.held = Some(value);
        open.queue_live();
        assert!(open.pending.is_empty(), "{fault}");
        assert_eq!(open.held.is_some(), fault == "future");
    }
    open.held = Some(valid);
    open.queue_live();
    assert_eq!(open.pending.len(), 1);
    assert!(open.pending[0].live);
}

#[tokio::test]
async fn live_observation_time_is_real_and_negotiated_without_changing_old_frames() {
    for preferred in [false, true] {
        let (mut open, _) = following().await;
        open.live_timestamps = preferred;
        let before = layer2_core::now_rfc3339();
        open.held = Some(fragment(&open));
        open.queue_live();
        let after = layer2_core::now_rfc3339();
        let mut body = response(open).into_body().into_data_stream();
        let bytes = body.next().await.unwrap().unwrap();
        let wire = std::str::from_utf8(&bytes).unwrap();
        let data = wire
            .lines()
            .find_map(|line| line.strip_prefix("data: "))
            .unwrap();
        let value: serde_json::Value = serde_json::from_str(data).unwrap();
        assert_eq!(value["delta"], "Live text");
        if preferred {
            let at = value["observed_at"].as_str().unwrap();
            assert!(at >= before.as_str() && at <= after.as_str());
        } else {
            assert!(value.get("observed_at").is_none());
        }
    }
    for (header, expected) in [
        ("cma-live-observed-at", true),
        ("cma-message-bindings, CMA-LIVE-OBSERVED-AT", true),
        ("cma-live-observed-at=true", false),
        ("other-cma-live-observed-at", false),
    ] {
        let mut headers = HeaderMap::new();
        headers.insert("prefer", header.parse().unwrap());
        assert_eq!(projection::live_timestamps(&headers), expected);
    }
}

#[tokio::test]
async fn buffered_live_text_is_discarded_when_a_commit_arrives_before_release() {
    let (mut open, f) = following().await;
    open.held = Some(fragment(&open));
    open.queue_live();
    let frame = open.pending.pop_front().unwrap();
    append(
        &f.store,
        AgentLogKind::AgentMessage {
            content: "Committed assistant text".into(),
        },
    )
    .await;
    assert!(!open.release(&frame).await);
    open.fill().await.unwrap();
    assert_eq!(open.pending.len(), 1);
    assert!(!open.pending[0].live);
    open.pending.clear();
    open.fill().await.unwrap();
    assert_eq!(
        open.pending.len(),
        1,
        "discarded fragments produce an explicit gap after durable catch-up"
    );
    assert!(open.pending[0].live && !open.pending[0].fragment);
}

#[tokio::test]
async fn serialized_pages_stop_before_byte_limit_without_acknowledging_omitted_rows() {
    let f = fixture(false, false).await;
    for _ in 0..5 {
        append(
            &f.store,
            AgentLogKind::AgentMessage {
                content: "a".repeat(1024 * 1024),
            },
        )
        .await;
    }
    let (status, page) = call(&f.router, Method::GET, SNAPSHOT, "owner-a", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(serde_json::to_vec(&page).unwrap().len() <= projection::MAX_BYTES);
    let items = page["items"].as_array().unwrap();
    assert_eq!(items.last().unwrap()["sequence"], 5);
    let (status, next) = call(
        &f.router,
        Method::GET,
        &format!(
            "{SNAPSHOT}?page_cursor={}",
            page["next_page_cursor"].as_str().unwrap()
        ),
        "owner-a",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(next["items"][0]["sequence"], 6);
    assert!(next["next_page_cursor"].is_null());
}
