//! What a session's log holds once its runtime starts talking.
//!
//! The property under all of these is that the log is the record. Whatever a
//! client saw, a client that arrives later — or comes back after a restart —
//! must be able to reconstruct from the sequence alone.

use std::collections::HashMap;
use std::sync::Arc;

use layer3_adapter::KernelAdapter;

use crate::authz::Caller;
use crate::conversation_support::{
    asking, chunk, conversation, conversing, offering, said, until_logged,
};
use crate::event::EventKind;
use crate::ids::SessionId;
use crate::service::SessionService;
use crate::session::{SessionStatus, StopReason};
use crate::test_support::{await_settled, service, spec, user, FakeAdapter, FakeStore};

async fn report_usage(adapter: &Arc<FakeAdapter>, used: u64, window: u64) {
    let held = adapter.conversation.lock().await;
    held.as_ref()
        .expect("the conversation was opened")
        .updates
        .send(said(layer3_adapter::SessionUpdate::UsageUpdate(
            layer3_adapter::UsageUpdate::new(used, window),
        )))
        .await
        .expect("the core is listening");
}

fn versioned_conversation_service(
    store: Arc<FakeStore>,
    adapter: Arc<FakeAdapter>,
    version: &str,
) -> Arc<SessionService> {
    let mut adapters: HashMap<crate::ids::KernelId, Arc<dyn KernelAdapter>> = HashMap::new();
    adapters.insert(crate::test_support::kernel(), adapter);
    let screen: crate::service::ScreenFactory =
        Arc::new(|geometry| Box::new(crate::test_support::NullScreen(geometry)));
    let environments =
        crate::test_support::environments_defaulting_to(crate::environment::AgentConfiguration {
            kernel: crate::test_support::kernel(),
            mode: crate::options::SessionMode::Conversation,
            ..Default::default()
        });
    SessionService::new(
        adapters,
        screen,
        store,
        crate::service::SessionPorts::default(),
        environments,
        crate::service::Deployment {
            runtime_version: Some(version.to_string()),
            ..crate::test_support::owned()
        },
    )
}

async fn await_ready(
    service: &Arc<SessionService>,
    caller: &Caller,
    id: &SessionId,
) -> crate::session::Session {
    for _ in 0..400 {
        if let Ok(session) = service.get_session(caller, id).await {
            if session.status == SessionStatus::Ready {
                return session;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    panic!("session never became ready");
}

#[tokio::test]
async fn runtime_updates_take_the_input_fence_before_conversation_ordering() {
    let (service, _owner, id, _adapter) = conversation().await;
    let input_gate = service.input_gates.gate(&id);
    let input_writer = input_gate.write_owned().await;
    let ordering = service.conversation_order_gates.gate(&id);
    let fold = service.fold_update(
        &id,
        layer3_adapter::ConversationUpdate::Lost { frames: 1 },
        &ordering,
    );
    tokio::pin!(fold);

    // Poll until the first blocked acquisition. The required order blocks on
    // the held input writer before it can own conversation ordering. Reversing
    // those gates leaves the ordering probe locked and recreates the rotation
    // deadlock this test guards against.
    tokio::select! {
        biased;
        result = &mut fold => panic!("the input fence did not block folding: {result:?}"),
        () = tokio::task::yield_now() => {}
    }
    let ordering_probe = ordering
        .try_lock()
        .expect("a blocked input fence must not retain conversation ordering");
    drop(ordering_probe);
    drop(input_writer);

    tokio::time::timeout(std::time::Duration::from_secs(1), &mut fold)
        .await
        .expect("folding resumes after the input boundary opens");
}

#[tokio::test]
async fn a_tool_call_and_its_progress_are_recorded() {
    let (svc, alice, id, adapter) = conversation().await;
    {
        let held = adapter.conversation.lock().await;
        let kernel = held.as_ref().expect("the conversation was opened");
        kernel
            .updates
            .send(said(layer3_adapter::SessionUpdate::ToolCall(
                layer3_adapter::ToolCall::new(
                    layer3_adapter::ToolCallId::new("call-1"),
                    "read a file".to_string(),
                ),
            )))
            .await
            .expect("the core is listening");
    }

    assert!(
        until_logged(&svc, &alice, &id, |kind| matches!(
            kind,
            EventKind::ToolCallStarted { call_id, title, .. }
                if call_id == "call-1" && title == "read a file"
        ))
        .await,
        "a tool call the agent made must be in the record"
    );
}

#[tokio::test]
async fn a_message_being_typed_is_liveness_and_never_numbered() {
    // Numbering deltas would punch permanent holes in the sequence a resuming
    // client cursors on, and replaying them would show somebody their
    // conversation being typed a second time.
    let (svc, alice, id, adapter) = conversation().await;
    {
        let held = adapter.conversation.lock().await;
        let kernel = held.as_ref().expect("the conversation was opened");
        for piece in ["Hel", "lo ", "there"] {
            kernel
                .updates
                .send(chunk(piece))
                .await
                .expect("the core is listening");
        }
    }
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let events = svc
        .events_since(&alice, &id, None)
        .await
        .expect("history readable");
    assert!(
        !events
            .iter()
            .any(|e| matches!(e.kind, EventKind::MessageCommitted { .. })),
        "nothing was committed, so nothing may be in the log: {events:?}"
    );
}

#[tokio::test]
async fn usage_is_reported_and_read_back() {
    let (svc, alice, id, adapter) = conversation().await;
    report_usage(&adapter, 1290, 258_400).await;
    assert!(
        until_logged(&svc, &alice, &id, |kind| matches!(
            kind,
            EventKind::UsageReported {
                context_used,
                context_window: Some(258_400),
                ..
            } if *context_used == 1290
        ))
        .await,
        "usage must reach the log, which is what get_usage reads"
    );

    let usage = svc.get_usage(&alice, &id).await.expect("usage is readable");
    assert_eq!(usage.context_used, 1290);
    assert_eq!(usage.context_window, Some(258_400));
}

#[tokio::test]
async fn a_zero_context_window_is_absent_rather_than_a_ceiling() {
    let (svc, alice, id, adapter) = conversation().await;
    report_usage(&adapter, 1290, 0).await;
    assert!(
        until_logged(&svc, &alice, &id, |kind| matches!(
            kind,
            EventKind::UsageReported {
                context_used: 1290,
                context_window: None,
                ..
            }
        ))
        .await
    );

    assert_eq!(
        svc.get_usage(&alice, &id)
            .await
            .expect("usage is readable")
            .context_window,
        None
    );
}

#[tokio::test]
async fn turn_accounting_does_not_wipe_a_context_reading_or_its_window() {
    let (svc, alice, id, adapter) = conversation().await;
    report_usage(&adapter, 1290, 258_400).await;
    assert!(
        until_logged(&svc, &alice, &id, |kind| matches!(
            kind,
            EventKind::UsageReported {
                context_used: 1290,
                ..
            }
        ))
        .await
    );
    {
        let held = adapter.conversation.lock().await;
        held.as_ref()
            .expect("the conversation was opened")
            .updates
            .send(layer3_adapter::ConversationUpdate::TurnEnded(
                layer3_adapter::PromptResponse::new(layer3_adapter::TurnStopReason::EndTurn)
                    .usage(layer3_adapter::TurnUsage::new(421, 412, 9)),
            ))
            .await
            .expect("the core is listening");
    }
    assert!(
        until_logged(&svc, &alice, &id, |kind| matches!(
            kind,
            EventKind::UsageReported {
                input_tokens: 412,
                ..
            }
        ))
        .await
    );

    let usage = svc.get_usage(&alice, &id).await.expect("usage is readable");
    assert_eq!(usage.context_used, 1290);
    assert_eq!(usage.context_window, Some(258_400));
}

#[tokio::test]
async fn a_context_fall_is_recorded_before_the_report_that_revealed_it() {
    let (svc, alice, id, adapter) = conversation().await;
    report_usage(&adapter, 198_000, 200_000).await;
    report_usage(&adapter, 42_100, 200_000).await;
    assert!(
        until_logged(&svc, &alice, &id, |kind| matches!(
            kind,
            EventKind::ContextCompacted {
                was: 198_000,
                now: 42_100,
                window: Some(200_000),
            }
        ))
        .await
    );

    let events = svc
        .events_since(&alice, &id, None)
        .await
        .expect("log reads");
    let compacted: Vec<_> = events
        .iter()
        .filter(|event| matches!(event.kind, EventKind::ContextCompacted { .. }))
        .collect();
    assert_eq!(compacted.len(), 1);
    let revealed = events
        .iter()
        .find(|event| {
            matches!(
                event.kind,
                EventKind::UsageReported {
                    context_used: 42_100,
                    ..
                }
            )
        })
        .expect("the revealing report is logged");
    assert!(compacted[0].seq < revealed.seq);
}

#[tokio::test]
async fn a_first_equal_or_rising_reading_is_not_a_compaction() {
    let (svc, alice, id, adapter) = conversation().await;
    for used in [42_100, 42_100, 198_000] {
        report_usage(&adapter, used, 200_000).await;
    }
    assert!(
        until_logged(&svc, &alice, &id, |kind| matches!(
            kind,
            EventKind::UsageReported {
                context_used: 198_000,
                ..
            }
        ))
        .await
    );

    let events = svc
        .events_since(&alice, &id, None)
        .await
        .expect("log reads");
    assert!(
        !events
            .iter()
            .any(|event| matches!(event.kind, EventKind::ContextCompacted { .. })),
        "a context that did not fall must have no compaction boundary"
    );
}

#[tokio::test]
async fn a_replacement_runtime_counting_from_less_is_not_a_compaction() {
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter {
        capabilities: conversing(),
        ..FakeAdapter::default()
    });
    let alice = user("alice");
    let old = versioned_conversation_service(Arc::clone(&store), Arc::clone(&adapter), "runtime:1");
    let session = old.create_session(&alice, spec()).await.expect("created");
    await_ready(&old, &alice, &session.id).await;
    report_usage(&adapter, 198_000, 200_000).await;
    assert!(
        until_logged(&old, &alice, &session.id, |kind| matches!(
            kind,
            EventKind::UsageReported {
                context_used: 198_000,
                ..
            }
        ))
        .await
    );
    drop(old);

    let new = versioned_conversation_service(store, Arc::clone(&adapter), "runtime:2");
    assert_eq!(new.reconcile().await.rotated_sessions, 1);
    await_ready(&new, &alice, &session.id).await;
    report_usage(&adapter, 42_100, 200_000).await;
    assert!(
        until_logged(&new, &alice, &session.id, |kind| matches!(
            kind,
            EventKind::UsageReported {
                context_used: 42_100,
                ..
            }
        ))
        .await
    );

    let events = new
        .events_since(&alice, &session.id, None)
        .await
        .expect("log reads");
    assert!(
        !events
            .iter()
            .any(|event| matches!(event.kind, EventKind::ContextCompacted { .. })),
        "readings from different runtimes must never be compared"
    );
}

#[tokio::test]
async fn a_session_without_the_usage_capability_is_refused_by_name() {
    let store = Arc::new(FakeStore::default());
    let svc = service(Arc::clone(&store), Arc::new(FakeAdapter::default()));
    let alice = user("alice");
    let created = svc.create_session(&alice, spec()).await.expect("created");
    await_settled(&svc, &alice, &created.id).await;

    assert_eq!(
        svc.get_usage(&alice, &created.id).await.err(),
        Some(crate::error::CoreError::CapabilityUnsupported(
            "usage".into()
        )),
        "a terminal session has no tokens to count and must say so"
    );
}

#[tokio::test]
async fn an_approval_parks_the_session_where_a_client_can_see_it() {
    // `RequiresAction` is the one stop reason that is explicitly not terminal.
    // A client shows this as a question; anything else reads as a session that
    // died holding somebody's work.
    let (svc, alice, id, adapter) = conversation().await;
    {
        let held = adapter.conversation.lock().await;
        held.as_ref()
            .expect("the conversation was opened")
            .updates
            .send(layer3_adapter::ConversationUpdate::Asked(asking(
                "req-1",
                "run a command",
            )))
            .await
            .expect("the core is listening");
    }

    assert!(
        until_logged(&svc, &alice, &id, |kind| matches!(
            kind,
            EventKind::ApprovalRequested { request_id, action, .. }
                if request_id == "req-1" && action.as_deref() == Some("run a command")
        ))
        .await,
        "the request must be in the record, naming what is being asked"
    );

    let session = svc.get_session(&alice, &id).await.expect("readable");
    assert_eq!(session.status, SessionStatus::Idle);
    assert_eq!(session.stop_reason, Some(StopReason::RequiresAction));
    assert!(!session.stop_reason.expect("present").is_terminal());
}

#[tokio::test]
async fn a_tool_calls_kind_is_recorded_as_the_protocol_spells_it() {
    // These strings live in durable events and are read back for the life of a
    // session. Taking them from a dependency's `Debug` output would mean a
    // rename there quietly changing what this service recorded about work
    // already done.
    let (svc, alice, id, adapter) = conversation().await;
    {
        let held = adapter.conversation.lock().await;
        held.as_ref()
            .expect("open")
            .updates
            .send(said(layer3_adapter::SessionUpdate::ToolCall(
                layer3_adapter::ToolCall::new(
                    layer3_adapter::ToolCallId::new("call-1"),
                    "read a file".to_string(),
                )
                .kind(layer3_adapter::ToolKind::Read),
            )))
            .await
            .expect("listening");
    }

    assert!(
        until_logged(&svc, &alice, &id, |kind| matches!(
            kind,
            EventKind::ToolCallStarted { kind, .. } if kind == "read"
        ))
        .await,
        "the recorded kind must be the protocol's own spelling"
    );
}

#[tokio::test]
async fn the_first_turn_after_a_session_settles_reaches_the_log() {
    // The production failure, in the shape a client actually meets it: create
    // a session, wait for it to say it is ready, send the first thing. The
    // turn ran and the log held nothing, so the client watched a stream that
    // never carried a message and never ended.
    //
    // Two writers had numbered it. Provisioning's closing `ready` was written
    // against the record after the last flush, and the entry's copy of the
    // position had not moved, so the very next thing the conversation logged
    // claimed a sequence the session already had. The index refused the batch
    // and every event in it was lost — not delayed, lost, and the durable log
    // is what `API_Overview.md` makes the thing a client recovers from.
    let (svc, alice, id, adapter) = conversation().await;
    {
        let held = adapter.conversation.lock().await;
        let kernel = held.as_ref().expect("the conversation was opened");
        kernel
            .updates
            .send(chunk("hi back"))
            .await
            .expect("the core is listening");
        kernel
            .updates
            .send(layer3_adapter::ConversationUpdate::TurnEnded(
                layer3_adapter::PromptResponse::new(layer3_adapter::TurnStopReason::EndTurn),
            ))
            .await
            .expect("the core is listening");
    }

    assert!(
        until_logged(&svc, &alice, &id, |kind| matches!(
            kind,
            EventKind::MessageCommitted { content } if content == "hi back"
        ))
        .await,
        "the first thing said after a session settles must be in its log"
    );

    // And the log is still a sequence: one event per number, in order, with
    // nothing standing in two places. A duplicate here would mean a resuming
    // client silently skipped or repeated part of its own history.
    let logged = svc
        .events_since(&alice, &id, None)
        .await
        .expect("the log is readable");
    let mut seqs: Vec<u64> = logged.iter().map(|e| e.seq).collect();
    let numbered = seqs.len();
    seqs.dedup();
    assert_eq!(seqs.len(), numbered, "no two events may share a sequence");
    assert!(seqs.windows(2).all(|w| w[0] < w[1]), "and they must ascend");
}

#[tokio::test]
async fn what_a_session_records_as_its_position_is_never_behind_its_log() {
    // How the collision was reachable at all. Both writers took their next
    // number from the position on the record, so a record left behind the log
    // handed the same number out twice — and after a restart it would do it
    // again, to a process with no memory of what it had already issued.
    let (svc, alice, id, adapter) = conversation().await;
    {
        let held = adapter.conversation.lock().await;
        let kernel = held.as_ref().expect("the conversation was opened");
        kernel
            .updates
            .send(said(layer3_adapter::SessionUpdate::ToolCall(
                layer3_adapter::ToolCall::new(
                    layer3_adapter::ToolCallId::new("call-1"),
                    "read a file".to_string(),
                ),
            )))
            .await
            .expect("the core is listening");
    }
    assert!(
        until_logged(&svc, &alice, &id, |kind| matches!(
            kind,
            EventKind::ToolCallStarted { .. }
        ))
        .await
    );

    let highest = svc
        .events_since(&alice, &id, None)
        .await
        .expect("the log is readable")
        .iter()
        .map(|e| e.seq)
        .max()
        .expect("a settled session has a log");
    let recorded = svc
        .get_session(&alice, &id)
        .await
        .expect("the session is readable")
        .last_seq;

    assert!(
        recorded >= highest,
        "the record says {recorded} and the log reaches {highest}; a process starting \
         from that record would hand out numbers the log already has"
    );
}

#[tokio::test]
async fn a_message_being_typed_is_watchable_while_it_is_typed() {
    // The gap this closes: between a turn starting and its message being
    // committed the log says nothing at all, so anything reading only the log
    // draws a working agent and a dead one identically. A turn takes as long
    // as the agent takes.
    let (svc, alice, id, adapter) = conversation().await;
    let mut watching = svc
        .subscribe_live(&alice, &id)
        .await
        .expect("a conversation session can be watched");

    {
        let held = adapter.conversation.lock().await;
        let kernel = held.as_ref().expect("the conversation was opened");
        for piece in ["Hel", "lo ", "there"] {
            kernel
                .updates
                .send(chunk(piece))
                .await
                .expect("the core is listening");
        }
    }

    let mut seen = String::new();
    for _ in 0..3 {
        match tokio::time::timeout(std::time::Duration::from_secs(2), watching.recv()).await {
            Ok(Ok(crate::event::LiveFrame::MessageDelta(text))) => seen.push_str(&text),
            other => panic!("expected a piece of the message, got {other:?}"),
        }
    }
    assert_eq!(seen, "Hello there");
}

#[tokio::test]
async fn nothing_watchable_reaches_the_durable_log() {
    // The other half, and the one that matters more. A piece of a message
    // given a sequence would punch a permanent hole in what a resuming client
    // cursors on, and replaying it would show somebody their conversation
    // being typed a second time.
    let (svc, alice, id, adapter) = conversation().await;
    let before = svc
        .events_since(&alice, &id, None)
        .await
        .expect("readable")
        .len();

    {
        let held = adapter.conversation.lock().await;
        let kernel = held.as_ref().expect("the conversation was opened");
        for piece in ["one ", "two ", "three"] {
            kernel
                .updates
                .send(chunk(piece))
                .await
                .expect("the core is listening");
        }
    }
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let after = svc
        .events_since(&alice, &id, None)
        .await
        .expect("readable")
        .len();
    assert_eq!(
        before, after,
        "a message being typed is liveness; the log must not have grown"
    );
}

#[tokio::test]
async fn a_session_that_holds_no_conversation_is_refused_by_name() {
    // Rather than handed a receiver that stays silent forever, which a caller
    // cannot tell from an agent that has not started talking yet.
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let svc = service(Arc::clone(&store), Arc::clone(&adapter));
    let alice = user("alice");
    let created = svc.create_session(&alice, spec()).await.expect("created");
    await_settled(&svc, &alice, &created.id).await;

    let refused = svc.subscribe_live(&alice, &created.id).await;

    assert!(
        matches!(
            refused,
            Err(crate::error::CoreError::CapabilityUnsupported(ref what))
                if what == "conversation"
        ),
        "got {refused:?}"
    );
}

#[tokio::test]
async fn somebody_elses_session_cannot_be_watched() {
    let (svc, _alice, id, _adapter) = conversation().await;
    let mallory = user("mallory");

    assert!(matches!(
        svc.subscribe_live(&mallory, &id).await,
        Err(crate::error::CoreError::Forbidden)
    ));
}

#[tokio::test]
async fn the_agents_reasoning_is_watchable_and_is_never_recorded() {
    // A turn spent thinking looks exactly like a turn spent stuck. The
    // reasoning crosses Layer 3 today — `AgentThoughtChunk` is an ACP v1
    // variant and the bridge forwards `SessionUpdate` whole — and the fold
    // dropped it on the floor.
    //
    // It must be watchable *and* absent from the log. §18.1 requires a turn
    // drawn from history and a turn watched arriving to produce the same
    // result, so a durable thought is a thought that gets replayed: somebody
    // reloading to check what the agent decided would watch it think again.
    let (svc, alice, id, adapter) = conversation().await;
    let mut watching = svc.subscribe_live(&alice, &id).await.expect("watchable");
    let before = svc
        .events_since(&alice, &id, None)
        .await
        .expect("readable")
        .len();

    {
        let held = adapter.conversation.lock().await;
        let kernel = held.as_ref().expect("the conversation was opened");
        kernel
            .updates
            .send(said(layer3_adapter::SessionUpdate::AgentThoughtChunk(
                layer3_adapter::ContentChunk::new(layer3_adapter::ContentBlock::from(
                    "the file is probably under src/",
                )),
            )))
            .await
            .expect("the core is listening");
    }

    match tokio::time::timeout(std::time::Duration::from_secs(2), watching.recv()).await {
        Ok(Ok(crate::event::LiveFrame::ThoughtDelta(text))) => {
            assert_eq!(text, "the file is probably under src/");
        }
        other => panic!("expected the agent's reasoning, got {other:?}"),
    }

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let after = svc
        .events_since(&alice, &id, None)
        .await
        .expect("readable")
        .len();
    assert_eq!(before, after, "reasoning must not reach the durable log");
}

#[tokio::test]
async fn reasoning_does_not_become_part_of_the_message_the_agent_commits() {
    // The one way this could go quietly wrong. If a thought accumulated into
    // the message being assembled, the committed message — which *is*
    // durable, and is what a reload shows — would contain the agent's
    // reasoning, permanently, having never been said out loud.
    let (svc, alice, id, adapter) = conversation().await;
    {
        let held = adapter.conversation.lock().await;
        let kernel = held.as_ref().expect("the conversation was opened");
        kernel
            .updates
            .send(said(layer3_adapter::SessionUpdate::AgentThoughtChunk(
                layer3_adapter::ContentChunk::new(layer3_adapter::ContentBlock::from(
                    "I should not say this part",
                )),
            )))
            .await
            .expect("the core is listening");
        kernel
            .updates
            .send(chunk("Found it."))
            .await
            .expect("the core is listening");
        kernel
            .updates
            .send(layer3_adapter::ConversationUpdate::TurnEnded(
                layer3_adapter::PromptResponse::new(layer3_adapter::TurnStopReason::EndTurn),
            ))
            .await
            .expect("the core is listening");
    }

    assert!(
        until_logged(&svc, &alice, &id, |kind| matches!(
            kind,
            EventKind::MessageCommitted { content } if content == "Found it."
        ))
        .await,
        "the committed message must be exactly what the agent said"
    );
    let logged = svc.events_since(&alice, &id, None).await.expect("readable");
    assert!(
        !logged.iter().any(|e| matches!(
            &e.kind,
            EventKind::MessageCommitted { content } if content.contains("should not say")
        )),
        "no part of the reasoning may be in the record: {logged:?}"
    );
}

/// A tool call with a body a session might hand over in its environment.
fn call_with_secret() -> layer3_adapter::ToolCall {
    layer3_adapter::ToolCall::new(
        layer3_adapter::ToolCallId::new("call-1"),
        "run a command".to_string(),
    )
    .raw_input(Some(serde_json::json!({
        "command": "curl -H 'Authorization: Bearer the-token'",
    })))
}

#[test]
fn a_tool_body_is_redacted_of_values_the_session_was_handed() {
    use super::durable;
    let kind = durable(
        layer3_adapter::SessionUpdate::ToolCall(call_with_secret()),
        true,
        &["the-token".to_string()],
    )
    .expect("a tool call folds");
    match kind {
        EventKind::ToolCallStarted {
            arguments: Some(body),
            ..
        } => {
            assert!(
                !body.text.contains("the-token"),
                "secret leaked: {}",
                body.text
            );
            assert!(body.text.contains("[redacted]"));
            assert!(!body.truncated);
        }
        other => panic!("expected a started call with arguments, got {other:?}"),
    }
}

#[test]
fn a_tool_body_is_withheld_when_the_session_is_not_granted() {
    use super::durable;
    let kind = durable(
        layer3_adapter::SessionUpdate::ToolCall(call_with_secret()),
        false,
        &["the-token".to_string()],
    )
    .expect("a tool call folds");
    match kind {
        EventKind::ToolCallStarted {
            arguments: None, ..
        } => {}
        other => panic!("expected no arguments when not granted, got {other:?}"),
    }
}

#[test]
fn a_tool_body_over_the_cap_says_it_was_cut() {
    use super::durable;
    let big = "x".repeat(9000);
    let fields = layer3_adapter::ToolCallUpdateFields::default()
        .raw_output(Some(serde_json::Value::String(big)));
    let update =
        layer3_adapter::ToolCallUpdate::new(layer3_adapter::ToolCallId::new("call-1"), fields);
    let kind = durable(
        layer3_adapter::SessionUpdate::ToolCallUpdate(update),
        true,
        &[],
    )
    .expect("a tool update folds");
    match kind {
        EventKind::ToolCallUpdated {
            result: Some(body), ..
        } => {
            assert!(body.truncated);
            assert!(body.text.len() <= 8192);
        }
        other => panic!("expected a truncated result, got {other:?}"),
    }
}

/*
 * A session whose kernel wants its owner to sign in — #351.
 *
 * These assert on the **durable log**, never on the adapter having reported
 * it. That distinction is the bug: #320 made the request reach the folder and
 * was verified by asking the adapter, so nobody noticed the folder had
 * nowhere to put it and the event was discarded on arrival.
 */

/// Build a conversation session whose kernel asks for a sign-in during the
/// open, before the core has a live entry for it.
async fn signing_in() -> (Arc<SessionService>, Caller, SessionId) {
    let adapter = Arc::new(FakeAdapter {
        capabilities: conversing(),
        sign_in_on_open: true,
        ..FakeAdapter::default()
    });
    let profiles =
        crate::test_support::environments_defaulting_to(crate::environment::AgentConfiguration {
            kernel: crate::test_support::kernel(),
            mode: crate::options::SessionMode::Conversation,
            ..Default::default()
        });
    let svc = crate::test_support::service_with_environments(
        Arc::new(FakeStore::default()),
        Arc::clone(&adapter),
        profiles,
    );
    let alice = user("alice");
    let created = svc.create_session(&alice, spec()).await.expect("created");
    await_settled(&svc, &alice, &created.id).await;
    (svc, alice, created.id)
}

#[tokio::test]
async fn a_sign_in_raised_before_the_session_is_ready_reaches_the_log() {
    let (svc, alice, id) = signing_in().await;

    assert!(
        until_logged(&svc, &alice, &id, |kind| matches!(
            kind,
            EventKind::SignInRequired { url, .. } if url == "https://auth.example.test/device"
        ))
        .await,
        "the owner cannot sign in to a prompt that was never recorded"
    );
}

#[tokio::test]
async fn a_sign_in_carries_the_code_its_owner_has_to_enter() {
    // The address alone is not actionable: a device flow asks for a code, and
    // one that reached nobody leaves the session waiting on it forever.
    let (svc, alice, id) = signing_in().await;

    assert!(
        until_logged(&svc, &alice, &id, |kind| matches!(
            kind,
            EventKind::SignInRequired { code, message, .. }
                if code.as_deref() == Some("WDJB-MJHT")
                    && message.as_deref() == Some("Sign in to continue")
        ))
        .await,
        "the code and the kernel's own wording must survive into the record"
    );
}

#[tokio::test]
async fn the_commands_a_kernel_offers_are_recorded_rather_than_only_broadcast() {
    // The list used to ride the live lane alone, on the reasoning that a
    // replayed list could offer commands that no longer exist. The kernel
    // announces it as the conversation opens, which is before anybody is
    // subscribed, so what actually happened is that it reached nobody at all —
    // and the protocol has no request to ask for it again.
    let (svc, alice, id, adapter) = conversation().await;
    {
        let held = adapter.conversation.lock().await;
        let kernel = held.as_ref().expect("the conversation was opened");
        kernel
            .updates
            .send(said(
                layer3_adapter::SessionUpdate::AvailableCommandsUpdate(offering(&[(
                    "review",
                    "Review the working tree",
                )])),
            ))
            .await
            .expect("the core is listening");
    }

    assert!(
        until_logged(&svc, &alice, &id, |kind| matches!(
            kind,
            EventKind::AvailableCommandsSet { commands }
                if commands.len() == 1
                    && commands[0].name == "review"
                    && commands[0].description == "Review the working tree"
        ))
        .await,
        "a command list nothing recorded is a command list nobody can ever be shown"
    );
}

#[tokio::test]
async fn the_commands_a_kernel_offers_survive_arriving_before_the_entry_exists() {
    /*
     * The real timing, pinned directly. `entries` holds only sessions with a
     * running runtime, and this announcement is made as the conversation
     * opens — so arriving with no entry is the ordinary case for it, not a
     * fault. Before the fallback it returned early and the list was gone.
     */
    let (svc, alice, id, adapter) = conversation().await;
    svc.entries.write().await.remove(&id);

    {
        let held = adapter.conversation.lock().await;
        let kernel = held.as_ref().expect("the conversation was opened");
        kernel
            .updates
            .send(said(
                layer3_adapter::SessionUpdate::AvailableCommandsUpdate(offering(&[(
                    "compact",
                    "Summarise the conversation so far",
                )])),
            ))
            .await
            .expect("the core is listening");
    }

    assert!(
        until_logged(&svc, &alice, &id, |kind| matches!(
            kind,
            EventKind::AvailableCommandsSet { commands }
                if commands.iter().any(|c| c.name == "compact")
        ))
        .await,
        "the announcement precedes the entry, so dropping it there drops it always"
    );
}

#[tokio::test]
async fn a_kernel_that_offers_nothing_records_that_rather_than_staying_silent() {
    // "This kernel has no commands" and "nothing has been said yet" are
    // different facts. Recording only a non-empty list would leave a client
    // unable to tell them apart, and leave a previous list standing.
    let (svc, alice, id, adapter) = conversation().await;
    {
        let held = adapter.conversation.lock().await;
        let kernel = held.as_ref().expect("the conversation was opened");
        kernel
            .updates
            .send(said(
                layer3_adapter::SessionUpdate::AvailableCommandsUpdate(offering(&[])),
            ))
            .await
            .expect("the core is listening");
    }

    assert!(
        until_logged(&svc, &alice, &id, |kind| matches!(
            kind,
            EventKind::AvailableCommandsSet { commands } if commands.is_empty()
        ))
        .await,
        "an empty offering is an answer and has to be recorded as one"
    );
}

#[tokio::test]
async fn a_sign_in_is_recorded_for_a_session_with_no_live_entry() {
    /*
     * The fallback itself, pinned directly rather than through the timing of
     * an open. `entries` holds only sessions with a running runtime, so its
     * absence is what the sign-in path has to survive — removing the entry
     * from a settled session reproduces that exactly, and driving the update
     * down the kernel's own channel keeps the test on the real path rather
     * than on a hook that exists only for it.
     */
    let (svc, alice, id, adapter) = conversation().await;
    svc.entries.write().await.remove(&id);

    {
        let held = adapter.conversation.lock().await;
        let kernel = held.as_ref().expect("the conversation was opened");
        kernel
            .updates
            .send(layer3_adapter::ConversationUpdate::SignInRequired {
                url: "https://auth.example.test/device".to_string(),
                code: Some("WDJB-MJHT".to_string()),
                message: None,
            })
            .await
            .expect("the core is listening");
    }

    assert!(
        until_logged(&svc, &alice, &id, |kind| matches!(
            kind,
            EventKind::SignInRequired { code, .. } if code.as_deref() == Some("WDJB-MJHT")
        ))
        .await,
        "a sign-in must be recorded even with no live entry to append to"
    );
}

#[tokio::test]
async fn a_sign_in_for_a_session_nobody_holds_any_more_is_dropped_quietly() {
    /*
     * The failure path. Once the session is gone from the record there is
     * nothing to append against and nobody left to show it to, so dropping it
     * is right — but it must drop rather than panic, because the runtime
     * raising it has simply not noticed the session ended yet.
     */
    let (svc, alice, id, adapter) = conversation().await;
    svc.terminate_session(&alice, &id)
        .await
        .expect("terminated");
    svc.delete_session(&alice, &id).await.expect("deleted");

    {
        let held = adapter.conversation.lock().await;
        let kernel = held.as_ref().expect("the conversation was opened");
        // A closed channel is itself the session being gone; either way this
        // must not bring the folder down.
        let _ = kernel
            .updates
            .send(layer3_adapter::ConversationUpdate::SignInRequired {
                url: "https://auth.example.test/device".to_string(),
                code: None,
                message: None,
            })
            .await;
    }

    // Give the folder a chance to meet it, then prove the service is still
    // answering: a panic in that task would take the log with it.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(
        !svc.list_sessions(&alice, &crate::SessionListQuery::default())
            .await
            .expect("session list is readable")
            .items
            .iter()
            .any(|s| s.id == id),
        "the core must survive a sign-in for a session that no longer exists"
    );
}
