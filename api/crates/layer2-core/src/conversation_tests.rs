//! What a session's log holds once its runtime starts talking.
//!
//! The property under all of these is that the log is the record. Whatever a
//! client saw, a client that arrives later — or comes back after a restart —
//! must be able to reconstruct from the sequence alone.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use layer3_adapter::{KernelAdapter, PermissionLocator};

use super::pending_approvals_from;
use crate::authz::Caller;
use crate::conversation_support::{
    asking, chunk, conversation, conversing, offering, said, until_logged,
};
use crate::event::{Event, EventKind};
use crate::ids::SessionId;
use crate::service::SessionService;
use crate::session::{SessionStatus, StopReason};
use crate::test_support::{await_settled, service, spec, user, FakeAdapter, FakeStore};

fn logged(seq: u64, kind: EventKind) -> Event {
    Event {
        session_id: SessionId("sesn_pending_approvals".into()),
        seq,
        kind,
        appended_at: "2026-08-26T05:00:00Z".into(),
    }
}

#[test]
fn an_unresolved_approval_request_folds_as_pending() {
    let events = vec![logged(
        1,
        EventKind::ApprovalRequested {
            request_id: "approval-1".into(),
            action: Some("run a command".into()),
            detail: None,
            locator: None,
        },
    )];

    assert_eq!(
        pending_approvals_from(&events)
            .expect("history is consistent")
            .pending,
        HashMap::from([("approval-1".to_string(), None)])
    );
}

#[test]
fn a_resolved_approval_request_does_not_fold_as_pending() {
    let events = vec![
        logged(
            1,
            EventKind::ApprovalRequested {
                request_id: "approval-1".into(),
                action: Some("run a command".into()),
                detail: None,
                locator: None,
            },
        ),
        logged(
            2,
            EventKind::ApprovalResolved {
                request_id: "approval-1".into(),
                allowed: true,
            },
        ),
    ];

    assert!(pending_approvals_from(&events)
        .expect("history is consistent")
        .pending
        .is_empty());
}

#[test]
fn interleaved_approval_requests_fold_independently() {
    let events = vec![
        logged(
            1,
            EventKind::ApprovalRequested {
                request_id: "approval-1".into(),
                action: Some("first action".into()),
                detail: None,
                locator: None,
            },
        ),
        logged(2, EventKind::TurnStarted),
        logged(
            3,
            EventKind::ApprovalRequested {
                request_id: "approval-2".into(),
                action: Some("second action".into()),
                detail: None,
                locator: None,
            },
        ),
        logged(
            4,
            EventKind::ApprovalResolved {
                request_id: "approval-1".into(),
                allowed: false,
            },
        ),
        logged(
            5,
            EventKind::ApprovalRequested {
                request_id: "approval-3".into(),
                action: Some("third action".into()),
                detail: None,
                locator: None,
            },
        ),
        logged(
            6,
            EventKind::ApprovalResolved {
                request_id: "approval-3".into(),
                allowed: true,
            },
        ),
    ];

    assert_eq!(
        pending_approvals_from(&events)
            .expect("history is consistent")
            .pending,
        HashMap::from([("approval-2".to_string(), None)])
    );
}

#[test]
fn duplicate_requests_are_idempotent_but_changed_locators_fail_closed() {
    let first = PermissionLocator::new(r#"{"version":1,"call":7}"#);
    let different = PermissionLocator::new(r#"{"version":1,"call":8}"#);
    let request = |seq, locator| {
        logged(
            seq,
            EventKind::ApprovalRequested {
                request_id: "stable-id".into(),
                action: Some("run a command".into()),
                detail: None,
                locator: Some(locator),
            },
        )
    };

    let replayed = vec![request(1, first.clone()), request(2, first)];
    assert_eq!(
        pending_approvals_from(&replayed)
            .expect("an identical replay is one request")
            .pending
            .len(),
        1
    );

    let resolved_then_replayed = vec![
        request(1, PermissionLocator::new("resolved")),
        logged(
            2,
            EventKind::ApprovalResolved {
                request_id: "stable-id".into(),
                allowed: true,
            },
        ),
        request(3, PermissionLocator::new("resolved")),
    ];
    assert!(
        pending_approvals_from(&resolved_then_replayed)
            .expect("a late duplicate keeps its resolved identity")
            .pending
            .is_empty(),
        "a delivered request must not reopen after its resolution"
    );

    let changed = vec![
        request(1, PermissionLocator::new("first")),
        request(2, different),
    ];
    let error = pending_approvals_from(&changed)
        .expect_err("one public id cannot be redirected to a different call");
    assert!(error.contains("different delivery locator"), "got {error}");
}

#[test]
fn runtime_and_conversation_boundaries_expire_old_approvals() {
    let boundaries = vec![
        EventKind::TurnStarted,
        EventKind::TurnCompleted,
        EventKind::ConversationOpened {
            conversation_id: "replacement-conversation".into(),
        },
        EventKind::PlaneChanged {
            from: crate::options::SessionMode::Conversation,
            to: crate::options::SessionMode::Terminal,
            snapshot_at: "2026-08-26T06:00:00Z".into(),
        },
        EventKind::RuntimeLost {
            runtime_id: "runtime-old".into(),
            cause: Some(crate::session::RuntimeLossCause::Displaced),
        },
        EventKind::StatusChanged {
            status: SessionStatus::Idle,
            stop_reason: Some(StopReason::UserTerminated),
            detail: None,
            capability: None,
        },
        EventKind::StatusChanged {
            status: SessionStatus::Terminated,
            stop_reason: Some(StopReason::UserTerminated),
            detail: None,
            capability: None,
        },
        EventKind::StatusChanged {
            status: SessionStatus::Rotating,
            stop_reason: None,
            detail: None,
            capability: None,
        },
    ];

    for boundary in boundaries {
        let events = vec![
            logged(
                1,
                EventKind::ApprovalRequested {
                    request_id: "obsolete".into(),
                    action: None,
                    detail: None,
                    locator: None,
                },
            ),
            logged(2, boundary.clone()),
        ];
        assert!(
            pending_approvals_from(&events)
                .expect("the boundary is understood")
                .pending
                .is_empty(),
            "{boundary:?} must expire the old runtime's request"
        );
    }
}

#[test]
fn requires_action_status_preserves_the_request_it_parks() {
    let events = vec![
        logged(
            1,
            EventKind::ApprovalRequested {
                request_id: "still-waiting".into(),
                action: None,
                detail: None,
                locator: None,
            },
        ),
        logged(
            2,
            EventKind::StatusChanged {
                status: SessionStatus::Idle,
                stop_reason: Some(StopReason::RequiresAction),
                detail: None,
                capability: None,
            },
        ),
    ];
    assert!(pending_approvals_from(&events)
        .expect("the wait is valid")
        .pending
        .contains_key("still-waiting"));
}

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
    let link = service
        .entries
        .read()
        .await
        .get(&id)
        .expect("the live entry is installed")
        .conversation_link
        .expect("the live entry carries its conversation link");
    let input_gate = service.input_gates.gate(&id);
    let input_writer = input_gate.write_owned().await;
    let ordering = service.conversation_order_gates.gate(&id);
    let fold = service.fold_update(
        &id,
        link,
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
async fn opening_replay_larger_than_the_channel_is_folded_in_fifo_order() {
    let store = Arc::new(FakeStore::default());
    let mut replay = (0..80).map(|_| chunk("x")).collect::<Vec<_>>();
    replay.push(layer3_adapter::ConversationUpdate::TurnEnded(
        layer3_adapter::PromptResponse::new(layer3_adapter::TurnStopReason::EndTurn),
    ));
    replay.push(layer3_adapter::ConversationUpdate::Asked(asking(
        "opening-replay-approval",
        "approve after replay",
    )));
    let adapter = Arc::new(FakeAdapter {
        capabilities: conversing(),
        opening_updates: tokio::sync::Mutex::new(replay),
        ..FakeAdapter::default()
    });
    let service = versioned_conversation_service(store, adapter, "runtime:opening-replay");
    let owner = user("opening-replay-owner");
    let created = service
        .create_session(&owner, spec())
        .await
        .expect("the conversation is accepted");
    await_settled(&service, &owner, &created.id).await;
    assert!(
        until_logged(&service, &owner, &created.id, |kind| matches!(
            kind,
            EventKind::ApprovalRequested { request_id, .. }
                if request_id == "opening-replay-approval"
        ))
        .await,
        "the queued tail is folded after publication"
    );
    let settled = service
        .get_session(&owner, &created.id)
        .await
        .expect("the replayed session remains readable");
    assert_eq!(settled.status, SessionStatus::Idle);
    assert_eq!(settled.stop_reason, Some(StopReason::RequiresAction));

    let events = service
        .events_since(&owner, &created.id, None)
        .await
        .expect("opening replay is durable");
    let message = events
        .iter()
        .find(|event| matches!(&event.kind, EventKind::MessageCommitted { .. }))
        .expect("the replayed chunks are committed as one message");
    assert!(matches!(
        &message.kind,
        EventKind::MessageCommitted { content } if content == &"x".repeat(80)
    ));
    let completion = events
        .iter()
        .find(|event| matches!(event.kind, EventKind::TurnCompleted))
        .expect("the replayed turn completion is durable");
    assert!(
        message.seq < completion.seq,
        "the message must remain ahead of the completion that ended it"
    );
    service
        .resolve_approval(
            &owner,
            &created.id,
            "opening-replay-approval".into(),
            crate::conversation::ApprovalDecision::Deny,
        )
        .await
        .expect("provisioning installs the pending approval reconstructed by replay");
}

#[tokio::test]
async fn failed_opening_discards_ordinary_replay_and_preserves_the_runtime() {
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter {
        capabilities: conversing(),
        ..FakeAdapter::default()
    });
    let owner = user("failed-opening-owner");
    let first = versioned_conversation_service(
        Arc::clone(&store),
        Arc::clone(&adapter),
        "runtime:failed-opening",
    );
    let created = first
        .create_session(&owner, spec())
        .await
        .expect("the original conversation is accepted");
    assert_eq!(
        await_settled(&first, &owner, &created.id).await.status,
        SessionStatus::Ready
    );
    let before = first
        .events_since(&owner, &created.id, None)
        .await
        .expect("original history is readable")
        .last()
        .map_or(0, |event| event.seq);
    let original_runtime = first
        .record(&created.id)
        .await
        .expect("the original runtime is durable")
        .runtime_id;

    adapter
        .opening_updates
        .lock()
        .await
        .push(layer3_adapter::ConversationUpdate::Asked(asking(
            "discarded-opening-request",
            "must not become durable",
        )));
    adapter
        .refuse_conversation_open
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let replacement = versioned_conversation_service(
        Arc::clone(&store),
        Arc::clone(&adapter),
        "runtime:failed-opening",
    );
    replacement.reconcile().await;

    let later = replacement
        .events_since(&owner, &created.id, Some(before))
        .await
        .expect("history remains readable");
    assert!(
        later.iter().all(|event| !matches!(
            &event.kind,
            EventKind::ApprovalRequested { request_id, .. }
                if request_id == "discarded-opening-request"
        )),
        "ordinary replay from a refused opening must never be published"
    );
    let durable = replacement
        .record(&created.id)
        .await
        .expect("the existing session remains durable");
    assert_eq!(durable.runtime_id, original_runtime);
    assert_eq!(durable.session.status, SessionStatus::Ready);
}

#[tokio::test]
async fn generation_zero_replacement_fences_old_updates_and_the_actual_old_close() {
    let store = Arc::new(FakeStore::default());
    store.disable_coordination();
    let adapter = Arc::new(FakeAdapter {
        capabilities: conversing(),
        ..FakeAdapter::default()
    });
    let owner = user("compatibility-link-owner");
    let service = versioned_conversation_service(
        Arc::clone(&store),
        Arc::clone(&adapter),
        "runtime:compatibility-link",
    );
    let created = service
        .create_session(&owner, spec())
        .await
        .expect("the compatibility conversation is accepted");
    assert_eq!(
        await_settled(&service, &owner, &created.id).await.status,
        SessionStatus::Ready
    );

    let (old_link, old_updates) = {
        let entries = service.entries.read().await;
        let entry = entries.get(&created.id).expect("the first link is live");
        assert_eq!(entry.runtime_generation, 0);
        let updates = adapter
            .conversation
            .lock()
            .await
            .as_ref()
            .expect("the first runtime link is open")
            .updates
            .clone();
        (
            entry
                .conversation_link
                .expect("the first link has a local identity"),
            updates,
        )
    };
    let record = service
        .record(&created.id)
        .await
        .expect("the compatibility runtime is durable");
    service.entries.write().await.remove(&created.id);
    service
        .readopt(&record)
        .await
        .expect("the same runtime can publish a replacement link");
    let new_link = service
        .entries
        .read()
        .await
        .get(&created.id)
        .expect("the replacement link is live")
        .conversation_link
        .expect("the replacement has a local identity");
    assert_ne!(old_link, new_link);

    service
        .send_prompt(&owner, &created.id, "keep the successor running".into())
        .await
        .expect("the successor accepts a prompt");
    let before_stale = service
        .events_since(&owner, &created.id, None)
        .await
        .expect("the successor history is readable")
        .last()
        .map_or(0, |event| event.seq);

    let ordering = service.conversation_order_gates.gate(&created.id);
    let blocked = ordering.lock().await;
    old_updates
        .send(chunk("stale predecessor output"))
        .await
        .expect("the predecessor receiver is still alive");
    drop(old_updates);
    tokio::task::yield_now().await;
    drop(blocked);
    for _ in 0..2 {
        let barrier = ordering.lock().await;
        drop(barrier);
        tokio::task::yield_now().await;
    }

    let entry = service.entries.read().await;
    let successor = entry
        .get(&created.id)
        .expect("the successor entry remains installed");
    assert_eq!(successor.session.status, SessionStatus::Running);
    assert!(successor.assembling.is_none());
    drop(entry);
    assert!(
        service
            .events_since(&owner, &created.id, Some(before_stale))
            .await
            .expect("the successor history remains readable")
            .is_empty(),
        "the predecessor's queued frame and channel close must not mutate the successor"
    );
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

    let revoked_owner = Caller::new(alice.subject.clone(), Default::default());
    assert_eq!(
        svc.get_usage(&revoked_owner, &created.id).await.err(),
        Some(crate::error::CoreError::Forbidden),
        "authorization is checked before the session's capability"
    );
    assert_eq!(
        svc.get_usage(&user("mallory"), &created.id).await.err(),
        Some(crate::error::CoreError::Forbidden),
        "ownership is checked before revealing the session's capability"
    );
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
    // The opening receiver is the only authority allowed to persist a frame
    // before the live entry exists. Drive the adapter's real opening path so
    // this cannot pass through the stale-link fallback the durable protocol
    // deliberately removed.
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter {
        capabilities: conversing(),
        opening_updates: tokio::sync::Mutex::new(vec![said(
            layer3_adapter::SessionUpdate::AvailableCommandsUpdate(offering(&[(
                "compact",
                "Summarise the conversation so far",
            )])),
        )]),
        ..FakeAdapter::default()
    });
    let svc = versioned_conversation_service(store, adapter, "runtime:opening-commands");
    let alice = user("opening-commands-owner");
    let created = svc
        .create_session(&alice, spec())
        .await
        .expect("the conversation is accepted");
    await_settled(&svc, &alice, &created.id).await;

    assert!(
        until_logged(&svc, &alice, &created.id, |kind| matches!(
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
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter {
        capabilities: conversing(),
        opening_updates: tokio::sync::Mutex::new(vec![
            layer3_adapter::ConversationUpdate::SignInRequired {
                url: "https://auth.example.test/device".to_string(),
                code: Some("WDJB-MJHT".to_string()),
                message: None,
            },
        ]),
        ..FakeAdapter::default()
    });
    let svc = versioned_conversation_service(store, adapter, "runtime:opening-sign-in");
    let alice = user("opening-sign-in-owner");
    let created = svc
        .create_session(&alice, spec())
        .await
        .expect("the conversation is accepted");
    await_settled(&svc, &alice, &created.id).await;

    assert!(
        until_logged(&svc, &alice, &created.id, |kind| matches!(
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

#[tokio::test]
async fn linear_prompt_metadata_is_durable_but_ordinary_prompt_history_is_unchanged() {
    let (ordinary, owner, ordinary_id, _) = conversation().await;
    let ordinary_turn = ordinary
        .send_prompt(&owner, &ordinary_id, "ordinary input".into())
        .await
        .expect("ordinary input is accepted");
    let ordinary_events = ordinary
        .events_since(&owner, &ordinary_id, None)
        .await
        .expect("ordinary history is readable");
    assert!(ordinary_events.iter().any(|event| {
        event.seq == ordinary_turn && matches!(event.kind, EventKind::TurnStarted)
    }));
    assert!(ordinary_events
        .iter()
        .all(|event| !matches!(event.kind, EventKind::TurnMetadata { .. })));

    let (linear, owner, linear_id, _) = conversation().await;
    let metadata = BTreeMap::from([
        ("request".into(), "response-create".into()),
        ("trace".into(), "linear-one".into()),
    ]);
    let linear_turn = linear
        .send_linear_prompt(
            &owner,
            &linear_id,
            "linear input".into(),
            None,
            metadata.clone(),
        )
        .await
        .expect("the first linear input is accepted");
    let linear_events = linear
        .events_since(&owner, &linear_id, None)
        .await
        .expect("linear history is readable");
    let turn_index = linear_events
        .iter()
        .position(|event| event.seq == linear_turn && matches!(event.kind, EventKind::TurnStarted))
        .expect("the returned turn boundary is durable");
    assert!(matches!(
        &linear_events[turn_index + 1].kind,
        EventKind::OwnerMessage { content } if content == "linear input"
    ));
    assert!(matches!(
        &linear_events[turn_index + 2].kind,
        EventKind::TurnMetadata { metadata: retained } if retained == &metadata
    ));
}

#[tokio::test]
async fn linear_metadata_bounds_are_enforced_before_runtime_delivery() {
    let (service, owner, id, adapter) = conversation().await;
    let cases = [
        (0..=crate::MAX_TURN_METADATA_PAIRS)
            .map(|index| (format!("key-{index}"), "value".into()))
            .collect(),
        BTreeMap::from([(
            "k".repeat(crate::MAX_TURN_METADATA_KEY_CHARS + 1),
            "value".into(),
        )]),
        BTreeMap::from([(
            "key".into(),
            "v".repeat(crate::MAX_TURN_METADATA_VALUE_CHARS + 1),
        )]),
    ];
    for metadata in cases {
        assert!(matches!(
            service
                .send_linear_prompt(&owner, &id, "must not arrive".into(), None, metadata)
                .await,
            Err(crate::CoreError::InvalidRequest(_))
        ));
    }

    let sent = adapter
        .conversation
        .lock()
        .await
        .as_ref()
        .expect("the conversation is open")
        .sent()
        .await;
    assert!(
        sent.is_empty(),
        "invalid metadata must be refused pre-delivery"
    );
    assert!(service
        .events_since(&owner, &id, None)
        .await
        .unwrap()
        .iter()
        .all(|event| !matches!(event.kind, EventKind::TurnStarted)));
}

#[tokio::test]
async fn a_stale_linear_predecessor_is_refused_before_runtime_delivery() {
    let (service, owner, id, adapter) = conversation().await;
    let error = service
        .send_linear_prompt(
            &owner,
            &id,
            "fork attempt".into(),
            Some(99),
            BTreeMap::new(),
        )
        .await
        .expect_err("a nonexistent predecessor cannot be extended");
    assert_eq!(
        error,
        crate::CoreError::InvalidState {
            detail: Some(crate::CONVERSATION_ADVANCED_DETAIL.into())
        }
    );
    assert!(
        adapter
            .conversation
            .lock()
            .await
            .as_ref()
            .expect("the conversation is open")
            .sent()
            .await
            .is_empty(),
        "a stale branch must never reach the runtime"
    );
}

#[tokio::test]
async fn a_second_linear_prompt_names_the_live_response_instead_of_a_stale_predecessor() {
    let (service, owner, id, adapter) = conversation().await;
    let turn = service
        .send_linear_prompt(&owner, &id, "first".into(), None, BTreeMap::new())
        .await
        .expect("the first response starts");
    let error = service
        .send_linear_prompt(&owner, &id, "must wait".into(), Some(turn), BTreeMap::new())
        .await
        .expect_err("linear input cannot steer the live response");
    assert_eq!(
        error,
        crate::CoreError::InvalidState {
            detail: Some(crate::CONVERSATION_IN_PROGRESS_DETAIL.into())
        }
    );
    assert_eq!(
        adapter
            .conversation
            .lock()
            .await
            .as_ref()
            .expect("the conversation is open")
            .sent()
            .await
            .len(),
        1,
        "the refused input never reaches the runtime"
    );
}

#[tokio::test]
async fn simultaneous_linear_prompts_cannot_fork_one_conversation_end() {
    let (service, owner, id, _) = conversation().await;
    let first =
        service.send_linear_prompt(&owner, &id, "first contender".into(), None, BTreeMap::new());
    let second = service.send_linear_prompt(
        &owner,
        &id,
        "second contender".into(),
        None,
        BTreeMap::new(),
    );
    let (first, second) = tokio::join!(first, second);
    assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
    assert!(first.is_err() || second.is_err());

    let events = service
        .events_since(&owner, &id, None)
        .await
        .expect("the winning history is readable");
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event.kind, EventKind::TurnStarted))
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event.kind, EventKind::OwnerMessage { .. }))
            .count(),
        1
    );
}

#[tokio::test]
async fn a_completed_linear_turn_can_only_be_extended_by_its_exact_response() {
    let (service, owner, id, adapter) = conversation().await;
    let first = service
        .send_linear_prompt(&owner, &id, "first turn".into(), None, BTreeMap::new())
        .await
        .expect("the first turn starts");
    adapter
        .conversation
        .lock()
        .await
        .as_ref()
        .expect("the conversation is open")
        .updates
        .send(layer3_adapter::ConversationUpdate::TurnEnded(
            layer3_adapter::PromptResponse::new(layer3_adapter::TurnStopReason::EndTurn),
        ))
        .await
        .expect("the core receives completion");
    assert!(
        until_logged(&service, &owner, &id, |kind| matches!(
            kind,
            EventKind::TurnCompleted
        ))
        .await
    );

    let stale = service
        .send_linear_prompt(
            &owner,
            &id,
            "missing predecessor".into(),
            None,
            BTreeMap::new(),
        )
        .await
        .expect_err("omitting the established predecessor is a fork");
    assert!(matches!(stale, crate::CoreError::InvalidState { .. }));

    let second = service
        .send_linear_prompt(
            &owner,
            &id,
            "second turn".into(),
            Some(first),
            BTreeMap::new(),
        )
        .await
        .expect("the exact predecessor extends the conversation");
    assert!(second > first);
}
