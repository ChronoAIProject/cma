use std::collections::BTreeMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use layer3_adapter::{
    AdapterError, ContentBlock, ConversationInput, KernelAdapter, RuntimeLossReason,
    WorkspaceCheckpointDisposition,
};

use super::checkpoint_service_tests::{
    build_service, fixture, host, use_package, CheckpointFixture,
};
use super::*;
use crate::store::{ChatHostStore, SessionStore};
use crate::{
    ChatPauseOutcome, ChatPromptDeliveryState, ChatRevivePhase, ChatUnavailableCause, CoreError,
    EventKind, RecoveryState, SessionId, SessionStatus, StopReason, StoredEventKind,
};

const OLD_ACTIVITY: &str = "2026-09-01T01:45:00Z";
const PAUSE_AT: &str = "2026-09-01T02:00:01Z";

async fn pause(fixture: &CheckpointFixture, marker: &str) -> ChatHostRecord {
    for event in fixture.sessions.events.lock().await.iter_mut() {
        event.appended_at = OLD_ACTIVITY.into();
    }
    use_package(fixture, 1, marker).await;
    let id = host(fixture).await.id;
    assert!(matches!(
        fixture
            .service
            .pause_chat_host(&id, PAUSE_AT)
            .await
            .unwrap(),
        ChatPauseOutcome::Complete(_)
    ));
    let paused = host(fixture).await;
    assert_eq!(paused.status, ChatHostStatus::Paused);
    paused
}

async fn wait_for_host_status(
    fixture: &CheckpointFixture,
    status: ChatHostStatus,
) -> ChatHostRecord {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let current = host(fixture).await;
            if current.status == status {
                break current;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the chat host reaches its durable target state")
}

async fn runtime_loss_count(fixture: &CheckpointFixture, session_id: &SessionId) -> usize {
    fixture
        .sessions
        .events
        .lock()
        .await
        .iter()
        .filter(|event| event.session_id == *session_id)
        .filter(|event| {
            matches!(
                event.kind,
                StoredEventKind::Known(EventKind::RuntimeLost { .. })
            )
        })
        .count()
}

fn prompt_text(input: &ConversationInput) -> Option<&str> {
    let ConversationInput::Prompt(content) = input else {
        return None;
    };
    match content.as_slice() {
        [ContentBlock::Text(text)] => Some(text.text.as_str()),
        _ => None,
    }
}

fn default_prefixed(text: &str) -> String {
    super::threads::compose_chat_message(&ChatSettings::default().message_prefixes, text)
}

#[tokio::test]
async fn paused_message_restores_every_thread_and_delivers_the_admitted_turn() {
    let fixture = fixture("revive-message", 3).await;
    let paused = pause(&fixture, "dirty-worktree-marker").await;
    let target = fixture.threads[2].id.clone();
    fixture.adapter.chat_workspace_events.lock().await.clear();

    let turn_started = fixture
        .service
        .send_prompt(&fixture.caller, &target, "continue from disk".into())
        .await
        .expect("the paused message revives and is delivered");

    let revived = host(&fixture).await;
    assert_eq!(revived.status, ChatHostStatus::Active);
    assert_eq!(revived.runtime_generation, 2);
    assert_eq!(revived.rotations, 0);
    assert!(revived.revived_at.is_some());
    assert!(revived.revive_failed_threads.is_empty());
    assert_eq!(revived.last_transition_claim, None);
    let admission = revived.prompt_admissions.last().unwrap();
    assert_eq!(admission.session_id, target);
    assert_eq!(admission.state, ChatPromptDeliveryState::Delivered);
    assert_eq!(admission.turn_started, Some(turn_started));

    let replacement = fixture.adapter.last_spec.lock().await.clone().unwrap();
    assert_eq!(replacement.generation.unwrap().get(), 2);
    assert!(replacement
        .reservation_token
        .as_deref()
        .is_some_and(|token| !token.is_empty()));
    let workspace = replacement
        .workspace
        .expect("the complete archive is carried");
    assert_eq!(
        workspace.archive_sha256,
        paused.checkpoint.as_ref().unwrap().workspace_archive_sha256
    );
    assert!(workspace
        .archive
        .windows("dirty-worktree-marker".len())
        .any(|window| window == b"dirty-worktree-marker"));

    let restores = fixture.adapter.conversation_restores.lock().await.clone();
    assert_eq!(restores.len(), 3);
    for thread in &fixture.threads {
        assert!(restores.iter().any(|(_, slot, conversation)| {
            slot == thread.id.as_str() && conversation == &format!("conversation-for-{}", thread.id)
        }));
        let record = fixture.sessions.get(&thread.id).await.unwrap().unwrap();
        assert_eq!(
            record.session.status,
            if thread.id == target {
                SessionStatus::Running
            } else {
                SessionStatus::Ready
            }
        );
        assert_eq!(record.session.recovery_state, RecoveryState::Normal);
        assert_eq!(record.session.stop_reason, None);
        assert_eq!(record.runtime_id, revived.runtime_id);
        assert_eq!(
            record
                .chat_origin
                .as_ref()
                .and_then(|origin| origin.paused_notice.as_ref()),
            None
        );
    }

    let options = fixture.adapter.conversation_options.lock().await.clone();
    let restored_options = &options[options.len() - 3..];
    for thread in &fixture.threads {
        let option = restored_options
            .iter()
            .find(|option| option.conversation_slot.as_deref() == Some(thread.id.as_str()))
            .expect("every slot is opened eagerly");
        assert_eq!(option.resume_conversation, None);
        assert_eq!(
            option.working_directory.as_deref(),
            thread.chat.as_ref().map(|chat| chat.worktree_path.as_str())
        );
    }
    assert_eq!(
        *fixture.adapter.chat_workspace_events.lock().await,
        [
            "workspace_restore",
            "conversation_snapshot_restore",
            "conversation_snapshot_restore",
            "conversation_snapshot_restore",
            "workspace_setup",
            "conversation_open",
            "conversation_open",
            "conversation_open",
        ],
        "revival must set up the restored workspace before any conversation reopens"
    );
    let received = fixture.adapter.conversation_inputs.lock().await;
    let target_inputs = received.get(target.as_str()).unwrap().lock().await.clone();
    assert_eq!(
        target_inputs
            .iter()
            .filter_map(prompt_text)
            .map(str::to_owned)
            .collect::<Vec<_>>(),
        [default_prefixed("continue from disk")]
    );
    let events = fixture
        .sessions
        .events
        .lock()
        .await
        .iter()
        .filter(|event| event.session_id == target)
        .filter_map(|event| match &event.kind {
            StoredEventKind::Known(kind) => Some(kind.clone()),
            StoredEventKind::Unknown { .. } => None,
        })
        .collect::<Vec<_>>();
    assert!(events.windows(2).any(|pair| matches!(
        pair,
        [EventKind::TurnStarted, EventKind::OwnerMessage { content }]
            if content == "continue from disk"
    )));
}

#[tokio::test]
async fn revive_setup_failure_retains_index_and_excerpt_through_queued_admission() {
    let fixture = fixture("revive-setup-failure", 2).await;
    pause(&fixture, "setup-failure-workspace").await;
    fixture.adapter.chat_workspace_events.lock().await.clear();
    *fixture.adapter.workspace_setup_failure_index.lock().await = Some(1);

    let error = fixture
        .service
        .send_prompt(
            &fixture.caller,
            &fixture.threads[0].id,
            "retain the setup failure evidence".into(),
        )
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        CoreError::ChatUnavailable(ChatUnavailableCause::ProvisioningFailed {
            phase: ChatProvisioningPhase::WorkspaceSetup,
            command_index: Some(1),
            retryable: true,
        })
    ));
    let failed = host(&fixture).await;
    assert_eq!(failed.status, ChatHostStatus::Paused);
    let failure = failed.last_failure.as_ref().unwrap();
    assert_eq!(failure.command_index, Some(1));
    assert_eq!(
        failure.output_excerpt.as_deref(),
        Some("bounded test failure")
    );
    let admission = failed.prompt_admissions.last().unwrap();
    assert_eq!(admission.state, ChatPromptDeliveryState::RecoveryRequired);
    assert_eq!(admission.failure_command_index, Some(1));
    assert_eq!(
        *fixture.adapter.chat_workspace_events.lock().await,
        [
            "workspace_restore",
            "conversation_snapshot_restore",
            "conversation_snapshot_restore",
            "workspace_setup",
        ],
        "a failed setup remains ahead of every conversation open"
    );
}

#[tokio::test]
async fn paused_message_records_image_drift_at_the_checkpoint_boundary() {
    let fixture = fixture("revive-paused-image-drift", 1).await;
    let paused = pause(&fixture, "paused-image-drift-workspace").await;
    let mut drifted = paused.clone();
    drifted.runtime_version = Some("runtime-chat-v0".into());
    fixture
        .hosts
        .update_if_revision(paused.revision, &drifted)
        .await
        .unwrap()
        .unwrap();

    fixture
        .service
        .send_prompt(
            &fixture.caller,
            &fixture.threads[0].id,
            "revive on the current image".into(),
        )
        .await
        .unwrap();

    let active = host(&fixture).await;
    assert_eq!(active.runtime_version.as_deref(), Some("runtime-chat-v1"));
    assert_eq!(active.runtime_generation, 2);
    assert_eq!(active.rotations, 1);
    assert_eq!(
        active.rotated_at,
        active
            .checkpoint
            .as_ref()
            .map(|checkpoint| checkpoint.captured_at.clone())
    );
    let record = fixture
        .sessions
        .get(&fixture.threads[0].id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.session.rotations, 1);
    assert_eq!(record.session.rotated_at, active.rotated_at);
}

#[tokio::test]
async fn one_conversation_restore_failure_is_typed_and_does_not_block_siblings() {
    let fixture = fixture("revive-one-failure", 3).await;
    pause(&fixture, "all-thread-workspace").await;
    let failed = fixture.threads[1].id.clone();
    fixture
        .adapter
        .fail_conversation_restores
        .lock()
        .await
        .insert(failed.0.clone());

    let error = fixture
        .service
        .send_prompt(&fixture.caller, &failed, "message for failed slot".into())
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        CoreError::ChatUnavailable(ChatUnavailableCause::RestoreFailed {
            phase: ChatRevivePhase::ConversationRestore,
            ..
        })
    ));
    let revived = host(&fixture).await;
    assert_eq!(revived.status, ChatHostStatus::Active);
    assert_eq!(revived.revive_failed_threads.len(), 1);
    assert_eq!(revived.revive_failed_threads[0].session_id, failed);

    for thread in &fixture.threads {
        let record = fixture.sessions.get(&thread.id).await.unwrap().unwrap();
        if thread.id == failed {
            assert_eq!(record.session.status, SessionStatus::Idle);
            assert_eq!(record.session.recovery_state, RecoveryState::Degraded);
            assert_eq!(record.session.stop_reason, Some(StopReason::Error));
        } else {
            assert_eq!(record.session.status, SessionStatus::Ready);
            assert_eq!(record.session.recovery_state, RecoveryState::Normal);
        }
    }
    assert_eq!(
        revived.prompt_admissions.last().unwrap().state,
        ChatPromptDeliveryState::RecoveryRequired
    );
    assert!(matches!(
        fixture
            .service
            .send_prompt(&fixture.caller, &failed, "retry the failed slot".into())
            .await,
        Err(CoreError::ChatUnavailable(
            ChatUnavailableCause::RestoreFailed {
                phase: ChatRevivePhase::ConversationRestore,
                ..
            }
        ))
    ));
}

#[tokio::test]
async fn runtime_loss_during_conversation_restore_aborts_the_entire_replacement() {
    let fixture = fixture("revive-restore-runtime-loss", 3).await;
    pause(&fixture, "runtime-loss-during-restore").await;
    fixture
        .adapter
        .lose_runtime_on_conversation_restore
        .store(true, Ordering::SeqCst);

    let error = fixture
        .service
        .send_prompt(
            &fixture.caller,
            &fixture.threads[1].id,
            "do not publish a dead replacement".into(),
        )
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        CoreError::ChatUnavailable(ChatUnavailableCause::RestoreFailed {
            phase: ChatRevivePhase::ConversationRestore,
            retryable: true,
        })
    ));
    let failed = host(&fixture).await;
    assert_eq!(failed.status, ChatHostStatus::Paused);
    assert_eq!(failed.runtime_id, None);
    assert_eq!(failed.revive_phase, Some(ChatRevivePhase::RecoveryRequired));
    assert_eq!(failed.revive_failed_threads.len(), fixture.threads.len());
    assert!(failed.revive_failed_threads.iter().all(|failure| {
        failure.phase == ChatRevivePhase::ConversationRestore && failure.retryable
    }));
    let admission = failed.prompt_admissions.last().unwrap();
    assert_eq!(admission.state, ChatPromptDeliveryState::RecoveryRequired);
    assert_eq!(
        admission.failure_phase,
        Some(ChatRevivePhase::ConversationRestore)
    );
    assert!(admission.failure_retryable);
    for thread in &fixture.threads {
        let record = fixture.sessions.get(&thread.id).await.unwrap().unwrap();
        assert_eq!(record.session.status, SessionStatus::Idle);
        assert_eq!(record.session.recovery_state, RecoveryState::Degraded);
        assert_eq!(record.session.stop_reason, Some(StopReason::Error));
    }
}

#[tokio::test]
async fn runtime_loss_while_reopening_a_thread_aborts_the_entire_replacement() {
    let fixture = fixture("revive-open-runtime-loss", 3).await;
    pause(&fixture, "runtime-loss-during-thread-open").await;
    fixture
        .adapter
        .lose_runtime_on_conversation_open
        .store(true, Ordering::SeqCst);

    let error = fixture
        .service
        .send_prompt(
            &fixture.caller,
            &fixture.threads[1].id,
            "do not isolate a dead runtime".into(),
        )
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        CoreError::ChatUnavailable(ChatUnavailableCause::RestoreFailed {
            phase: ChatRevivePhase::ThreadRestore,
            retryable: true,
        })
    ));
    let failed = host(&fixture).await;
    assert_eq!(failed.status, ChatHostStatus::Paused);
    assert_eq!(failed.runtime_id, None);
    assert_eq!(failed.revive_phase, Some(ChatRevivePhase::RecoveryRequired));
    assert_eq!(failed.revive_failed_threads.len(), fixture.threads.len());
    assert!(failed
        .revive_failed_threads
        .iter()
        .all(|failure| failure.phase == ChatRevivePhase::ThreadRestore && failure.retryable));
    for thread in &fixture.threads {
        let record = fixture.sessions.get(&thread.id).await.unwrap().unwrap();
        assert_eq!(record.session.status, SessionStatus::Idle);
        assert_eq!(record.session.recovery_state, RecoveryState::Degraded);
        assert_eq!(record.session.stop_reason, Some(StopReason::Error));
    }
}

#[tokio::test]
async fn one_thread_load_failure_is_typed_and_does_not_block_siblings() {
    let fixture = fixture("revive-one-load-failure", 3).await;
    pause(&fixture, "all-thread-load-workspace").await;
    let failed = fixture.threads[1].id.clone();
    fixture
        .adapter
        .fail_conversation_opens
        .lock()
        .await
        .insert(failed.0.clone());

    let error = fixture
        .service
        .send_prompt(&fixture.caller, &failed, "message for failed load".into())
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        CoreError::ChatUnavailable(ChatUnavailableCause::RestoreFailed {
            phase: ChatRevivePhase::ThreadRestore,
            ..
        })
    ));
    let revived = host(&fixture).await;
    assert_eq!(revived.status, ChatHostStatus::Active);
    assert_eq!(revived.revive_failed_threads.len(), 1);
    assert_eq!(revived.revive_failed_threads[0].session_id, failed);
    for thread in &fixture.threads {
        let record = fixture.sessions.get(&thread.id).await.unwrap().unwrap();
        if thread.id == failed {
            assert_eq!(record.session.status, SessionStatus::Idle);
            assert_eq!(record.session.recovery_state, RecoveryState::Degraded);
            assert_eq!(record.session.stop_reason, Some(StopReason::Error));
        } else {
            assert_eq!(record.session.status, SessionStatus::Ready);
            assert_eq!(record.session.recovery_state, RecoveryState::Normal);
            assert_eq!(record.runtime_id, revived.runtime_id);
        }
    }
}

#[tokio::test]
async fn tampered_checkpoint_fails_before_replacement_provisioning() {
    let fixture = fixture("revive-tampered", 1).await;
    pause(&fixture, "verified-before-tamper").await;
    fixture
        .checkpoints
        .tamper_next_read
        .store(true, Ordering::SeqCst);
    let provisions_before = fixture.adapter.provision_calls.load(Ordering::SeqCst);

    let error = fixture
        .service
        .send_prompt(
            &fixture.caller,
            &fixture.threads[0].id,
            "must not reach a fresh workspace".into(),
        )
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        CoreError::ChatUnavailable(ChatUnavailableCause::RestoreFailed {
            phase: ChatRevivePhase::CheckpointVerification,
            retryable: false,
        })
    ));
    assert_eq!(
        fixture.adapter.provision_calls.load(Ordering::SeqCst),
        provisions_before
    );
    let failed = host(&fixture).await;
    assert_eq!(failed.status, ChatHostStatus::Paused);
    assert_eq!(failed.revive_phase, Some(ChatRevivePhase::RecoveryRequired));
    assert_eq!(failed.runtime_id, None);
    assert_eq!(
        failed
            .last_operator_note
            .as_ref()
            .map(|note| note.code.as_str()),
        Some("recovery_required")
    );
}

#[tokio::test]
async fn prompts_queued_during_provision_are_delivered_in_host_admission_order() {
    let fixture = fixture("revive-queue-order", 2).await;
    pause(&fixture, "queued-order-workspace").await;
    let provisions_before = fixture.adapter.provision_calls.load(Ordering::SeqCst);
    fixture.adapter.pause_next_provision();
    let first_service = Arc::clone(&fixture.service);
    let first_caller = fixture.caller.clone();
    let first_id = fixture.threads[0].id.clone();
    let first = tokio::spawn(async move {
        first_service
            .send_prompt(&first_caller, &first_id, "first admitted".into())
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        while fixture.adapter.provision_calls.load(Ordering::SeqCst) == provisions_before {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("revival reaches replacement provisioning");
    fixture.adapter.wait_until_provision_paused().await;

    let second_service = Arc::clone(&fixture.service);
    let second_caller = fixture.caller.clone();
    let second_id = fixture.threads[1].id.clone();
    let second = tokio::spawn(async move {
        second_service
            .send_prompt(&second_caller, &second_id, "second admitted".into())
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if host(&fixture).await.prompt_admissions.len() == 2 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("both prompts become durable before provisioning resumes");
    let queued = host(&fixture).await;
    assert_eq!(
        queued
            .prompt_admissions
            .iter()
            .map(|admission| (admission.sequence, admission.payload.as_str()))
            .collect::<Vec<_>>(),
        [(1, "first admitted"), (2, "second admitted")]
    );

    fixture.adapter.resume_provision();
    first.await.unwrap().unwrap();
    second.await.unwrap().unwrap();
    let settled = host(&fixture).await;
    assert!(settled
        .prompt_admissions
        .iter()
        .all(|admission| admission.state == ChatPromptDeliveryState::Delivered));
    let received = fixture.adapter.conversation_inputs.lock().await;
    assert_eq!(
        received
            .get(fixture.threads[0].id.as_str())
            .unwrap()
            .lock()
            .await
            .iter()
            .filter_map(prompt_text)
            .map(str::to_owned)
            .collect::<Vec<_>>(),
        [default_prefixed("first admitted")]
    );
    assert_eq!(
        received
            .get(fixture.threads[1].id.as_str())
            .unwrap()
            .lock()
            .await
            .iter()
            .filter_map(prompt_text)
            .map(str::to_owned)
            .collect::<Vec<_>>(),
        [default_prefixed("second admitted")]
    );
}

#[tokio::test]
async fn active_host_keeps_later_prompts_behind_its_durable_queue() {
    let fixture = fixture("active-durable-queue-order", 1).await;
    let target = fixture.threads[0].id.clone();
    let mut queued = host(&fixture).await;
    queued.prompt_admission_sequence = 1;
    queued.prompt_admissions.push(ChatPromptAdmission::new(
        target.clone(),
        "first durable".into(),
        1,
        PAUSE_AT.into(),
    ));
    fixture
        .hosts
        .update_if_revision(queued.revision, &queued)
        .await
        .unwrap()
        .unwrap();

    fixture
        .service
        .send_prompt(&fixture.caller, &target, "second arrival".into())
        .await
        .expect("the later prompt joins the existing durable queue");

    let settled = host(&fixture).await;
    assert_eq!(settled.prompt_admissions.len(), 2);
    assert!(settled
        .prompt_admissions
        .iter()
        .all(|admission| admission.state == ChatPromptDeliveryState::Delivered));
    let received = fixture.adapter.conversation_inputs.lock().await;
    let inputs = received.get(target.as_str()).unwrap().lock().await;
    assert_eq!(
        inputs
            .iter()
            .filter_map(prompt_text)
            .map(str::to_owned)
            .collect::<Vec<_>>(),
        [
            default_prefixed("first durable"),
            default_prefixed("second arrival")
        ]
    );
}

#[tokio::test]
async fn active_host_admission_survives_the_preexisting_queue_draining() {
    let fixture = fixture("active-drained-durable-queue", 1).await;
    let target = fixture.threads[0].id.clone();

    let turn_started = fixture
        .service
        .send_prompt_to_paused_chat(
            &fixture.caller.subject,
            &target,
            "admit after the observed queue drained".into(),
            None,
        )
        .await
        .expect("the stale queue observation does not reject a valid admission");

    let settled = host(&fixture).await;
    let admission = settled.prompt_admissions.last().unwrap();
    assert_eq!(admission.session_id, target);
    assert_eq!(admission.state, ChatPromptDeliveryState::Delivered);
    assert_eq!(admission.turn_started, Some(turn_started));
}

#[tokio::test]
async fn credential_failure_never_publishes_the_replacement_host_as_active() {
    let fixture = fixture("revive-credential-failure", 2).await;
    pause(&fixture, "credential-failure-workspace").await;
    fixture
        .adapter
        .refuse_credential_refresh
        .store(true, Ordering::SeqCst);
    let provisions_before = fixture.adapter.provision_calls.load(Ordering::SeqCst);

    let error = fixture
        .service
        .send_prompt(
            &fixture.caller,
            &fixture.threads[0].id,
            "must stay behind the credential fence".into(),
        )
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        CoreError::ChatUnavailable(ChatUnavailableCause::ProvisioningFailed {
            phase: ChatProvisioningPhase::Credential,
            ..
        })
    ));
    assert_eq!(
        fixture.adapter.provision_calls.load(Ordering::SeqCst),
        provisions_before + 1
    );
    let failed = host(&fixture).await;
    assert_eq!(failed.status, ChatHostStatus::Paused);
    assert_eq!(failed.runtime_id, None);
    assert_eq!(failed.revive_phase, Some(ChatRevivePhase::RecoveryRequired));
    assert_eq!(
        failed.prompt_admissions.last().unwrap().state,
        ChatPromptDeliveryState::RecoveryRequired
    );
    assert!(failed
        .revive_failed_threads
        .iter()
        .all(|failure| failure.phase == ChatRevivePhase::PromptDelivery));
    for thread in &fixture.threads {
        let record = fixture.sessions.get(&thread.id).await.unwrap().unwrap();
        assert_eq!(record.session.status, SessionStatus::Idle);
        assert_eq!(record.session.recovery_state, RecoveryState::Degraded);
        assert_eq!(record.session.stop_reason, Some(StopReason::Error));
    }
    let received = fixture.adapter.conversation_inputs.lock().await;
    assert!(received
        .get(fixture.threads[0].id.as_str())
        .unwrap()
        .lock()
        .await
        .is_empty());
}

#[tokio::test]
async fn failed_replacement_teardown_retains_cleanup_authority_until_reconcile() {
    let fixture = fixture("revive-cleanup-debt", 2).await;
    pause(&fixture, "cleanup-debt-workspace").await;
    fixture
        .adapter
        .refuse_credential_refresh
        .store(true, Ordering::SeqCst);
    fixture.adapter.fail_terminations.store(1, Ordering::SeqCst);

    let error = fixture
        .service
        .send_prompt(
            &fixture.caller,
            &fixture.threads[0].id,
            "remain fenced through cleanup".into(),
        )
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        CoreError::ChatUnavailable(ChatUnavailableCause::ProvisioningFailed {
            phase: ChatProvisioningPhase::Credential,
            ..
        })
    ));

    let cleanup = host(&fixture).await;
    let replacement = cleanup
        .runtime_id
        .clone()
        .expect("failed cleanup retains the exact replacement handle");
    assert_eq!(cleanup.status, ChatHostStatus::Reviving);
    assert_eq!(
        cleanup.revive_phase,
        Some(ChatRevivePhase::RecoveryRequired)
    );
    assert!(cleanup.last_transition_claim.is_some());
    assert!(cleanup
        .last_failure
        .as_ref()
        .is_some_and(|failure| failure.cleanup_pending));
    assert!(fixture
        .adapter
        .running
        .lock()
        .await
        .contains_key(&replacement));
    let admission_count = cleanup.prompt_admissions.len();
    assert!(matches!(
        fixture
            .service
            .send_prompt(
                &fixture.caller,
                &fixture.threads[0].id,
                "do not admit across cleanup debt".into(),
            )
            .await,
        Err(CoreError::ChatUnavailable(
            ChatUnavailableCause::ProvisioningFailed {
                phase: ChatProvisioningPhase::Credential,
                ..
            }
        ))
    ));
    assert_eq!(
        host(&fixture).await.prompt_admissions.len(),
        admission_count
    );

    let restarted = build_service(
        Arc::clone(&fixture.adapter),
        Arc::clone(&fixture.hosts),
        Arc::clone(&fixture.sessions),
        Arc::clone(&fixture.checkpoints),
    );
    restarted.reconcile().await;
    let settled = wait_for_host_status(&fixture, ChatHostStatus::Paused).await;
    assert_eq!(settled.runtime_id, None);
    assert_eq!(settled.last_transition_claim, None);
    assert!(settled
        .last_failure
        .as_ref()
        .is_some_and(|failure| !failure.cleanup_pending));
    assert!(!fixture
        .adapter
        .running
        .lock()
        .await
        .contains_key(&replacement));
}

#[tokio::test]
async fn image_drift_rotates_every_thread_at_the_captured_boundary() {
    let fixture = fixture("rotate-image-drift", 3).await;
    use_package(&fixture, 1, "rotation-workspace").await;
    let before = host(&fixture).await;
    let old_runtime = before.runtime_id.clone().unwrap();
    let mut drifted = before.clone();
    drifted.runtime_version = Some("runtime-chat-v0".into());
    fixture
        .hosts
        .update_if_revision(before.revision, &drifted)
        .await
        .unwrap()
        .unwrap();
    fixture
        .adapter
        .hold_checkpoint
        .store(true, Ordering::SeqCst);

    let service = Arc::clone(&fixture.service);
    let host_id = drifted.id.clone();
    let rotation =
        tokio::spawn(async move { service.rotate_chat_host_on_image_drift(&host_id).await });
    tokio::time::timeout(
        Duration::from_secs(2),
        fixture.adapter.checkpoint_started.acquire(),
    )
    .await
    .expect("rotation reaches the held checkpoint")
    .unwrap()
    .forget();

    let rotating = host(&fixture).await;
    assert_eq!(rotating.status, ChatHostStatus::Pausing);
    assert_eq!(rotating.revive_cause, Some(ChatReviveCause::ImageDrift));
    for thread in &fixture.threads {
        let record = fixture.sessions.get(&thread.id).await.unwrap().unwrap();
        assert_eq!(record.session.status, SessionStatus::Rotating);
        assert!(matches!(
            fixture
                .service
                .send_prompt(&fixture.caller, &thread.id, "must wait".into())
                .await,
            Err(CoreError::SessionRotating)
        ));
    }

    fixture.adapter.continue_checkpoint.add_permits(1);
    assert!(rotation.await.unwrap().unwrap());
    let rotated = host(&fixture).await;
    assert_eq!(rotated.status, ChatHostStatus::Active);
    assert_eq!(rotated.runtime_version.as_deref(), Some("runtime-chat-v1"));
    assert_eq!(rotated.runtime_generation, 2);
    assert_eq!(rotated.rotations, 1);
    assert_eq!(
        rotated.rotated_at,
        rotated
            .checkpoint
            .as_ref()
            .map(|checkpoint| checkpoint.captured_at.clone())
    );
    assert_ne!(rotated.runtime_id.as_deref(), Some(old_runtime.as_str()));
    assert!(fixture
        .adapter
        .terminated
        .lock()
        .await
        .contains(&old_runtime));
    for thread in &fixture.threads {
        let record = fixture.sessions.get(&thread.id).await.unwrap().unwrap();
        assert_eq!(record.session.status, SessionStatus::Ready);
        assert_eq!(record.session.rotations, 1);
        assert_eq!(record.session.rotated_at, rotated.rotated_at);
        assert_eq!(record.runtime_id, rotated.runtime_id);
    }
}

#[tokio::test]
async fn deleting_the_final_thread_preempts_a_held_image_rotation_checkpoint() {
    let fixture = fixture("delete-held-image-rotation", 1).await;
    use_package(&fixture, 1, "delete-held-image-rotation-workspace").await;
    let before = host(&fixture).await;
    let old_runtime = before.runtime_id.clone().unwrap();
    let mut drifted = before.clone();
    drifted.runtime_version = Some("runtime-chat-v0".into());
    fixture
        .hosts
        .update_if_revision(before.revision, &drifted)
        .await
        .unwrap()
        .unwrap();
    fixture
        .adapter
        .hold_checkpoint
        .store(true, Ordering::SeqCst);
    *fixture.adapter.checkpoint_error.lock().await = Some(AdapterError::Unavailable(
        "held workspace checkpoint could not be safely recovered after runtime termination".into(),
    ));

    let service = Arc::clone(&fixture.service);
    let host_id = drifted.id.clone();
    let rotation =
        tokio::spawn(async move { service.rotate_chat_host_on_image_drift(&host_id).await });
    tokio::time::timeout(
        Duration::from_secs(2),
        fixture.adapter.checkpoint_started.acquire(),
    )
    .await
    .expect("image rotation reaches the held checkpoint")
    .unwrap()
    .forget();
    assert_eq!(host(&fixture).await.status, ChatHostStatus::Pausing);
    let held_lifecycle = fixture.service.lock_lifecycle(&fixture.threads[0].id).await;

    let service = Arc::clone(&fixture.service);
    let caller = fixture.caller.clone();
    let session_id = fixture.threads[0].id.clone();
    let deletion = tokio::spawn(async move { service.delete_session(&caller, &session_id).await });
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if fixture
                .adapter
                .terminated
                .lock()
                .await
                .contains(&old_runtime)
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("final-thread deletion terminates the runtime before waiting for the input fence");
    assert!(
        !deletion.is_finished(),
        "durable deletion still waits for the checkpoint holder to release its locks"
    );

    fixture.adapter.continue_checkpoint.add_permits(1);
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(2), rotation)
            .await
            .expect("rotation exits after runtime termination")
            .unwrap(),
        Err(CoreError::RuntimeRefused(_))
    ));
    assert!(
        !deletion.is_finished(),
        "durable deletion still waits for its session lifecycle fence"
    );
    drop(held_lifecycle);
    tokio::time::timeout(Duration::from_secs(2), deletion)
        .await
        .expect("deletion finishes after checkpoint capture releases its locks")
        .unwrap()
        .expect("the final chat thread is deleted");

    assert!(fixture
        .sessions
        .get(&fixture.threads[0].id)
        .await
        .unwrap()
        .is_none());
    let retired = host(&fixture).await;
    assert_eq!(retired.status, ChatHostStatus::Retired);
    assert_eq!(retired.runtime_id, None);
    assert!(retired.threads[0].removed);
    assert_eq!(
        fixture.adapter.terminated.lock().await.as_slice(),
        &[old_runtime]
    );
}

#[tokio::test]
async fn runtime_loss_records_every_thread_and_restores_the_newest_checkpoint() {
    let fixture = fixture("revive-runtime-loss", 3).await;
    use_package(&fixture, 1, "last-verified-workspace").await;
    let before = host(&fixture).await;
    let checkpoint = fixture
        .service
        .checkpoint_chat_host(&before.id, "2026-09-01T02:00:00Z")
        .await
        .unwrap();
    let ChatCheckpointOutcome::Complete(checkpoint) = checkpoint else {
        panic!("the recovery checkpoint was not captured");
    };
    let old_runtime = before.runtime_id.clone().unwrap();
    fixture
        .adapter
        .stop_serving_because(&old_runtime, RuntimeLossReason::Displaced)
        .await;
    fixture.service.reconcile().await;

    let recovered = host(&fixture).await;
    assert_eq!(recovered.status, ChatHostStatus::Active);
    assert_eq!(recovered.runtime_generation, 2);
    assert_eq!(recovered.rotations, 0);
    assert_eq!(
        recovered.rotated_at.as_deref(),
        Some(checkpoint.captured_at.as_str())
    );
    assert_ne!(recovered.runtime_id.as_deref(), Some(old_runtime.as_str()));
    assert!(fixture
        .adapter
        .terminated
        .lock()
        .await
        .contains(&old_runtime));
    for thread in &fixture.threads {
        let record = fixture.sessions.get(&thread.id).await.unwrap().unwrap();
        assert_eq!(record.session.status, SessionStatus::Ready);
        assert_eq!(record.session.recovery_state, RecoveryState::Normal);
        assert_eq!(record.runtime_id, recovered.runtime_id);
        assert_eq!(record.session.rotated_at, recovered.rotated_at);
        let losses = fixture
            .sessions
            .events
            .lock()
            .await
            .iter()
            .filter(|event| event.session_id == thread.id)
            .filter(|event| {
                matches!(
                    &event.kind,
                    StoredEventKind::Known(EventKind::RuntimeLost {
                        runtime_id,
                        cause: Some(crate::session::RuntimeLossCause::Displaced),
                    }) if runtime_id == &old_runtime
                )
            })
            .count();
        assert_eq!(losses, 1);
    }
}

#[tokio::test]
async fn runtime_loss_events_do_not_move_threads_before_the_host_fence() {
    let fixture = fixture("runtime-loss-host-fence", 2).await;
    use_package(&fixture, 1, "runtime-loss-host-fence-workspace").await;
    let before = host(&fixture).await;
    fixture
        .service
        .checkpoint_chat_host(&before.id, "2026-09-01T02:00:00Z")
        .await
        .unwrap();
    let checkpointed = host(&fixture).await;
    let old_runtime = checkpointed.runtime_id.clone().unwrap();
    fixture.adapter.stop_serving(&old_runtime).await;
    fixture.hosts.fail_updates.store(1, Ordering::SeqCst);

    assert!(fixture
        .service
        .recover_lost_chat_host(&checkpointed.id, &old_runtime, None)
        .await
        .is_err());

    let unfenced = host(&fixture).await;
    assert_eq!(unfenced.status, ChatHostStatus::Active);
    assert_eq!(unfenced.runtime_id.as_deref(), Some(old_runtime.as_str()));
    for thread in &fixture.threads {
        let record = fixture.sessions.get(&thread.id).await.unwrap().unwrap();
        assert_eq!(record.session.status, SessionStatus::Ready);
        assert_eq!(runtime_loss_count(&fixture, &thread.id).await, 1);
    }

    assert!(fixture
        .service
        .recover_lost_chat_host(&checkpointed.id, &old_runtime, None)
        .await
        .unwrap());
    let recovered = host(&fixture).await;
    assert_eq!(recovered.status, ChatHostStatus::Active);
    assert_ne!(recovered.runtime_id.as_deref(), Some(old_runtime.as_str()));
    for thread in &fixture.threads {
        assert_eq!(runtime_loss_count(&fixture, &thread.id).await, 1);
    }
}

#[tokio::test]
async fn restart_replays_the_exact_inactivity_pause_claim() {
    let fixture = fixture("restart-mid-pause", 2).await;
    for event in fixture.sessions.events.lock().await.iter_mut() {
        event.appended_at = OLD_ACTIVITY.into();
    }
    use_package(&fixture, 1, "restart-pause-workspace").await;
    fixture
        .adapter
        .hold_checkpoint
        .store(true, Ordering::SeqCst);
    let before = host(&fixture).await;
    let old_runtime = before.runtime_id.clone().unwrap();
    let service = Arc::clone(&fixture.service);
    let host_id = before.id.clone();
    let pausing = tokio::spawn(async move { service.pause_chat_host(&host_id, PAUSE_AT).await });
    tokio::time::timeout(
        Duration::from_secs(2),
        fixture.adapter.checkpoint_started.acquire(),
    )
    .await
    .expect("pause reaches the held checkpoint")
    .unwrap()
    .forget();
    let claimed = host(&fixture).await;
    assert_eq!(claimed.status, ChatHostStatus::Pausing);
    assert_eq!(claimed.checkpoint_sequence, 1);
    assert_eq!(claimed.checkpoint, None);
    pausing.abort();
    assert!(pausing.await.unwrap_err().is_cancelled());

    let restarted = build_service(
        Arc::clone(&fixture.adapter),
        Arc::clone(&fixture.hosts),
        Arc::clone(&fixture.sessions),
        Arc::clone(&fixture.checkpoints),
    );
    restarted.reconcile().await;

    let paused = host(&fixture).await;
    assert_eq!(paused.status, ChatHostStatus::Paused);
    assert_eq!(paused.runtime_id, None);
    assert_eq!(paused.last_transition_claim, None);
    assert_eq!(
        paused
            .checkpoint
            .as_ref()
            .map(|checkpoint| checkpoint.checkpoint_version),
        Some(1)
    );
    let requests = fixture.adapter.checkpoint_requests.lock().await;
    assert_eq!(requests.len(), 2);
    assert!(requests.iter().all(|(_, request)| {
        request.version == 1
            && request.disposition == WorkspaceCheckpointDisposition::HoldForTeardown
    }));
    assert!(fixture
        .adapter
        .terminated
        .lock()
        .await
        .contains(&old_runtime));
}

#[tokio::test]
async fn restart_finishes_a_published_pause_after_the_old_runtime_is_gone() {
    let fixture = fixture("restart-after-pause-teardown", 2).await;
    let active = host(&fixture).await;
    let old_runtime = active.runtime_id.clone().unwrap();
    let paused = pause(&fixture, "published-pause-workspace").await;
    let mut claimed = paused.clone();
    claimed.runtime_id = Some(old_runtime);
    claimed.last_transition_claim = Some("chpa_restart_after_teardown".into());
    claimed.transition_started_at = Some(PAUSE_AT.into());
    let claimed = fixture
        .hosts
        .update_if_revision(paused.revision, &claimed)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        claimed
            .checkpoint
            .as_ref()
            .map(|checkpoint| checkpoint.checkpoint_version),
        Some(claimed.checkpoint_sequence)
    );

    let restarted = build_service(
        Arc::clone(&fixture.adapter),
        Arc::clone(&fixture.hosts),
        Arc::clone(&fixture.sessions),
        Arc::clone(&fixture.checkpoints),
    );
    restarted.reconcile().await;

    let settled = host(&fixture).await;
    assert_eq!(settled.status, ChatHostStatus::Paused);
    assert_eq!(settled.runtime_id, None);
    assert_eq!(settled.last_transition_claim, None);
    assert_eq!(settled.transition_started_at, None);
    assert_eq!(settled.checkpoint, claimed.checkpoint);
    for thread in &fixture.threads {
        let record = fixture.sessions.get(&thread.id).await.unwrap().unwrap();
        assert_eq!(record.session.status, SessionStatus::Idle);
        assert_eq!(record.session.stop_reason, Some(StopReason::IdleTimeout));
    }
}

#[tokio::test]
async fn interrupted_pause_recovers_only_from_the_previous_published_checkpoint() {
    let fixture = fixture("pause-runtime-lost-before-publish", 2).await;
    use_package(&fixture, 1, "previous-published-workspace").await;
    let before = host(&fixture).await;
    let previous = fixture
        .service
        .checkpoint_chat_host(&before.id, "2026-09-01T01:30:00Z")
        .await
        .unwrap();
    let ChatCheckpointOutcome::Complete(previous) = previous else {
        panic!("the previous recovery checkpoint was not captured");
    };
    for event in fixture.sessions.events.lock().await.iter_mut() {
        event.appended_at = OLD_ACTIVITY.into();
    }
    use_package(&fixture, 2, "unpublished-pause-workspace").await;
    fixture
        .adapter
        .hold_checkpoint
        .store(true, Ordering::SeqCst);
    let checkpointed = host(&fixture).await;
    let old_runtime = checkpointed.runtime_id.clone().unwrap();
    let service = Arc::clone(&fixture.service);
    let host_id = checkpointed.id.clone();
    let pausing = tokio::spawn(async move { service.pause_chat_host(&host_id, PAUSE_AT).await });
    tokio::time::timeout(
        Duration::from_secs(2),
        fixture.adapter.checkpoint_started.acquire(),
    )
    .await
    .expect("pause reaches its unpublished held checkpoint")
    .unwrap()
    .forget();
    let claimed = host(&fixture).await;
    assert_eq!(claimed.status, ChatHostStatus::Pausing);
    assert_eq!(claimed.checkpoint_sequence, 2);
    assert_eq!(
        claimed
            .checkpoint
            .as_ref()
            .map(|checkpoint| checkpoint.checkpoint_version),
        Some(1)
    );
    pausing.abort();
    assert!(pausing.await.unwrap_err().is_cancelled());
    fixture
        .adapter
        .stop_serving_because(&old_runtime, RuntimeLossReason::Displaced)
        .await;

    let restarted = build_service(
        Arc::clone(&fixture.adapter),
        Arc::clone(&fixture.hosts),
        Arc::clone(&fixture.sessions),
        Arc::clone(&fixture.checkpoints),
    );
    restarted.reconcile().await;

    let active = host(&fixture).await;
    assert_eq!(active.status, ChatHostStatus::Active);
    assert_eq!(active.runtime_generation, 2);
    assert_eq!(active.checkpoint_sequence, 2);
    assert_eq!(
        active
            .checkpoint
            .as_ref()
            .map(|checkpoint| checkpoint.checkpoint_version),
        Some(1)
    );
    assert_eq!(
        active.rotated_at.as_deref(),
        Some(previous.captured_at.as_str())
    );
    let workspace = fixture
        .adapter
        .last_spec
        .lock()
        .await
        .clone()
        .and_then(|spec| spec.workspace)
        .expect("the prior verified workspace is restored");
    assert!(workspace
        .archive
        .windows("previous-published-workspace".len())
        .any(|window| window == b"previous-published-workspace"));
    assert!(!workspace
        .archive
        .windows("unpublished-pause-workspace".len())
        .any(|window| window == b"unpublished-pause-workspace"));
    for thread in &fixture.threads {
        assert_eq!(runtime_loss_count(&fixture, &thread.id).await, 1);
    }
}

#[tokio::test]
async fn restart_replays_the_exact_image_rotation_claim() {
    let fixture = fixture("restart-mid-image-rotation", 2).await;
    use_package(&fixture, 1, "restart-rotation-workspace").await;
    let before = host(&fixture).await;
    let old_runtime = before.runtime_id.clone().unwrap();
    let mut drifted = before.clone();
    drifted.runtime_version = Some("runtime-chat-v0".into());
    fixture
        .hosts
        .update_if_revision(before.revision, &drifted)
        .await
        .unwrap()
        .unwrap();
    fixture
        .adapter
        .hold_checkpoint
        .store(true, Ordering::SeqCst);
    let service = Arc::clone(&fixture.service);
    let host_id = drifted.id.clone();
    let rotating =
        tokio::spawn(async move { service.rotate_chat_host_on_image_drift(&host_id).await });
    tokio::time::timeout(
        Duration::from_secs(2),
        fixture.adapter.checkpoint_started.acquire(),
    )
    .await
    .expect("image rotation reaches the held checkpoint")
    .unwrap()
    .forget();
    for thread in &fixture.threads {
        let record = fixture.sessions.get(&thread.id).await.unwrap().unwrap();
        assert_eq!(record.session.status, SessionStatus::Rotating);
    }
    rotating.abort();
    assert!(rotating.await.unwrap_err().is_cancelled());

    let restarted = build_service(
        Arc::clone(&fixture.adapter),
        Arc::clone(&fixture.hosts),
        Arc::clone(&fixture.sessions),
        Arc::clone(&fixture.checkpoints),
    );
    restarted.reconcile().await;

    let active = host(&fixture).await;
    assert_eq!(active.status, ChatHostStatus::Active);
    assert_eq!(active.runtime_generation, 2);
    assert_eq!(active.rotations, 1);
    assert_eq!(active.runtime_version.as_deref(), Some("runtime-chat-v1"));
    assert_ne!(active.runtime_id.as_deref(), Some(old_runtime.as_str()));
    let requests = fixture.adapter.checkpoint_requests.lock().await;
    assert_eq!(requests.len(), 2);
    assert!(requests.iter().all(|(_, request)| request.version == 1));
    for thread in &fixture.threads {
        let record = fixture.sessions.get(&thread.id).await.unwrap().unwrap();
        assert_eq!(record.session.status, SessionStatus::Ready);
        assert_eq!(record.runtime_id, active.runtime_id);
        assert_eq!(record.session.rotations, 1);
    }
}

#[tokio::test]
async fn restart_finishes_image_rotation_after_teardown_before_the_revive_cas() {
    let fixture = fixture("restart-after-rotation-teardown", 2).await;
    use_package(&fixture, 1, "published-rotation-workspace").await;
    let active = host(&fixture).await;
    let old_runtime = active.runtime_id.clone().unwrap();
    let checkpoint = fixture
        .service
        .checkpoint_chat_host(&active.id, PAUSE_AT)
        .await
        .unwrap();
    assert!(matches!(checkpoint, ChatCheckpointOutcome::Complete(_)));
    let checkpointed = host(&fixture).await;

    {
        let mut sessions = fixture.sessions.sessions.lock().await;
        for thread in &fixture.threads {
            sessions
                .get_mut(thread.id.as_str())
                .expect("the retained chat thread exists")
                .session
                .status = SessionStatus::Rotating;
        }
    }
    let mut claimed = checkpointed.clone();
    claimed.status = ChatHostStatus::Pausing;
    claimed.last_transition_claim = Some("chro_restart_after_teardown".into());
    claimed.transition_started_at = Some(PAUSE_AT.into());
    claimed.revive_cause = Some(ChatReviveCause::ImageDrift);
    let claimed = fixture
        .hosts
        .update_if_revision(checkpointed.revision, &claimed)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        claimed
            .checkpoint
            .as_ref()
            .map(|reference| reference.checkpoint_version),
        Some(claimed.checkpoint_sequence)
    );
    fixture.adapter.stop_serving(&old_runtime).await;
    let provisions_before = fixture.adapter.provision_calls.load(Ordering::SeqCst);

    let restarted = build_service(
        Arc::clone(&fixture.adapter),
        Arc::clone(&fixture.hosts),
        Arc::clone(&fixture.sessions),
        Arc::clone(&fixture.checkpoints),
    );
    restarted.reconcile().await;

    let settled = host(&fixture).await;
    assert_eq!(settled.status, ChatHostStatus::Active);
    assert_eq!(settled.runtime_generation, 2);
    assert_eq!(settled.rotations, 1);
    assert_eq!(
        settled.rotated_at,
        claimed.checkpoint.map(|value| value.captured_at)
    );
    assert_ne!(settled.runtime_id.as_deref(), Some(old_runtime.as_str()));
    assert_eq!(
        fixture.adapter.provision_calls.load(Ordering::SeqCst),
        provisions_before + 1
    );
    for thread in &fixture.threads {
        let record = fixture.sessions.get(&thread.id).await.unwrap().unwrap();
        assert_eq!(record.session.status, SessionStatus::Ready);
        assert_eq!(record.runtime_id, settled.runtime_id);
    }
}

#[tokio::test]
async fn durable_host_transition_fences_prompts_before_thread_state_changes() {
    let fixture = fixture("durable-prompt-fence", 1).await;
    let before = host(&fixture).await;
    let target = fixture.threads[0].id.clone();
    let mut transitioning = before.clone();
    transitioning.status = ChatHostStatus::Pausing;
    transitioning.last_transition_claim = Some("chro_durable_prompt_fence".into());
    transitioning.transition_started_at = Some(PAUSE_AT.into());
    transitioning.revive_cause = Some(ChatReviveCause::ImageDrift);
    transitioning.checkpoint_sequence = 1;
    let transitioning = fixture
        .hosts
        .update_if_revision(before.revision, &transitioning)
        .await
        .unwrap()
        .unwrap();

    assert!(matches!(
        fixture
            .service
            .send_prompt(&fixture.caller, &target, "must not cross the fence".into())
            .await,
        Err(CoreError::SessionRotating)
    ));
    let mut ordinary_pause = transitioning.clone();
    ordinary_pause.revive_cause = None;
    fixture
        .hosts
        .update_if_revision(transitioning.revision, &ordinary_pause)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        fixture
            .service
            .send_prompt(&fixture.caller, &target, "still fenced".into())
            .await,
        Err(CoreError::ChatUnavailable(
            ChatUnavailableCause::RevivePending
        ))
    ));
    assert!(fixture
        .adapter
        .conversation_inputs
        .lock()
        .await
        .get(target.as_str())
        .unwrap()
        .lock()
        .await
        .is_empty());
}

#[tokio::test]
async fn runtime_loss_without_a_checkpoint_fails_closed_once_per_thread() {
    let fixture = fixture("runtime-loss-no-checkpoint", 2).await;
    let before = host(&fixture).await;
    let old_runtime = before.runtime_id.clone().unwrap();
    let provisions_before = fixture.adapter.provision_calls.load(Ordering::SeqCst);
    fixture
        .adapter
        .stop_serving_because(&old_runtime, RuntimeLossReason::Failed)
        .await;

    fixture.service.reconcile().await;
    fixture.service.reconcile().await;

    let failed = host(&fixture).await;
    assert_eq!(failed.status, ChatHostStatus::Paused);
    assert_eq!(failed.runtime_id, None);
    assert_eq!(failed.revive_phase, Some(ChatRevivePhase::RecoveryRequired));
    assert_eq!(
        fixture.adapter.provision_calls.load(Ordering::SeqCst),
        provisions_before
    );
    for thread in &fixture.threads {
        let record = fixture.sessions.get(&thread.id).await.unwrap().unwrap();
        assert_eq!(record.session.status, SessionStatus::Idle);
        assert_eq!(record.session.recovery_state, RecoveryState::Degraded);
        assert_eq!(record.session.stop_reason, Some(StopReason::Error));
        assert_eq!(runtime_loss_count(&fixture, &thread.id).await, 1);
    }
}

#[tokio::test]
async fn corrupt_runtime_loss_checkpoint_never_provisions_a_fresh_workspace() {
    let fixture = fixture("runtime-loss-corrupt-checkpoint", 2).await;
    use_package(&fixture, 1, "corrupt-loss-boundary").await;
    let before = host(&fixture).await;
    fixture
        .service
        .checkpoint_chat_host(&before.id, "2026-09-01T02:00:00Z")
        .await
        .unwrap();
    let checkpointed = host(&fixture).await;
    let old_runtime = checkpointed.runtime_id.clone().unwrap();
    fixture
        .checkpoints
        .tamper_next_read
        .store(true, Ordering::SeqCst);
    let provisions_before = fixture.adapter.provision_calls.load(Ordering::SeqCst);
    fixture.adapter.stop_serving(&old_runtime).await;

    fixture.service.reconcile().await;

    let failed = host(&fixture).await;
    assert_eq!(failed.status, ChatHostStatus::Paused);
    assert_eq!(failed.revive_phase, Some(ChatRevivePhase::RecoveryRequired));
    assert_eq!(
        fixture.adapter.provision_calls.load(Ordering::SeqCst),
        provisions_before
    );
    for thread in &fixture.threads {
        let record = fixture.sessions.get(&thread.id).await.unwrap().unwrap();
        assert_eq!(record.session.status, SessionStatus::Idle);
        assert_eq!(record.session.recovery_state, RecoveryState::Degraded);
        assert_eq!(runtime_loss_count(&fixture, &thread.id).await, 1);
    }
}

#[tokio::test]
async fn restart_adopts_one_existing_revive_generation_and_fences_ambiguous_prompt() {
    let fixture = fixture("restart-mid-revive", 2).await;
    let paused = pause(&fixture, "restart-revive-workspace").await;
    let mut reviving = paused.clone();
    reviving.status = ChatHostStatus::Reviving;
    reviving.runtime_generation = 2;
    reviving.last_transition_claim = Some("chrv_restart_mid_revive".into());
    reviving.transition_started_at = Some(PAUSE_AT.into());
    reviving.revive_cause = Some(ChatReviveCause::OwnerMessage);
    reviving.revive_phase = Some(ChatRevivePhase::RuntimeProvisioning);
    reviving.prompt_admission_sequence = 1;
    let mut admission = ChatPromptAdmission::new(
        fixture.threads[0].id.clone(),
        "ambiguous delivery".into(),
        1,
        PAUSE_AT.into(),
    );
    admission.state = ChatPromptDeliveryState::DispatchFenced;
    admission.dispatch_fenced_at = Some(PAUSE_AT.into());
    reviving.prompt_admissions.push(admission);
    fixture
        .hosts
        .update_if_revision(paused.revision, &reviving)
        .await
        .unwrap()
        .unwrap();

    let mut spec = fixture.adapter.last_spec.lock().await.clone().unwrap();
    spec.generation = Some(std::num::NonZeroU64::new(2).unwrap());
    spec.chat_host = Some(paused.id.0.clone());
    let first = fixture.adapter.provision(&spec).await.unwrap();
    let duplicate = fixture.adapter.provision(&spec).await.unwrap();
    let provisions_before_reconcile = fixture.adapter.provision_calls.load(Ordering::SeqCst);
    let restarted = build_service(
        Arc::clone(&fixture.adapter),
        Arc::clone(&fixture.hosts),
        Arc::clone(&fixture.sessions),
        Arc::clone(&fixture.checkpoints),
    );

    restarted.reconcile().await;
    let active = wait_for_host_status(&fixture, ChatHostStatus::Active).await;

    assert_eq!(
        fixture.adapter.provision_calls.load(Ordering::SeqCst),
        provisions_before_reconcile
    );
    assert!(matches!(
        active.runtime_id.as_deref(),
        Some(runtime) if runtime == first.0 || runtime == duplicate.0
    ));
    let discarded = if active.runtime_id.as_deref() == Some(first.0.as_str()) {
        duplicate.0.as_str()
    } else {
        first.0.as_str()
    };
    assert!(fixture
        .adapter
        .terminated
        .lock()
        .await
        .iter()
        .any(|runtime| runtime == discarded));
    assert_eq!(active.prompt_admissions.len(), 1);
    assert_eq!(
        active.prompt_admissions[0].state,
        ChatPromptDeliveryState::RecoveryRequired
    );
    assert_eq!(
        active.prompt_admissions[0].failure_phase,
        Some(ChatRevivePhase::PromptDelivery)
    );
}

#[tokio::test]
async fn stale_reviving_scan_cannot_replace_or_delete_a_newer_generation() {
    let fixture = fixture("stale-revive-generation", 1).await;
    let paused = pause(&fixture, "stale-revive-workspace").await;
    let mut spec = fixture.adapter.last_spec.lock().await.clone().unwrap();
    spec.chat_host = Some(paused.id.0.clone());
    spec.generation = Some(std::num::NonZeroU64::new(2).unwrap());
    let stale_runtime = fixture.adapter.provision(&spec).await.unwrap();

    let mut observed = paused.clone();
    observed.status = ChatHostStatus::Reviving;
    observed.runtime_generation = 2;
    observed.runtime_id = Some(stale_runtime.0.clone());
    observed.last_transition_claim = Some("chrv_stale_generation".into());
    observed.transition_started_at = Some(PAUSE_AT.into());
    observed.revive_cause = Some(ChatReviveCause::OwnerMessage);
    observed.revive_phase = Some(ChatRevivePhase::RuntimeProvisioning);
    let observed = fixture
        .hosts
        .update_if_revision(paused.revision, &observed)
        .await
        .unwrap()
        .unwrap();

    spec.generation = Some(std::num::NonZeroU64::new(3).unwrap());
    let current_runtime = fixture.adapter.provision(&spec).await.unwrap();
    let mut current = observed.clone();
    current.revision += 1;
    current.runtime_generation = 3;
    current.runtime_id = Some(current_runtime.0.clone());
    current.last_transition_claim = Some("chrv_current_generation".into());
    current.updated_at = crate::clock::now_rfc3339();
    *fixture.hosts.replace_after_candidate_list.lock().await = Some((2, current.clone()));

    fixture.service.reconcile().await;

    let retained = host(&fixture).await;
    assert_eq!(retained.runtime_generation, 3);
    assert_eq!(retained.runtime_id, Some(current_runtime.0.clone()));
    assert_eq!(
        retained.last_transition_claim.as_deref(),
        Some("chrv_current_generation")
    );
    assert!(fixture
        .adapter
        .running
        .lock()
        .await
        .contains_key(&current_runtime.0));
    assert!(!fixture
        .adapter
        .terminated
        .lock()
        .await
        .contains(&current_runtime.0));
}

#[tokio::test]
async fn reconcile_settles_an_expired_active_dispatch_before_later_prompts() {
    let fixture = fixture("active-expired-dispatch", 1).await;
    let target = fixture.threads[0].id.clone();
    let before = host(&fixture).await;
    let mut fenced = before.clone();
    fenced.prompt_admission_sequence = 2;
    let mut ambiguous = ChatPromptAdmission::new(
        target.clone(),
        "possibly delivered before restart".into(),
        1,
        "2000-01-01T00:00:00Z".into(),
    );
    ambiguous.state = ChatPromptDeliveryState::DispatchFenced;
    ambiguous.dispatch_fenced_at = Some("2000-01-01T00:00:01Z".into());
    fenced.prompt_admissions.push(ambiguous);
    fenced.prompt_admissions.push(ChatPromptAdmission::new(
        target.clone(),
        "deliver only after ambiguity settles".into(),
        2,
        "2000-01-01T00:00:02Z".into(),
    ));
    fixture
        .hosts
        .update_if_revision(before.revision, &fenced)
        .await
        .unwrap()
        .unwrap();

    fixture.service.reconcile().await;
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let current = host(&fixture).await;
            if current.prompt_admissions[1].state == ChatPromptDeliveryState::Delivered {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the queued successor is dispatched after ambiguity is recorded");

    let settled = host(&fixture).await;
    assert_eq!(
        settled.prompt_admissions[0].state,
        ChatPromptDeliveryState::RecoveryRequired
    );
    assert_eq!(
        settled.prompt_admissions[0].failure_phase,
        Some(ChatRevivePhase::PromptDelivery)
    );
    assert_eq!(
        settled.prompt_admissions[1].state,
        ChatPromptDeliveryState::Delivered
    );
    let received = fixture.adapter.conversation_inputs.lock().await;
    let inputs = received.get(target.as_str()).unwrap().lock().await;
    assert_eq!(
        inputs
            .iter()
            .filter_map(prompt_text)
            .map(str::to_owned)
            .collect::<Vec<_>>(),
        [default_prefixed("deliver only after ambiguity settles")]
    );
}

#[tokio::test]
async fn reconcile_does_not_steal_a_fresh_active_dispatch_fence() {
    let fixture = fixture("active-fresh-dispatch", 1).await;
    let target = fixture.threads[0].id.clone();
    let before = host(&fixture).await;
    let now = crate::clock::now_rfc3339();
    let mut fenced = before.clone();
    fenced.prompt_admission_sequence = 2;
    let mut in_flight =
        ChatPromptAdmission::new(target.clone(), "in flight".into(), 1, now.clone());
    in_flight.state = ChatPromptDeliveryState::DispatchFenced;
    in_flight.dispatch_fenced_at = Some(now.clone());
    fenced.prompt_admissions.push(in_flight);
    fenced.prompt_admissions.push(ChatPromptAdmission::new(
        target.clone(),
        "must remain queued".into(),
        2,
        now,
    ));
    fixture
        .hosts
        .update_if_revision(before.revision, &fenced)
        .await
        .unwrap()
        .unwrap();

    fixture.service.reconcile().await;

    let unchanged = host(&fixture).await;
    assert_eq!(
        unchanged.prompt_admissions[0].state,
        ChatPromptDeliveryState::DispatchFenced
    );
    assert_eq!(
        unchanged.prompt_admissions[1].state,
        ChatPromptDeliveryState::Queued
    );
    assert!(fixture
        .adapter
        .conversation_inputs
        .lock()
        .await
        .get(target.as_str())
        .unwrap()
        .lock()
        .await
        .is_empty());
}

#[tokio::test]
async fn reviving_host_admits_a_prompt_even_when_the_thread_row_still_looks_ready() {
    let fixture = fixture("reviving-host-ready-thread", 1).await;
    let paused = pause(&fixture, "ready-row-revive-workspace").await;
    let target = fixture.threads[0].id.clone();
    {
        let mut sessions = fixture.sessions.sessions.lock().await;
        let record = sessions
            .get_mut(target.as_str())
            .expect("the retained chat thread exists");
        record.session.status = SessionStatus::Ready;
        record.session.recovery_state = RecoveryState::Normal;
        record.session.stop_reason = None;
    }
    let mut reviving = paused.clone();
    reviving.status = ChatHostStatus::Reviving;
    reviving.runtime_generation = 2;
    reviving.last_transition_claim = Some("chrv_ready_row".into());
    reviving.transition_started_at = Some(PAUSE_AT.into());
    reviving.revive_cause = Some(ChatReviveCause::OwnerMessage);
    reviving.revive_phase = Some(ChatRevivePhase::CheckpointVerification);
    fixture
        .hosts
        .update_if_revision(paused.revision, &reviving)
        .await
        .unwrap()
        .unwrap();

    let turn_started = fixture
        .service
        .send_prompt(
            &fixture.caller,
            &target,
            "admit from durable host state".into(),
        )
        .await
        .expect("durable host state routes the prompt through revival");

    let active = host(&fixture).await;
    assert_eq!(active.status, ChatHostStatus::Active);
    let admission = active.prompt_admissions.last().unwrap();
    assert_eq!(admission.session_id, target);
    assert_eq!(admission.state, ChatPromptDeliveryState::Delivered);
    assert_eq!(admission.turn_started, Some(turn_started));
    let received = fixture.adapter.conversation_inputs.lock().await;
    let inputs = received.get(target.as_str()).unwrap().lock().await;
    assert_eq!(
        inputs
            .iter()
            .filter_map(prompt_text)
            .map(str::to_owned)
            .collect::<Vec<_>>(),
        [default_prefixed("admit from durable host state")]
    );
}

#[tokio::test]
async fn new_chat_creation_revives_a_paused_host_before_allocating_its_worktree() {
    let fixture = fixture("new-chat-revives-host", 1).await;
    pause(&fixture, "new-chat-revive-workspace").await;
    let provisions_before = fixture.adapter.provision_calls.load(Ordering::SeqCst);
    let mut spec = crate::test_support::spec();
    spec.surface = Some(crate::SessionSurface::Chat);

    let created = fixture
        .service
        .create_session(&fixture.caller, spec)
        .await
        .expect("new chat revives the shared host before allocating a thread");

    let active = host(&fixture).await;
    assert_eq!(active.status, ChatHostStatus::Active);
    assert_eq!(active.runtime_generation, 2);
    assert_eq!(
        active.threads.iter().filter(|slot| !slot.removed).count(),
        2
    );
    assert!(active
        .threads
        .iter()
        .any(|slot| slot.session_id == created.id && !slot.removed));
    assert_eq!(
        fixture.adapter.provision_calls.load(Ordering::SeqCst),
        provisions_before + 1
    );
    assert_eq!(created.status, SessionStatus::Ready);
    assert_eq!(created.runtime_version.as_deref(), Some("runtime-chat-v1"));
}

#[tokio::test]
async fn paused_chat_retains_linear_metadata_through_revival_and_delivery() {
    let fixture = fixture("revive-linear-metadata", 1).await;
    pause(&fixture, "linear-metadata-workspace").await;
    let target = fixture.threads[0].id.clone();
    let metadata = BTreeMap::from([("request".into(), "response-create".into())]);

    let turn = fixture
        .service
        .send_linear_prompt(
            &fixture.caller,
            &target,
            "continue linearly".into(),
            None,
            metadata.clone(),
        )
        .await
        .expect("the paused linear prompt revives and reaches the runtime");

    let active = host(&fixture).await;
    let admission = active.prompt_admissions.last().unwrap();
    assert_eq!(admission.state, ChatPromptDeliveryState::Delivered);
    assert_eq!(admission.turn_started, Some(turn));
    let context = admission
        .linear_prompt
        .as_ref()
        .expect("the durable admission retains linear authority");
    assert_eq!(context.expected_previous_turn, None);
    assert_eq!(context.metadata, metadata);

    let events = fixture.sessions.events.lock().await;
    let turn_index = events
        .iter()
        .position(|event| {
            event.session_id == target
                && event.seq == turn
                && matches!(event.kind, StoredEventKind::Known(EventKind::TurnStarted))
        })
        .expect("the delivered turn boundary is durable");
    assert!(matches!(
        &events[turn_index + 2].kind,
        StoredEventKind::Known(EventKind::TurnMetadata { metadata: retained })
            if retained == &context.metadata
    ));
}

#[tokio::test]
async fn paused_chat_returns_the_exact_linear_conflict_without_delivering_a_fork() {
    let fixture = fixture("revive-linear-conflict", 1).await;
    pause(&fixture, "linear-conflict-workspace").await;
    let target = fixture.threads[0].id.clone();

    let error = fixture
        .service
        .send_linear_prompt(
            &fixture.caller,
            &target,
            "must not fork".into(),
            Some(999),
            BTreeMap::new(),
        )
        .await
        .expect_err("the stale predecessor is enforced after revival");
    assert_eq!(
        error,
        CoreError::InvalidState {
            detail: Some(crate::CONVERSATION_ADVANCED_DETAIL.into())
        }
    );

    let active = host(&fixture).await;
    let admission = active.prompt_admissions.last().unwrap();
    assert_eq!(admission.state, ChatPromptDeliveryState::RecoveryRequired);
    assert_eq!(
        admission.failure_phase,
        Some(ChatRevivePhase::PromptDelivery)
    );
    assert!(admission.linear_conflict);
    assert!(!admission.failure_retryable);
    let received = fixture.adapter.conversation_inputs.lock().await;
    let inputs = received.get(target.as_str()).unwrap().lock().await;
    assert!(
        inputs.iter().all(|input| prompt_text(input).is_none()),
        "the stale linear prompt must not reach the restored conversation"
    );
}
