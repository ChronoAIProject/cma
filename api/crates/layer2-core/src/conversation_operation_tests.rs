//! Driving a conversation: prompting, interrupting, answering and metering.
//!
//! Split from the folding tests so both stay within the project's file-size
//! limit, on a real division: those are about what a runtime says, and these
//! are about what an owner does.

use std::sync::Arc;

use layer3_adapter::{ConversationInput, PermissionDecision};

use crate::conversation::ApprovalDecision;
use crate::conversation_support::{
    asking, chunk, conversation, conversation_with, conversing, said, until_logged, until_sent,
};
use crate::error::CoreError;
use crate::event::EventKind;
use crate::session::{SessionStatus, StopReason};
use crate::test_support::{await_settled, service, spec, user, FakeAdapter, FakeStore};

#[tokio::test]
async fn an_idle_prompt_needs_no_steer_grant() {
    let (svc, alice, id, adapter) = conversation().await;
    let idle = svc.get_session(&alice, &id).await.expect("readable");
    assert_eq!(idle.status, SessionStatus::Ready);
    assert!(!idle.capabilities.steer);

    svc.send_prompt(&alice, &id, "what is in this repository?".into())
        .await
        .expect("an idle conversation needs no mid-turn input grant");

    let sent = until_sent(&adapter, |input| {
        matches!(input, ConversationInput::Prompt(_))
    })
    .await;
    assert!(
        sent.iter()
            .any(|input| matches!(input, ConversationInput::Prompt(_))),
        "a prompt must reach the runtime: {sent:?}"
    );
}

#[tokio::test]
async fn a_prompt_records_the_turn_boundary_before_its_owner_message() {
    let (svc, alice, id, _adapter) = conversation().await;
    svc.send_prompt(&alice, &id, "open the turn".into())
        .await
        .expect("the owner may prompt their own session");

    let events = svc
        .events_since(&alice, &id, None)
        .await
        .expect("the prompt history is readable");
    let prompt_events = events
        .iter()
        .filter_map(|event| match &event.kind {
            EventKind::TurnStarted => Some(&event.kind),
            EventKind::OwnerMessage { .. } => Some(&event.kind),
            _ => None,
        })
        .collect::<Vec<_>>();

    assert!(
        matches!(
            prompt_events.as_slice(),
            [EventKind::TurnStarted, EventKind::OwnerMessage { content }]
                if content == "open the turn"
        ),
        "the durable turn must open before its owner message: {prompt_events:?}"
    );
}

#[tokio::test]
async fn a_runtime_completion_cannot_overtake_the_prompt_turn_boundary() {
    let (svc, alice, id, adapter) = conversation().await;
    let updates = adapter
        .conversation
        .lock()
        .await
        .as_ref()
        .expect("the conversation is open")
        .updates
        .clone();
    let input = layer3_adapter::ConversationInputSender::spawn(1, move |input| {
        let updates = updates.clone();
        async move {
            if matches!(input, ConversationInput::Prompt(_)) {
                updates
                    .send(layer3_adapter::ConversationUpdate::TurnEnded(
                        layer3_adapter::PromptResponse::new(
                            layer3_adapter::TurnStopReason::EndTurn,
                        ),
                    ))
                    .await
                    .expect("the core is folding runtime updates");
                // Let the folder observe the completion before delivery is
                // acknowledged. Without the conversation-order gate it folds
                // the ending here, ahead of the boundary below.
                tokio::task::yield_now().await;
            }
            Ok(())
        }
    });
    svc.entries
        .write()
        .await
        .get_mut(&id)
        .expect("the session is live")
        .conversation = Some(input);

    svc.send_prompt(&alice, &id, "finish immediately".into())
        .await
        .expect("the prompt was delivered");
    assert!(
        until_logged(&svc, &alice, &id, |kind| matches!(
            kind,
            EventKind::TurnCompleted
        ))
        .await,
        "the immediate completion must still be recorded"
    );

    let events = svc
        .events_since(&alice, &id, None)
        .await
        .expect("the completed prompt history is readable");
    let turn = events
        .iter()
        .filter_map(|event| match &event.kind {
            EventKind::TurnStarted | EventKind::OwnerMessage { .. } | EventKind::TurnCompleted => {
                Some(&event.kind)
            }
            _ => None,
        })
        .collect::<Vec<_>>();

    assert!(
        matches!(
            turn.as_slice(),
            [
                EventKind::TurnStarted,
                EventKind::OwnerMessage { content },
                EventKind::TurnCompleted
            ] if content == "finish immediately"
        ),
        "the accepted prompt must be recorded before its effects: {turn:?}"
    );
}

#[tokio::test]
async fn a_mid_turn_prompt_without_steer_reaches_neither_runtime_nor_log() {
    let (svc, alice, id, adapter) = conversation().await;
    svc.send_prompt(&alice, &id, "begin the turn".into())
        .await
        .expect("the idle prompt starts the turn");

    let sent_before = until_sent(&adapter, |input| {
        matches!(input, ConversationInput::Prompt(_))
    })
    .await;
    let events_before = svc
        .events_since(&alice, &id, None)
        .await
        .expect("history readable");
    let cursor = events_before.last().expect("the turn was recorded").seq;

    assert_eq!(
        svc.send_prompt(&alice, &id, "redirect the running turn".into())
            .await,
        Err(CoreError::CapabilityUnsupported("steer".into()))
    );

    tokio::task::yield_now().await;
    let sent_after = adapter
        .conversation
        .lock()
        .await
        .as_ref()
        .expect("the conversation remains open")
        .sent()
        .await;
    assert_eq!(
        sent_after.len(),
        sent_before.len(),
        "the refused prompt must send no input to the runtime: {sent_after:?}"
    );

    let appended = svc
        .events_since(&alice, &id, Some(cursor))
        .await
        .expect("history readable");
    assert!(
        !appended.iter().any(|event| matches!(
            event.kind,
            EventKind::OwnerMessage { .. } | EventKind::TurnStarted
        )),
        "the refused prompt must append neither owner input nor a turn boundary: {appended:?}"
    );
    assert!(
        appended.is_empty(),
        "a capability refusal must not change the log at all: {appended:?}"
    );
}

#[tokio::test]
async fn concurrent_idle_prompts_admit_one_before_the_other_can_decide_to_steer() {
    let (svc, alice, id, _adapter) = conversation().await;
    let before = svc
        .events_since(&alice, &id, None)
        .await
        .expect("history readable");
    let cursor = before.last().expect("session readiness was recorded").seq;

    let delivered = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let (first_started, first_is_started) = tokio::sync::oneshot::channel();
    let (release_first, first_may_finish) = tokio::sync::oneshot::channel();
    let mut first_started = Some(first_started);
    let mut first_may_finish = Some(first_may_finish);
    let delivered_by_runtime = Arc::clone(&delivered);
    let held_input = layer3_adapter::ConversationInputSender::spawn(2, move |input| {
        let started = first_started.take();
        let may_finish = first_may_finish.take();
        let delivered = Arc::clone(&delivered_by_runtime);
        async move {
            delivered.lock().await.push(input);
            if let Some(started) = started {
                let _ = started.send(());
            }
            if let Some(may_finish) = may_finish {
                let _ = may_finish.await;
            }
            Ok(())
        }
    });
    svc.entries
        .write()
        .await
        .get_mut(&id)
        .expect("the session is live")
        .conversation = Some(held_input);

    let first_service = Arc::clone(&svc);
    let first_caller = alice.clone();
    let first_id = id.clone();
    let first = tokio::spawn(async move {
        first_service
            .send_prompt(&first_caller, &first_id, "first prompt".into())
            .await
    });
    first_is_started
        .await
        .expect("the first prompt reached runtime delivery");

    let second = svc.send_prompt(&alice, &id, "second prompt".into());
    tokio::pin!(second);
    tokio::select! {
        biased;
        result = &mut second => panic!("the second prompt did not wait: {result:?}"),
        () = tokio::task::yield_now() => {}
    }
    assert_eq!(
        delivered.lock().await.len(),
        1,
        "only the admitted prompt may reach runtime delivery"
    );

    release_first
        .send(())
        .expect("the first prompt is still awaiting release");
    first
        .await
        .expect("the first prompt task completes")
        .expect("the idle prompt is admitted");
    assert_eq!(
        second.await,
        Err(CoreError::CapabilityUnsupported("steer".into()))
    );

    let delivered = delivered.lock().await;
    assert_eq!(
        delivered
            .iter()
            .filter(|input| matches!(input, ConversationInput::Prompt(_)))
            .count(),
        1,
        "the refused concurrent prompt must never reach the runtime: {delivered:?}"
    );
    drop(delivered);

    let appended = svc
        .events_since(&alice, &id, Some(cursor))
        .await
        .expect("history readable");
    assert_eq!(
        appended.len(),
        3,
        "one prompt appends one turn boundary, owner message, and status: {appended:?}"
    );
    let owner_messages = appended
        .iter()
        .filter_map(|event| match &event.kind {
            EventKind::OwnerMessage { content } => Some(content.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        owner_messages,
        vec!["first prompt"],
        "only the admitted prompt may append owner input: {appended:?}"
    );
    assert_eq!(
        appended
            .iter()
            .filter(|event| matches!(event.kind, EventKind::TurnStarted))
            .count(),
        1,
        "only the admitted prompt may append a turn boundary: {appended:?}"
    );
    assert_eq!(
        appended
            .iter()
            .filter(|event| matches!(
                event.kind,
                EventKind::StatusChanged {
                    status: SessionStatus::Running,
                    ..
                }
            ))
            .count(),
        1,
        "only the admitted prompt may append a running transition: {appended:?}"
    );
}

#[tokio::test]
async fn a_mid_turn_prompt_with_steer_reaches_the_test_runtime() {
    let mut capabilities = conversing();
    capabilities.steer = true;
    let (svc, alice, id, adapter) = conversation_with(capabilities).await;
    svc.send_prompt(&alice, &id, "begin the turn".into())
        .await
        .expect("the idle prompt starts the turn");
    svc.send_prompt(&alice, &id, "redirect the running turn".into())
        .await
        .expect("the advertised mid-turn input grant is honoured");

    let sent = until_sent(&adapter, |input| match input {
        ConversationInput::Prompt(content) => content.iter().any(|block| match block {
            layer3_adapter::ContentBlock::Text(text) => text.text == "redirect the running turn",
            _ => false,
        }),
        _ => false,
    })
    .await;
    assert_eq!(
        sent.iter()
            .filter(|input| matches!(input, ConversationInput::Prompt(_)))
            .count(),
        2,
        "both prompts must reach a stub that advertises mid-turn input: {sent:?}"
    );
}

#[tokio::test]
async fn interrupting_cancels_the_turn_rather_than_the_session() {
    let (svc, alice, id, adapter) = conversation().await;
    svc.interrupt_turn(&alice, &id)
        .await
        .expect("the owner may interrupt their own session");

    let sent = until_sent(&adapter, |input| matches!(input, ConversationInput::Cancel)).await;
    assert!(sent
        .iter()
        .any(|input| matches!(input, ConversationInput::Cancel)));

    let session = svc.get_session(&alice, &id).await.expect("readable");
    assert!(
        !session.status.is_terminal(),
        "a cancelled turn is not an ended session"
    );
}

#[tokio::test]
async fn an_undeliverable_interrupt_is_runtime_unavailable() {
    let (svc, alice, id, _adapter) = conversation().await;
    let input = layer3_adapter::ConversationInputSender::spawn(1, |_input| async {
        Err(layer3_adapter::AdapterError::Unavailable(
            "the runtime link is gone".to_string(),
        ))
    });
    svc.entries
        .write()
        .await
        .get_mut(&id)
        .expect("the session is live")
        .conversation = Some(input);

    assert_eq!(
        svc.interrupt_turn(&alice, &id).await,
        Err(crate::error::CoreError::RuntimeUnavailable(
            "the runtime link is gone".to_string()
        ))
    );
}

#[tokio::test]
async fn a_terminal_session_cannot_be_prompted() {
    // The declared-subset rule: a session without the conversation capability
    // is refused by name rather than accepted and quietly ignored.
    let store = Arc::new(FakeStore::default());
    let svc = service(Arc::clone(&store), Arc::new(FakeAdapter::default()));
    let alice = user("alice");
    let created = svc.create_session(&alice, spec()).await.expect("created");
    await_settled(&svc, &alice, &created.id).await;

    assert_eq!(
        svc.send_prompt(&alice, &created.id, "hello".into())
            .await
            .err(),
        Some(crate::error::CoreError::CapabilityUnsupported(
            "conversation".into()
        ))
    );
}

#[tokio::test]
async fn a_terminal_session_records_exactly_what_it_did_before() {
    // The property that keeps every existing session safe: nothing on the
    // conversation plane appears in a log that has no conversation.
    let store = Arc::new(FakeStore::default());
    let svc = service(Arc::clone(&store), Arc::new(FakeAdapter::default()));
    let alice = user("alice");
    let created = svc.create_session(&alice, spec()).await.expect("created");
    await_settled(&svc, &alice, &created.id).await;

    let events = svc
        .events_since(&alice, &created.id, None)
        .await
        .expect("history readable");
    assert!(
        !events.iter().any(|e| matches!(
            e.kind,
            EventKind::MessageCommitted { .. }
                | EventKind::ToolCallStarted { .. }
                | EventKind::ToolCallUpdated { .. }
                | EventKind::UsageReported { .. }
                | EventKind::ConversationGap { .. }
        )),
        "a terminal session's log must be what it always was: {events:?}"
    );
}

#[tokio::test]
async fn a_conversation_that_ends_mid_turn_closes_the_turn_it_left_open() {
    // A dangling turn sits in the log forever looking like work in progress,
    // which is indistinguishable from an agent that is merely slow.
    let (svc, alice, id, adapter) = conversation().await;
    {
        let held = adapter.conversation.lock().await;
        held.as_ref()
            .expect("open")
            .updates
            .send(said(layer3_adapter::SessionUpdate::ToolCall(
                layer3_adapter::ToolCall::new(
                    layer3_adapter::ToolCallId::new("call-1"),
                    "a long job".to_string(),
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

    // Mid-turn: a prompt is what opens one, and the log says so.
    svc.send_prompt(&alice, &id, "do something long".into())
        .await
        .expect("the owner may prompt their own session");
    assert!(
        until_logged(&svc, &alice, &id, |kind| matches!(
            kind,
            EventKind::TurnStarted
        ))
        .await
    );

    // The runtime's conversation ends, as it does when a bridge exits.
    *adapter.conversation.lock().await = None;

    assert!(
        until_logged(&svc, &alice, &id, |kind| matches!(
            kind,
            EventKind::StatusChanged {
                stop_reason: Some(StopReason::Error),
                ..
            }
        ))
        .await,
        "the log must say the conversation ended rather than trailing off"
    );
}

#[tokio::test]
async fn a_message_is_committed_when_the_turn_that_carried_it_ends() {
    // The pieces are liveness; the whole thing is history. A client that
    // reconnects after the fact finds the message, not the typing.
    let (svc, alice, id, adapter) = conversation().await;
    svc.send_prompt(&alice, &id, "say something".into())
        .await
        .expect("prompted");
    {
        let held = adapter.conversation.lock().await;
        let kernel = held.as_ref().expect("open");
        for piece in ["Hel", "lo ", "there"] {
            kernel.updates.send(chunk(piece)).await.expect("listening");
        }
        kernel
            .updates
            .send(layer3_adapter::ConversationUpdate::TurnEnded(
                layer3_adapter::PromptResponse::new(layer3_adapter::TurnStopReason::EndTurn),
            ))
            .await
            .expect("listening");
    }

    assert!(
        until_logged(&svc, &alice, &id, |kind| matches!(
            kind,
            EventKind::MessageCommitted { content } if content == "Hello there"
        ))
        .await,
        "the whole message must be recorded, assembled from its pieces"
    );
    assert!(
        until_logged(&svc, &alice, &id, |kind| matches!(
            kind,
            EventKind::TurnCompleted
        ))
        .await
    );

    let settled = svc.get_session(&alice, &id).await.expect("readable");
    assert_eq!(settled.status, SessionStatus::Ready);
    assert_eq!(settled.stop_reason, Some(StopReason::EndTurn));
}

#[tokio::test]
async fn a_turn_that_ran_out_of_budget_says_so() {
    // The reason a turn ended is the difference between an agent that
    // finished, one that was interrupted and one that hit a ceiling. A client
    // showing all three the same way is showing none of them.
    let (svc, alice, id, adapter) = conversation().await;
    svc.send_prompt(&alice, &id, "say a great deal".into())
        .await
        .expect("prompted");
    {
        let held = adapter.conversation.lock().await;
        held.as_ref()
            .expect("open")
            .updates
            .send(layer3_adapter::ConversationUpdate::TurnEnded(
                layer3_adapter::PromptResponse::new(layer3_adapter::TurnStopReason::MaxTokens),
            ))
            .await
            .expect("listening");
    }

    assert!(
        until_logged(&svc, &alice, &id, |kind| matches!(
            kind,
            EventKind::StatusChanged {
                stop_reason: Some(StopReason::BudgetReached),
                ..
            }
        ))
        .await,
        "hitting a ceiling must be recorded as that rather than as an ending"
    );
}

#[tokio::test]
async fn a_gap_in_the_conversation_is_recorded_where_it_happened() {
    // The §5 counterpart of a rotation's timestamp: the owner is shown a
    // boundary rather than left to puzzle over a conversation that skips.
    let (svc, alice, id, adapter) = conversation().await;
    {
        let held = adapter.conversation.lock().await;
        held.as_ref()
            .expect("open")
            .updates
            .send(layer3_adapter::ConversationUpdate::Lost { frames: 12 })
            .await
            .expect("listening");
    }

    assert!(
        until_logged(&svc, &alice, &id, |kind| matches!(
            kind,
            EventKind::ConversationGap { frames } if *frames == 12
        ))
        .await,
        "a gap must be in the record, and say how large it was"
    );
}

#[tokio::test]
async fn a_second_message_commits_the_one_before_it() {
    // A runtime says nothing to mark the end of a message; what marks it is
    // the next one starting.
    let (svc, alice, id, adapter) = conversation().await;
    {
        let held = adapter.conversation.lock().await;
        let kernel = held.as_ref().expect("open");
        kernel
            .updates
            .send(layer3_adapter::ConversationUpdate::Said(
                layer3_adapter::SessionUpdate::AgentMessageChunk(
                    layer3_adapter::ContentChunk::new(layer3_adapter::ContentBlock::from("first"))
                        .message_id(layer3_adapter::MessageId::new("m-1")),
                ),
            ))
            .await
            .expect("listening");
        kernel
            .updates
            .send(layer3_adapter::ConversationUpdate::Said(
                layer3_adapter::SessionUpdate::AgentMessageChunk(
                    layer3_adapter::ContentChunk::new(layer3_adapter::ContentBlock::from("second"))
                        .message_id(layer3_adapter::MessageId::new("m-2")),
                ),
            ))
            .await
            .expect("listening");
    }

    assert!(
        until_logged(&svc, &alice, &id, |kind| matches!(
            kind,
            EventKind::MessageCommitted { content } if content == "first"
        ))
        .await,
        "the first message must be committed once the second begins"
    );
}

#[tokio::test]
async fn the_tokens_a_turn_spent_are_recorded_when_it_ends() {
    // Where a runtime actually reports them: the update stream says how full
    // the context is, and the turn's own response says what it cost.
    let (svc, alice, id, adapter) = conversation().await;
    svc.send_prompt(&alice, &id, "say something".into())
        .await
        .expect("prompted");
    {
        let held = adapter.conversation.lock().await;
        held.as_ref()
            .expect("open")
            .updates
            .send(layer3_adapter::ConversationUpdate::TurnEnded(
                layer3_adapter::PromptResponse::new(layer3_adapter::TurnStopReason::EndTurn)
                    .usage(layer3_adapter::TurnUsage::new(1290, 1234, 56)),
            ))
            .await
            .expect("listening");
    }

    assert!(
        until_logged(&svc, &alice, &id, |kind| matches!(
            kind,
            EventKind::UsageReported { input_tokens, output_tokens, .. }
                if *input_tokens == 1234 && *output_tokens == 56
        ))
        .await,
        "what a turn cost must be in the record"
    );

    let usage = svc.get_usage(&alice, &id).await.expect("readable");
    assert_eq!(usage.input_tokens, 1234);
    assert_eq!(usage.output_tokens, 56);
}

#[tokio::test]
async fn usage_never_falls_when_a_session_is_carried_onto_a_new_runtime() {
    // A replacement runtime counts afresh. Taking the latest would show
    // somebody their usage decreasing; adding them would count the history the
    // new runtime resumed from twice.
    let (svc, alice, id, adapter) = conversation().await;
    svc.send_prompt(&alice, &id, "one".into())
        .await
        .expect("prompted");
    {
        let held = adapter.conversation.lock().await;
        let kernel = held.as_ref().expect("open");
        kernel
            .updates
            .send(layer3_adapter::ConversationUpdate::TurnEnded(
                layer3_adapter::PromptResponse::new(layer3_adapter::TurnStopReason::EndTurn)
                    .usage(layer3_adapter::TurnUsage::new(9000, 8000, 1000)),
            ))
            .await
            .expect("listening");
        kernel
            .updates
            .send(layer3_adapter::ConversationUpdate::TurnEnded(
                layer3_adapter::PromptResponse::new(layer3_adapter::TurnStopReason::EndTurn)
                    .usage(layer3_adapter::TurnUsage::new(10, 5, 5)),
            ))
            .await
            .expect("listening");
    }

    assert!(
        until_logged(&svc, &alice, &id, |kind| matches!(
            kind,
            EventKind::UsageReported { input_tokens, .. } if *input_tokens == 5
        ))
        .await
    );
    let usage = svc.get_usage(&alice, &id).await.expect("readable");
    assert_eq!(
        usage.input_tokens, 8000,
        "a fresh runtime's smaller count must not shrink what has been spent"
    );
}

#[tokio::test]
async fn answering_an_approval_reaches_the_runtime_and_the_record() {
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
            EventKind::ApprovalRequested { .. }
        ))
        .await
    );

    svc.resolve_approval(&alice, &id, "req-1".into(), ApprovalDecision::Allow)
        .await
        .expect("the owner may answer their own session");

    let sent = until_sent(&adapter, |input| {
        matches!(input, ConversationInput::PermissionAnswer { .. })
    })
    .await;
    assert!(
        sent.iter().any(|input| matches!(
            input,
            ConversationInput::PermissionAnswer { id, decision }
                if id == "req-1" && *decision == PermissionDecision::Allow
        )),
        "the answer must reach the runtime, which is the only thing that can act on it: {sent:?}"
    );

    let events = svc
        .events_since(&alice, &id, None)
        .await
        .expect("history readable");
    assert!(events.iter().any(|e| matches!(
        &e.kind,
        EventKind::ApprovalResolved { request_id, allowed } if request_id == "req-1" && *allowed
    )));

    assert_eq!(
        svc.resolve_approval(&alice, &id, "req-1".into(), ApprovalDecision::Allow)
            .await
            .err(),
        Some(crate::error::CoreError::InvalidState),
        "an answered approval cannot be resolved a second time"
    );
}

#[tokio::test]
async fn answering_something_nothing_is_waiting_for_is_refused() {
    // A runtime handed an answer to a request it is not blocked on will either
    // ignore it or act on the wrong thing, and neither is visible to whoever
    // answered.
    let (svc, alice, id, _adapter) = conversation().await;
    assert_eq!(
        svc.resolve_approval(&alice, &id, "never-asked".into(), ApprovalDecision::Allow)
            .await
            .err(),
        Some(crate::error::CoreError::InvalidState)
    );
}

#[tokio::test]
async fn a_conversation_session_is_started_with_the_mode_its_owner_chose() {
    // How much the agent may do unattended is settled per conversation on some
    // kernels, so the runtime is told the same intent it was provisioned with.
    // Getting this wrong silently grants more than the person asked for.
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter {
        capabilities: crate::conversation_support::conversing(),
        ..FakeAdapter::default()
    });
    let profiles =
        crate::test_support::environments_defaulting_to(crate::environment::AgentConfiguration {
            kernel: crate::test_support::kernel(),
            mode: crate::options::SessionMode::Conversation,
            permission: crate::options::PermissionMode::Autonomous,
            ..Default::default()
        });
    let svc = crate::test_support::service_with_environments(
        Arc::clone(&store),
        Arc::clone(&adapter),
        profiles,
    );
    let alice = user("alice");
    let created = svc.create_session(&alice, spec()).await.expect("created");
    await_settled(&svc, &alice, &created.id).await;

    let provisioned = adapter.last_spec.lock().await.clone().expect("provisioned");
    assert_eq!(
        provisioned.options.mode,
        layer3_adapter::RuntimeMode::Conversation
    );
    assert_eq!(
        provisioned.options.permission,
        layer3_adapter::RuntimePermission::Autonomous,
        "the runtime must be told the intent its owner chose"
    );
}

#[tokio::test]
async fn a_turn_that_failed_is_not_recorded_as_one_that_finished() {
    // The requirement this exists for: whatever goes wrong underneath — the
    // agent unable to reach its model, a bridge that died, a prompt that was
    // never answered — has to reach whoever is reading the conversation.
    // Reported as an ending, all of those looked identical to an agent with
    // nothing to say, which is the one reading that helps nobody.
    let (svc, alice, id, adapter) = conversation().await;
    {
        let held = adapter.conversation.lock().await;
        let kernel = held.as_ref().expect("the conversation was opened");
        kernel
            .updates
            .send(layer3_adapter::ConversationUpdate::Failed {
                message: "could not reach the model endpoint".to_string(),
                capability: None,
            })
            .await
            .expect("the core is listening");
    }

    let recorded = until_logged(&svc, &alice, &id, |kind| {
        matches!(
            kind,
            EventKind::StatusChanged {
                stop_reason: Some(StopReason::Error),
                ..
            }
        )
    })
    .await;
    assert!(recorded, "a failed turn is recorded as a failure");

    let events = svc
        .events_since(&alice, &id, None)
        .await
        .expect("the log is readable");
    let stated = events.iter().rev().find_map(|e| match &e.kind {
        EventKind::StatusChanged { detail, .. } => detail.clone(),
        _ => None,
    });
    assert_eq!(
        stated.as_deref(),
        Some("could not reach the model endpoint"),
        "the reason travels in the words of whatever knew it"
    );
}
