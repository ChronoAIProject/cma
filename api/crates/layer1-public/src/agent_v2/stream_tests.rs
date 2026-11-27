use super::*;
use crate::agent_v2::fixtures::*;
use axum::http::{Method, StatusCode};
use futures_util::StreamExt;
use layer2_core::sandbox::AgentLiveFragment;
use layer2_core::{
    resource_access::*,
    sandbox::{
        store::AgentStore, AgentLiveFragmentKind, AgentLogKind, AgentMessageAssembly,
        AgentRuntimeUpdate, SandboxResourceReadError, SandboxResourceReader,
        SandboxResourceReading,
    },
    SubjectId,
};
use std::sync::Arc;
use tower::ServiceExt;

struct MissingResourceAccess;

#[async_trait::async_trait]
impl SandboxResourceReader for MissingResourceAccess {
    async fn sandbox_resources(
        &self,
        _: &layer2_core::sandbox::Sandbox,
    ) -> Result<SandboxResourceReading, SandboxResourceReadError> {
        unreachable!("event stream reads do not inspect runtime resources")
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

#[tokio::test(start_paused = true)]
async fn empty_shared_tail_still_refuses_revocation_and_expired_membership() {
    for expired in [false, true] {
        let f = fixture(true, true).await;
        let share = share(&f).await;
        let auth = AuthenticatedResourceCaller {
            context: context("reader"),
            credential: Some(crate::DelegatedCredential::new("fixture-credential".into())),
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
        open.fill().await.unwrap();
        assert!(open.pending.is_empty());
        if expired {
            tokio::time::advance(Duration::from_secs(60)).await;
        } else {
            f.access
                .change_share(
                    &context("owner-a"),
                    None,
                    ShareCommand {
                        selector: share.selector,
                        change: ShareChange::Revoke { share_id: share.id },
                        expected_share_revision: Some(share.revision),
                        idempotency_key: "revoke-empty-tail".into(),
                    },
                )
                .await
                .unwrap();
        }
        assert!(open.fill().await.is_err());
        assert!(open.pending.is_empty());
    }
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
        open.held = Some(value.into());
        open.queue_live(false);
        assert!(open.pending.is_empty(), "{fault}");
        assert_eq!(open.held.is_some(), fault == "future");
    }
    open.held = Some(valid.into());
    open.queue_live(false);
    assert_eq!(open.pending.len(), 1);
    assert!(open.pending[0].live);
}

#[tokio::test]
async fn turn_change_does_not_drop_a_held_fragment_for_the_new_turn() {
    let (mut open, _) = following().await;
    let mut held = fragment(&open);
    held.turn_sequence += 1;
    open.header.active_turn = Some(held.turn_sequence);
    open.held = Some(held.into());
    open.pending.push_back(Frame {
        event: Event::default().event("cma.agent.live").data("{}"),
        live: true,
        fragment: Some(fragment(&open).into()),
    });
    open.live_batch_needs_validation = true;
    let frame = open.pending.pop_front().unwrap();
    assert!(!open.release(&frame).await);
    assert!(open.held.is_some());
}

#[tokio::test]
async fn live_observation_time_is_real_and_negotiated_without_changing_old_frames() {
    for preferred in [false, true] {
        let (mut open, _) = following().await;
        open.live_timestamps = preferred;
        let before = layer2_core::now_rfc3339();
        open.held = Some(fragment(&open).into());
        open.queue_live(false);
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
async fn buffered_live_text_survives_a_commit_and_follows_durable_catch_up() {
    let (mut open, f) = following().await;
    open.held = Some(fragment(&open).into());
    open.queue_live(false);
    let frame = open.pending.pop_front().unwrap();
    append(
        &f.store,
        AgentLogKind::OwnerMessage {
            content: "Committed steering text".into(),
        },
    )
    .await;
    assert!(!open.release(&frame).await);
    // The release path only defers; the main loop owns the durable refill.
    assert!(open.pending.is_empty());
    open.fill().await.unwrap();
    assert_eq!(open.pending.len(), 1);
    assert!(!open.pending[0].live);
    open.pending.clear();
    open.fill().await.unwrap();
    assert_eq!(
        open.pending.len(),
        1,
        "the valid live frame resumes after catch-up"
    );
    assert!(open.pending[0].live && open.pending[0].fragment.is_some());
}

#[tokio::test]
async fn deferred_live_batch_preserves_current_first_and_coalesced_order() {
    let (mut open, f) = following().await;
    let source = fragment(&open);
    let frame = |delta: &str, message_id: &str, kind: &str| Frame {
        event: Event::default().event("cma.agent.live").data(
            serde_json::json!({
                "delta": delta,
                "message_id": message_id,
                "kind": kind,
            })
            .to_string(),
        ),
        live: true,
        fragment: Some(
            AgentLiveFragment {
                delta: delta.into(),
                message_id: message_id.into(),
                kind: if kind == "text_delta" {
                    AgentLiveFragmentKind::Message
                } else {
                    AgentLiveFragmentKind::Thought
                },
                ..source.clone()
            }
            .into(),
        ),
    };
    open.pending.extend([
        frame("first", "reply", "text_delta"),
        frame("thought", "thought", "reasoning_summary_delta"),
        frame("third", "reply", "text_delta"),
    ]);
    open.live_batch_needs_validation = true;
    let first = open
        .pending
        .pop_front()
        .expect("the batch has a first frame");
    append(
        &f.store,
        AgentLogKind::OwnerMessage {
            content: "durable head advanced".into(),
        },
    )
    .await;
    assert!(!open.release(&first).await);
    assert!(open.deferred_live.len() == 3);
    open.fill().await.unwrap();
    let chunks: Vec<_> = response(open)
        .into_body()
        .into_data_stream()
        .take(4)
        .collect()
        .await;
    let wire = chunks
        .into_iter()
        .map(|chunk| String::from_utf8(chunk.unwrap().to_vec()).unwrap())
        .collect::<String>();
    assert!(wire.find("first").unwrap() < wire.find("thought").unwrap());
    assert!(wire.find("thought").unwrap() < wire.find("third").unwrap());
    assert_eq!(wire.matches("\"message_id\":\"reply\"").count(), 2);
}

#[tokio::test]
async fn deferred_fill_failure_reaches_the_typed_stream_error() {
    let (mut open, f) = following().await;
    open.held = Some(fragment(&open).into());
    open.queue_live(false);
    let frame = open.pending.pop_front().unwrap();
    append(
        &f.store,
        AgentLogKind::AgentMessage {
            content: "durable head advanced".into(),
        },
    )
    .await;
    assert!(!open.release(&frame).await);
    open.state.reads = layer2_core::sandbox::AgentReadService::new(
        f.store.clone(),
        Arc::new(MissingResourceAccess),
    );
    let error = open.fill().await.unwrap_err();
    assert_eq!(error.code(), "agent_read_unavailable");
}

async fn defer_live(open: &mut Following, f: &Fixture) {
    open.held = Some(fragment(open).into());
    open.queue_live(false);
    let frame = open.pending.pop_front().unwrap();
    append(
        &f.store,
        AgentLogKind::OwnerMessage {
            content: "steering".into(),
        },
    )
    .await;
    assert!(!open.release(&frame).await);
    assert_eq!(open.deferred_live.len(), 1);
}

async fn resume_wire(mut open: Following) -> String {
    open.fill().await.unwrap();
    let mut durable = std::mem::take(&mut open.pending);
    assert!(durable.iter().all(|frame| !frame.live));
    open.fill().await.unwrap();
    durable.append(&mut open.pending);
    open.pending = durable;
    open.ended = true;
    let bytes = axum::body::to_bytes(response(open).into_body(), usize::MAX)
        .await
        .unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

#[tokio::test]
async fn deferred_live_is_not_emitted_after_durable_turn_completion() {
    let (mut open, f) = following().await;
    let turn_sequence = open.header.active_turn.unwrap();
    defer_live(&mut open, &f).await;
    append(
        &f.store,
        AgentLogKind::AgentMessage {
            content: "final answer".into(),
        },
    )
    .await;
    append(&f.store, AgentLogKind::TurnCompleted { turn_sequence }).await;
    let mut agent = f.store.get_agent(&open.header.id).await.unwrap().unwrap();
    let revision = agent.revision;
    agent.conversation.active_turn = None;
    agent.revision += 1;
    f.store.save_agent(revision, agent).await.unwrap();
    let wire = resume_wire(open).await;
    assert!(wire.contains("turn_completed"));
    assert!(!wire.contains("event: cma.agent.live\n"), "{wire}");
    assert!(
        !wire.contains("cma.agent.live_gap"),
        "completed text is covered: {wire}"
    );
}

#[tokio::test]
async fn deferred_live_runtime_change_reports_a_lagged_gap_for_the_same_turn() {
    let (mut open, f) = following().await;
    defer_live(&mut open, &f).await;
    let mut agent = f.store.get_agent(&open.header.id).await.unwrap().unwrap();
    let revision = agent.revision;
    agent.conversation.runtime_generation = Some(open.header.runtime_generation.unwrap() + 1);
    agent.revision += 1;
    f.store.save_agent(revision, agent).await.unwrap();
    let wire = resume_wire(open).await;
    assert!(wire.contains("event: cma.agent.live_gap\n"), "{wire}");
    assert!(wire.contains("\"reason\":\"lagged\""));
    assert!(wire.contains("\"frames\":1"));
    assert!(!wire.contains("event: cma.agent.live\n"));
}

#[tokio::test]
async fn deferred_live_rechecks_committed_messages_and_preserves_remaining_order() {
    let (mut open, f) = following().await;
    defer_live(&mut open, &f).await;
    // Both later fragments are still valid when the preceding message is committed.
    for delta in ["first thought", "next thought"] {
        let mut value = fragment(&open);
        value.delta = delta.into();
        value.kind = AgentLiveFragmentKind::Thought;
        open.held = Some(value.into());
        open.queue_live(false);
        open.deferred_live.extend(std::mem::take(&mut open.pending));
    }
    append(
        &f.store,
        AgentLogKind::AgentMessage {
            content: "committed answer".into(),
        },
    )
    .await;
    let wire = resume_wire(open).await;
    assert!(
        !wire.contains("Live text"),
        "superseded message must be suppressed: {wire}"
    );
    assert!(wire.find("first thought").unwrap() < wire.find("next thought").unwrap());
    assert!(!wire.contains("cma.agent.live_gap"));
}

#[path = "stream_lag_tests.rs"]
mod lag;

#[path = "stream_wake_tests.rs"]
mod wake;

#[path = "stream_observation_tests.rs"]
mod observation_tests;
#[tokio::test]
async fn agent_v2_buffered_frames_recheck_deleting_recipient_and_owner() {
    use crate::rest::shared_stream_tests::ChangingStanding;
    for deleting in ["reader", "owner-a", "unrelated", "outage"] {
        let mut f = fixture(true, true).await;
        share(&f).await;
        let standing = Arc::new(ChangingStanding::default());
        let access = Arc::new(
            f.access
                .as_ref()
                .clone()
                .with_account_standing(standing.clone()),
        );
        f.state.reads = f.state.reads.with_resource_access(access);
        let router = crate::agent_v2::router(f.state);
        let response = router
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
        assert!(body.next().await.unwrap().is_ok());
        *standing.0.lock().unwrap() = Some(deleting);
        let next = body.next().await;
        if deleting == "unrelated" {
            assert!(next.unwrap().is_ok());
        } else {
            assert!(next.is_none(), "a buffered frame escaped after {deleting}");
        }
    }
}

#[tokio::test]
async fn agent_v2_live_batch_rechecks_standing_between_frames() {
    use crate::rest::shared_stream_tests::ChangingStanding;
    for deleting in ["reader", "owner-a", "unrelated", "outage"] {
        let mut f = fixture(true, true).await;
        share(&f).await;
        let standing = Arc::new(ChangingStanding::default());
        let access = Arc::new(
            f.access
                .as_ref()
                .clone()
                .with_account_standing(standing.clone()),
        );
        f.state.reads = f.state.reads.with_resource_access(access);
        let auth = AuthenticatedResourceCaller {
            context: context("reader"),
            credential: Some(crate::DelegatedCredential::new("fixture-credential".into())),
        };
        let mut open = open(
            f.state,
            auth,
            AgentId("agt_fixture".into()),
            None,
            HeaderMap::new(),
        )
        .await
        .unwrap();
        open.pending.clear();
        let source = fragment(&open);
        let frame = Frame {
            event: Event::default().event("cma.agent.live").data("{}"),
            live: true,
            fragment: Some(source.into()),
        };
        open.live_batch_needs_validation = true;
        assert!(open.release(&frame).await);
        *standing.0.lock().unwrap() = Some(deleting);
        assert_eq!(open.release(&frame).await, deleting == "unrelated");
    }
}

#[tokio::test(start_paused = true)]
async fn agent_v2_live_revocation_closes_without_waiting_for_fallback() {
    use crate::rest::shared_stream_tests::ChangingStanding;
    for event_wake in [false, true] {
        for deleting in ["reader", "owner-a", "unrelated", "outage"] {
            let mut f = fixture(true, true).await;
            share(&f).await;
            let standing = Arc::new(ChangingStanding::default());
            let access = Arc::new(
                f.access
                    .as_ref()
                    .clone()
                    .with_account_standing(standing.clone()),
            );
            f.state.reads = f.state.reads.with_resource_access(access);
            let auth = AuthenticatedResourceCaller {
                context: context("reader"),
                credential: Some(crate::DelegatedCredential::new("fixture-credential".into())),
            };
            let mut open = open(
                f.state.with_event_wake(event_wake, Duration::from_secs(1)),
                auth,
                AgentId("agt_fixture".into()),
                None,
                HeaderMap::new(),
            )
            .await
            .unwrap();
            open.pending.clear();
            open.initial_gap = false;
            let frame = Frame {
                event: Event::default().event("cma.agent.live").data("{}"),
                live: true,
                fragment: Some(fragment(&open).into()),
            };
            open.pending.extend([frame.clone(), frame]);
            open.live_batch_needs_validation = true;
            let mut body = response(open).into_body().into_data_stream();
            assert!(body.next().await.unwrap().is_ok());
            *standing.0.lock().unwrap() = Some(deleting);
            let start = tokio::time::Instant::now();
            let next = body.next().await;
            if deleting == "unrelated" {
                assert!(next.unwrap().is_ok());
            } else {
                assert!(next.is_none(), "a live frame escaped after {deleting}");
            }
            assert_eq!(
                start.elapsed(),
                Duration::ZERO,
                "event_wake={event_wake}, standing={deleting}"
            );
        }
    }
}
