//! Driving a conversation: prompting, interrupting, answering and metering.
//!
//! Split from the folding tests so both stay within the project's file-size
//! limit, on a real division: those are about what a runtime says, and these
//! are about what an owner does.

use std::sync::Arc;

use layer3_adapter::{ContentBlock, ConversationInput, PermissionDecision};

use crate::conversation::ApprovalDecision;
use crate::conversation_support::{
    asking, chunk, conversation, conversing, said, until_logged, until_sent,
};
use crate::event::EventKind;
use crate::session::{SessionStatus, StopReason};
use crate::store::SessionStore;
use crate::test_support::{
    await_settled, environments_defaulting_to, service, service_with_environments,
    service_with_environments_on_version, spec, user, FakeAdapter, FakeStore,
};

#[tokio::test]
async fn a_prompt_reaches_the_runtime() {
    let (svc, alice, id, adapter) = conversation().await;
    svc.send_prompt(&alice, &id, "what is in this repository?".into())
        .await
        .expect("the owner may prompt their own session");

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
async fn every_conversation_input_is_refused_while_rotating_and_reusable_afterward() {
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter {
        capabilities: conversing(),
        ..FakeAdapter::default()
    });
    let profiles = environments_defaulting_to(crate::environment::AgentConfiguration {
        kernel: crate::test_support::kernel(),
        mode: crate::options::SessionMode::Conversation,
        ..Default::default()
    });
    let svc = service_with_environments(Arc::clone(&store), Arc::clone(&adapter), profiles);
    let alice = user("rotating-conversation-owner");
    let created = svc.create_session(&alice, spec()).await.expect("created");
    await_settled(&svc, &alice, &created.id).await;

    let mut record = store
        .get(&created.id)
        .await
        .expect("readable")
        .expect("present");
    record.session.status = SessionStatus::Rotating;
    store.seed(&record).await.expect("rotation is durable");
    {
        let mut entries = svc.entries.write().await;
        let entry = entries.get_mut(&created.id).expect("live entry");
        entry.session.status = SessionStatus::Rotating;
        entry.pending_approvals.insert("approval-1".into());
    }

    assert_eq!(
        svc.send_prompt(&alice, &created.id, "must wait".into())
            .await,
        Err(crate::error::CoreError::SessionRotating)
    );
    assert_eq!(
        svc.interrupt_turn(&alice, &created.id).await,
        Err(crate::error::CoreError::SessionRotating)
    );
    assert_eq!(
        svc.resolve_approval(
            &alice,
            &created.id,
            "approval-1".into(),
            ApprovalDecision::Allow,
        )
        .await,
        Err(crate::error::CoreError::SessionRotating)
    );
    assert!(
        adapter
            .conversation
            .lock()
            .await
            .as_ref()
            .expect("conversation is open")
            .sent()
            .await
            .is_empty(),
        "nothing may reach the outgoing conversation"
    );

    record.session.status = SessionStatus::Ready;
    store.seed(&record).await.expect("handover completed");
    svc.entries
        .write()
        .await
        .get_mut(&created.id)
        .expect("replacement entry")
        .session
        .status = SessionStatus::Ready;

    svc.send_prompt(&alice, &created.id, "retry now".into())
        .await
        .expect("the same prompt succeeds after rotation");
    svc.interrupt_turn(&alice, &created.id)
        .await
        .expect("the same interrupt succeeds after rotation");
    svc.resolve_approval(
        &alice,
        &created.id,
        "approval-1".into(),
        ApprovalDecision::Allow,
    )
    .await
    .expect("the same approval succeeds after rotation");

    let sent = until_sent(&adapter, |input| {
        matches!(input, ConversationInput::PermissionAnswer { .. })
    })
    .await;
    assert!(sent
        .iter()
        .any(|input| matches!(input, ConversationInput::Prompt(_))));
    assert!(sent
        .iter()
        .any(|input| matches!(input, ConversationInput::Cancel)));
    assert!(sent
        .iter()
        .any(|input| matches!(input, ConversationInput::PermissionAnswer { .. })));

    record.session.status = SessionStatus::Terminated;
    store.seed(&record).await.expect("termination is durable");
    svc.entries
        .write()
        .await
        .get_mut(&created.id)
        .expect("terminal entry")
        .session
        .status = SessionStatus::Terminated;
    assert_eq!(
        svc.send_prompt(&alice, &created.id, "cannot retry".into())
            .await,
        Err(crate::error::CoreError::SessionTerminated)
    );
    assert_eq!(
        svc.interrupt_turn(&alice, &created.id).await,
        Err(crate::error::CoreError::SessionTerminated)
    );
    assert_eq!(
        svc.resolve_approval(
            &alice,
            &created.id,
            "approval-1".into(),
            ApprovalDecision::Allow,
        )
        .await,
        Err(crate::error::CoreError::SessionTerminated)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conversation_input_crosses_a_real_rotation_only_after_replacement() {
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter {
        capabilities: conversing(),
        ..FakeAdapter::default()
    });
    let profiles = environments_defaulting_to(crate::environment::AgentConfiguration {
        kernel: crate::test_support::kernel(),
        mode: crate::options::SessionMode::Conversation,
        ..Default::default()
    });
    let svc = service_with_environments_on_version(
        Arc::clone(&store),
        Arc::clone(&adapter),
        profiles,
        "runtime:2",
    );
    let alice = user("real-rotating-conversation-owner");
    let created = svc.create_session(&alice, spec()).await.expect("created");
    await_settled(&svc, &alice, &created.id).await;

    let outgoing = adapter
        .conversation
        .lock()
        .await
        .take()
        .expect("the outgoing conversation is open");
    let mut record = store
        .get(&created.id)
        .await
        .expect("readable")
        .expect("present");
    record.session.runtime_version = Some("runtime:1".into());
    store.seed(&record).await.expect("old version is durable");
    svc.entries
        .write()
        .await
        .get_mut(&created.id)
        .expect("outgoing entry")
        .session
        .runtime_version = Some("runtime:1".into());

    adapter.pause_next_provision();
    svc.pause_next_live_install();
    let reconciling = tokio::spawn({
        let svc = Arc::clone(&svc);
        async move { svc.reconcile().await }
    });
    adapter.wait_until_provision_paused().await;
    assert!(
        !svc.entries.read().await.contains_key(&created.id),
        "the outgoing conversation entry is gone during replacement provisioning"
    );
    assert_eq!(
        store
            .get(&created.id)
            .await
            .expect("rotation is readable")
            .expect("session remains durable")
            .session
            .status,
        SessionStatus::Rotating
    );

    const PROMPT: &str = "retry this exact prompt";
    const APPROVAL: &str = "approval-1";
    assert_eq!(
        svc.send_prompt(&alice, &created.id, PROMPT.into()).await,
        Err(crate::error::CoreError::SessionRotating)
    );
    assert_eq!(
        svc.interrupt_turn(&alice, &created.id).await,
        Err(crate::error::CoreError::SessionRotating)
    );
    assert_eq!(
        svc.resolve_approval(
            &alice,
            &created.id,
            APPROVAL.into(),
            ApprovalDecision::Allow,
        )
        .await,
        Err(crate::error::CoreError::SessionRotating)
    );
    let foreign = user("foreign-conversation-observer");
    assert_eq!(
        svc.send_prompt(&foreign, &created.id, PROMPT.into()).await,
        Err(crate::error::CoreError::Forbidden)
    );
    assert_eq!(
        svc.interrupt_turn(&foreign, &created.id).await,
        Err(crate::error::CoreError::Forbidden)
    );
    assert_eq!(
        svc.resolve_approval(
            &foreign,
            &created.id,
            APPROVAL.into(),
            ApprovalDecision::Allow,
        )
        .await,
        Err(crate::error::CoreError::Forbidden)
    );
    assert!(
        outgoing.sent().await.is_empty(),
        "nothing may reach the outgoing conversation"
    );

    adapter.resume_provision();
    svc.wait_until_live_install_paused().await;
    assert!(
        !svc.entries.read().await.contains_key(&created.id),
        "the replacement conversation is not live at the readiness boundary"
    );
    assert_eq!(
        store
            .get(&created.id)
            .await
            .expect("readiness is readable")
            .expect("session remains durable")
            .session
            .status,
        SessionStatus::Ready
    );
    assert_eq!(
        svc.send_prompt(&alice, &created.id, PROMPT.into()).await,
        Err(crate::error::CoreError::SessionRotating)
    );
    assert_eq!(
        svc.interrupt_turn(&alice, &created.id).await,
        Err(crate::error::CoreError::SessionRotating)
    );
    assert_eq!(
        svc.resolve_approval(
            &alice,
            &created.id,
            APPROVAL.into(),
            ApprovalDecision::Allow,
        )
        .await,
        Err(crate::error::CoreError::SessionRotating)
    );
    assert!(
        outgoing.sent().await.is_empty(),
        "the durable readiness boundary still cannot reach the outgoing conversation"
    );
    svc.resume_live_install();
    assert_eq!(
        reconciling
            .await
            .expect("reconciliation task completes")
            .rotated_sessions,
        1
    );
    let settled = await_settled(&svc, &alice, &created.id).await;
    assert_eq!(settled.runtime_version.as_deref(), Some("runtime:2"));
    let replacement = adapter
        .conversation
        .lock()
        .await
        .take()
        .expect("the replacement conversation is open");
    svc.entries
        .write()
        .await
        .get_mut(&created.id)
        .expect("replacement entry")
        .pending_approvals
        .insert(APPROVAL.into());

    svc.send_prompt(&alice, &created.id, PROMPT.into())
        .await
        .expect("the same prompt succeeds after rotation");
    svc.interrupt_turn(&alice, &created.id)
        .await
        .expect("the same interrupt succeeds after rotation");
    svc.resolve_approval(
        &alice,
        &created.id,
        APPROVAL.into(),
        ApprovalDecision::Allow,
    )
    .await
    .expect("the same approval succeeds after rotation");

    let sent = replacement.sent().await;
    assert_eq!(sent.len(), 3, "only the three retried inputs are delivered");
    assert!(matches!(
        &sent[0],
        ConversationInput::Prompt(content)
            if content == &vec![ContentBlock::from(PROMPT)]
    ));
    assert!(matches!(sent[1], ConversationInput::Cancel));
    assert!(matches!(
        &sent[2],
        ConversationInput::PermissionAnswer { id, decision }
            if id == APPROVAL && *decision == PermissionDecision::Allow
    ));
    assert!(
        outgoing.sent().await.is_empty(),
        "retries are delivered only to the replacement conversation"
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
