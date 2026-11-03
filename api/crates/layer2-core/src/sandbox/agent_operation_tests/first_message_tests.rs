use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use layer3_adapter::{AdapterError, ConversationInput, ConversationInputSender};

use crate::sandbox::store::{AgentLogStore, AgentStore};
use crate::trigger_domain::{
    Launch, LaunchAuthorityClass, LaunchConfirmation, LaunchConsentSurface, LaunchResolution,
    LaunchSeed, LaunchState, LaunchTrustManifest, PublicTriggerId, RequirementsSnapshot,
    ReviewRequirements, TriggerId,
};

use super::super::*;
use super::fixtures::*;

#[test]
fn first_message_recovery_requires_the_exact_digest_and_acknowledgement() {
    let mut record = FirstMessageRecord::new("origin", "hello", NOW).unwrap();
    record
        .fence_dispatch_for(ACTIVE_TURN, 1, 9, LATER.into())
        .unwrap();
    record
        .require_recovery(
            FirstMessageFailure::new("ambiguous", "receipt lost", false, LATER).unwrap(),
        )
        .unwrap();
    let digest = record.payload_sha256.clone();

    assert!(record
        .authorize_recovery(&"0".repeat(64), true, LATEST.into())
        .is_err());
    assert!(record
        .authorize_recovery(&digest, false, LATEST.into())
        .is_err());
    record
        .authorize_recovery(&digest, true, LATEST.into())
        .unwrap();

    assert_eq!(record.state, FirstMessageDelivery::Queued);
    assert_eq!(record.recovery_generation, 1);
    assert_eq!(record.dispatch_generation, 0);
    assert_eq!(record.turn_sequence, None);
    assert!(record.failure.is_none());
}

#[test]
fn first_message_tracks_dispatch_generation_and_late_evidence() {
    let mut record = FirstMessageRecord::new("origin", "hello", NOW).unwrap();
    assert!(record
        .fence_dispatch_for(ACTIVE_TURN, 1, 0, LATER.into())
        .is_err());
    record
        .fence_dispatch_for(ACTIVE_TURN, 1, 11, LATER.into())
        .unwrap();
    assert_eq!(record.dispatch_generation, 11);
    record
        .require_recovery(
            FirstMessageFailure::new("ambiguous", "receipt lost", false, LATER).unwrap(),
        )
        .unwrap();
    record.mark_execution_observed(LATEST.into()).unwrap();
    assert_eq!(record.state, FirstMessageDelivery::ExecutionObserved);
    assert!(record.failure.is_none());
    record.complete(LATEST.into()).unwrap();
    assert_eq!(record.state, FirstMessageDelivery::Completed);

    let mut retry = FirstMessageRecord::new("origin", "hello", NOW).unwrap();
    retry
        .fence_dispatch_for(ACTIVE_TURN, 1, 12, LATER.into())
        .unwrap();
    retry.retry_after_pre_write_failure().unwrap();
    assert_eq!(retry.state, FirstMessageDelivery::Queued);
    assert_eq!(retry.dispatch_generation, 0);
    assert_eq!(retry.turn_sequence, Some(ACTIVE_TURN));
}

#[tokio::test]
async fn dispatch_without_a_configured_first_message_is_a_noop() {
    let store = Arc::new(InMemorySandboxStore::default());
    let mut agent = test_agent(AgentStatus::Ready, None);
    authorize_first_message_work(&mut agent, 5);
    store.create_agent(agent.clone()).await.unwrap();
    let (sender, inputs) = recording_sender(Ok(()));
    let registry = TestRuntimeRegistry::new(full_capabilities(), sender);
    let service = AgentOperationService::new(store.clone(), registry.clone());

    assert_eq!(
        service
            .dispatch_queued_first_message(&agent.id, work_authority(5))
            .await
            .unwrap(),
        FirstMessageDispatchOutcome::NotConfigured
    );
    assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(agent));
    assert_eq!(registry.resolutions(), 0);
    assert!(inputs.lock().unwrap().is_empty());
}

#[tokio::test]
async fn first_message_fence_is_durable_before_the_runtime_side_effect() {
    let store = Arc::new(InMemorySandboxStore::default());
    seed_active_parent(&store).await;
    let mut agent = test_agent(AgentStatus::Ready, Some("Start here."));
    authorize_first_message_work(&mut agent, 5);
    store.create_agent(agent.clone()).await.unwrap();
    let fence_seen = Arc::new(AtomicBool::new(false));
    let inputs = Arc::new(Mutex::new(Vec::new()));
    let sender = first_message_fence_checking_sender(
        store.clone(),
        agent.id.clone(),
        fence_seen.clone(),
        inputs.clone(),
        5,
    );
    let registry = TestRuntimeRegistry::new(full_capabilities(), sender);
    let service = AgentOperationService::new(store.clone(), registry.clone());

    let outcome = service
        .dispatch_queued_first_message(&agent.id, work_authority(5))
        .await
        .unwrap();
    let FirstMessageDispatchOutcome::Accepted(record) = outcome else {
        panic!("first message must be accepted")
    };

    assert!(fence_seen.load(Ordering::SeqCst));
    assert_eq!(record.state, FirstMessageDelivery::WriteConfirmed);
    assert_eq!(record.dispatch_generation, 5);
    let stored = store.get_agent(&agent.id).await.unwrap().unwrap();
    assert_eq!(stored.status, AgentStatus::Working);
    assert_eq!(stored.conversation.active_turn, record.turn_sequence);
    assert!(matches!(
        inputs.lock().unwrap().as_slice(),
        [ConversationInput::RecoverablePrompt { content, .. }] if prompt_text(content) == "Start here."
    ));
    assert_eq!(registry.resolutions(), 2);
    let logs = store.agent_log(&agent.id, 0, 20).await.unwrap();
    let logs: Vec<_> = logs
        .into_iter()
        .filter(|entry| !matches!(entry.kind, AgentLogKind::RuntimeMetadataChanged { .. }))
        .collect();
    assert!(matches!(
        logs.as_slice(),
        [
            AgentLogEntry {
                kind: AgentLogKind::FirstMessageChanged {
                    state: FirstMessageDelivery::DispatchFenced,
                    ..
                },
                ..
            },
            AgentLogEntry {
                kind: AgentLogKind::TurnStarted {
                    origin_key: Some(_),
                    ..
                },
                ..
            },
            AgentLogEntry {
                kind: AgentLogKind::OwnerMessage { .. },
                ..
            },
            AgentLogEntry {
                kind: AgentLogKind::StatusChanged {
                    from: AgentStatus::Ready,
                    to: AgentStatus::Working
                },
                ..
            },
            AgentLogEntry {
                kind: AgentLogKind::FirstMessageChanged {
                    state: FirstMessageDelivery::WriteConfirmed,
                    ..
                },
                ..
            },
        ]
    ));
}

#[tokio::test]
async fn proven_pre_write_failure_requeues_the_first_message() {
    let store = Arc::new(InMemorySandboxStore::default());
    seed_active_parent(&store).await;
    let mut agent = test_agent(AgentStatus::Ready, Some("Start here."));
    authorize_first_message_work(&mut agent, 5);
    store.create_agent(agent.clone()).await.unwrap();
    let (sender, inputs) = recording_sender(Ok(()));
    let registry = TestRuntimeRegistry::new(full_capabilities(), sender).fail_on_resolution(2);
    let service = AgentOperationService::new(store.clone(), registry);

    let outcome = service
        .dispatch_queued_first_message(&agent.id, work_authority(5))
        .await
        .unwrap();
    let FirstMessageDispatchOutcome::Retryable(record) = outcome else {
        panic!("pre-write failure must be retryable")
    };
    assert_eq!(record.state, FirstMessageDelivery::Queued);
    assert_eq!(record.attempt, 1);
    assert_eq!(record.dispatch_generation, 0);
    assert!(inputs.lock().unwrap().is_empty());
    let stored = store.get_agent(&agent.id).await.unwrap().unwrap();
    assert_eq!(stored.status, AgentStatus::Ready);
    assert_eq!(stored.conversation.active_turn, None);
}

#[tokio::test]
async fn ambiguous_first_message_never_resends_automatically() {
    let store = Arc::new(InMemorySandboxStore::default());
    seed_active_parent(&store).await;
    let mut agent = test_agent(AgentStatus::Ready, Some("Start here."));
    authorize_first_message_work(&mut agent, 5);
    store.create_agent(agent.clone()).await.unwrap();
    let (sender, inputs) = recording_sender(Err(AdapterError::Rejected("receipt lost".into())));
    let registry = TestRuntimeRegistry::new(full_capabilities(), sender);
    let service = AgentOperationService::new(store.clone(), registry.clone());

    let first = service
        .dispatch_queued_first_message(&agent.id, work_authority(5))
        .await
        .unwrap();
    let FirstMessageDispatchOutcome::RecoveryRequired(record) = first else {
        panic!("ambiguity must require recovery")
    };
    assert_eq!(
        record.failure.as_ref().unwrap().code,
        "first_message_write_ambiguous"
    );
    assert_eq!(inputs.lock().unwrap().len(), 1);

    assert!(matches!(
        service
            .dispatch_queued_first_message(&agent.id, work_authority(5))
            .await
            .unwrap(),
        FirstMessageDispatchOutcome::RecoveryRequired(_)
    ));
    assert_eq!(registry.resolutions(), 2);
    assert_eq!(inputs.lock().unwrap().len(), 1);

    let recorder = DurableAgentUpdateRecorder::new(store.clone());
    let mut assembly = AgentMessageAssembly::default();
    recorder
        .record_update(
            &agent.id,
            3,
            &mut assembly,
            layer3_adapter::ConversationUpdate::Said(
                layer3_adapter::SessionUpdate::AvailableCommandsUpdate(
                    layer3_adapter::AvailableCommandsUpdate::new(vec![
                        layer3_adapter::AvailableCommand::new("review", "Review the worktree"),
                    ]),
                ),
            ),
            LATEST.into(),
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .get_agent(&agent.id)
            .await
            .unwrap()
            .unwrap()
            .conversation
            .first_message
            .unwrap()
            .state,
        FirstMessageDelivery::RecoveryRequired
    );
    recorder
        .record_update(
            &agent.id,
            3,
            &mut assembly,
            layer3_adapter::ConversationUpdate::TurnEnded(layer3_adapter::PromptResponse::new(
                layer3_adapter::TurnStopReason::EndTurn,
            )),
            LATEST.into(),
        )
        .await
        .unwrap();
    let resolved = store.get_agent(&agent.id).await.unwrap().unwrap();
    assert_eq!(
        resolved.conversation.first_message.unwrap().state,
        FirstMessageDelivery::Completed
    );
}

#[tokio::test]
async fn same_lease_observes_in_progress_but_a_new_lease_fences_abandonment() {
    let same_store = Arc::new(InMemorySandboxStore::default());
    let mut same = fenced_first_message_agent(5);
    authorize_first_message_work(&mut same, 5);
    same_store.create_agent(same.clone()).await.unwrap();
    let (sender, inputs) = recording_sender(Ok(()));
    let registry = TestRuntimeRegistry::new(full_capabilities(), sender);
    let service = AgentOperationService::new(same_store, registry.clone());
    assert!(matches!(
        service
            .dispatch_queued_first_message(&same.id, work_authority(5))
            .await
            .unwrap(),
        FirstMessageDispatchOutcome::InProgress(_)
    ));
    assert_eq!(registry.resolutions(), 0);
    assert!(inputs.lock().unwrap().is_empty());

    let new_store = Arc::new(InMemorySandboxStore::default());
    let mut abandoned = fenced_first_message_agent(4);
    authorize_first_message_work(&mut abandoned, 5);
    new_store.create_agent(abandoned.clone()).await.unwrap();
    seed_log_to(&new_store, &abandoned.id, abandoned.conversation.last_seq).await;
    let (sender, inputs) = recording_sender(Ok(()));
    let registry = TestRuntimeRegistry::new(full_capabilities(), sender);
    let service = AgentOperationService::new(new_store.clone(), registry.clone());
    let outcome = service
        .dispatch_queued_first_message(&abandoned.id, work_authority(5))
        .await
        .unwrap();
    let FirstMessageDispatchOutcome::RecoveryRequired(record) = outcome else {
        panic!("abandoned fence must require recovery")
    };
    assert_eq!(
        record.failure.as_ref().unwrap().code,
        "first_message_dispatch_abandoned"
    );
    assert_eq!(registry.resolutions(), 0);
    assert!(inputs.lock().unwrap().is_empty());
}

#[tokio::test]
async fn stale_work_authority_cannot_touch_state_or_runtime() {
    let store = Arc::new(InMemorySandboxStore::default());
    let mut agent = test_agent(AgentStatus::Ready, Some("Start here."));
    authorize_first_message_work(&mut agent, 5);
    store.create_agent(agent.clone()).await.unwrap();
    let (sender, inputs) = recording_sender(Ok(()));
    let registry = TestRuntimeRegistry::new(full_capabilities(), sender);
    let service = AgentOperationService::new(store.clone(), registry.clone());

    assert_eq!(
        service
            .dispatch_queued_first_message(&agent.id, work_authority(4))
            .await,
        Err(AgentOperationError::StaleWorkAuthority)
    );
    assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(agent));
    assert_eq!(registry.resolutions(), 0);
    assert!(inputs.lock().unwrap().is_empty());
}

#[tokio::test]
async fn explicit_recovery_is_owner_authorized_and_closes_the_uncertain_turn() {
    let store = Arc::new(InMemorySandboxStore::default());
    seed_active_parent(&store).await;
    let mut agent = test_agent(AgentStatus::Ready, Some("Start here."));
    authorize_first_message_work(&mut agent, 5);
    store.create_agent(agent.clone()).await.unwrap();
    let (sender, _) = recording_sender(Err(AdapterError::Rejected("receipt lost".into())));
    let service = AgentOperationService::new(
        store.clone(),
        TestRuntimeRegistry::new(full_capabilities(), sender),
    );
    let outcome = service
        .dispatch_queued_first_message(&agent.id, work_authority(5))
        .await
        .unwrap();
    let FirstMessageDispatchOutcome::RecoveryRequired(record) = outcome else {
        panic!("ambiguity must require recovery")
    };
    let digest = record.payload_sha256.clone();

    assert_eq!(
        service
            .authorize_first_message_recovery(&unauthorized_owner(), &agent.id, &digest, true)
            .await,
        Err(AgentOperationError::Forbidden)
    );
    assert!(matches!(
        service
            .authorize_first_message_recovery(&owner(), &agent.id, &digest, false)
            .await,
        Err(AgentOperationError::InvalidRequest(_))
    ));
    assert!(matches!(
        service
            .authorize_first_message_recovery(&owner(), &agent.id, &"0".repeat(64), true)
            .await,
        Err(AgentOperationError::InvalidRequest(_))
    ));
    let recovered = service
        .authorize_first_message_recovery(&owner(), &agent.id, &digest, true)
        .await
        .unwrap();
    assert_eq!(recovered.state, FirstMessageDelivery::Queued);
    assert_eq!(recovered.recovery_generation, 1);

    let stored = store.get_agent(&agent.id).await.unwrap().unwrap();
    assert_eq!(stored.status, AgentStatus::Ready);
    assert_eq!(stored.conversation.active_turn, None);
    let logs = store.agent_log(&agent.id, 0, 30).await.unwrap();
    assert!(logs
        .iter()
        .any(|entry| matches!(entry.kind, AgentLogKind::TurnInterrupted { .. })));
}

#[tokio::test]
async fn late_terminal_evidence_resolves_first_message_ambiguity() {
    let store = Arc::new(InMemorySandboxStore::default());
    seed_active_parent(&store).await;
    let mut agent = test_agent(AgentStatus::Ready, Some("Start here."));
    authorize_first_message_work(&mut agent, 5);
    store.create_agent(agent.clone()).await.unwrap();
    let sender = completing_ambiguous_sender(store.clone(), agent.id.clone());
    let service = AgentOperationService::new(
        store.clone(),
        TestRuntimeRegistry::new(full_capabilities(), sender),
    );

    let outcome = service
        .dispatch_queued_first_message(&agent.id, work_authority(5))
        .await
        .unwrap();
    let FirstMessageDispatchOutcome::Accepted(record) = outcome else {
        panic!("execution evidence must win")
    };
    assert_eq!(record.state, FirstMessageDelivery::Completed);
    assert!(record.failure.is_none());
    assert_eq!(
        store.get_agent(&agent.id).await.unwrap().unwrap().status,
        AgentStatus::Ready
    );
}

#[tokio::test]
async fn trigger_launch_observes_the_real_first_message_dispatch_without_resending() {
    let store = Arc::new(InMemorySandboxStore::default());
    seed_active_parent(&store).await;
    let mut agent = test_agent(AgentStatus::Ready, Some("Inspect the workspace"));
    authorize_first_message_work(&mut agent, 5);
    store.create_agent(agent.clone()).await.unwrap();
    let (sender, inputs) = recording_sender(Ok(()));
    let service = AgentOperationService::new(
        store.clone(),
        TestRuntimeRegistry::new(full_capabilities(), sender),
    );
    let mut launch = Launch::new(LaunchSeed {
        trigger_id: TriggerId("trg_first_message_integration".into()),
        revision_digest: "a".repeat(64),
        public_id: PublicTriggerId("pub_first_message_integration".into()),
        owner: agent.owner.clone(),
        authority: LaunchAuthorityClass::Visitor,
        placement_id: None,
        referrer_origin: None,
        requirements: RequirementsSnapshot {
            services: Vec::new(),
            checked_at: NOW.into(),
        },
        review: ReviewRequirements {
            needs: Vec::new(),
            can_start_now: true,
        },
        trust: LaunchTrustManifest::none(),
        resolution: LaunchResolution::pending(),
        has_first_message: true,
        command_key_hash: "b".repeat(64),
        command_fingerprint: "c".repeat(64),
        created_at: NOW.into(),
    })
    .unwrap();
    launch
        .start(LaunchConfirmation {
            surface: LaunchConsentSurface::Landing,
            trust_confirmed_digest: None,
            acknowledgements: Vec::new(),
            confirmed_at: NOW.into(),
        })
        .unwrap();

    let fence = launch
        .work
        .claim("launch-worker".into(), NOW, LATER.into())
        .unwrap();
    launch
        .record_worker_sandbox(
            "launch-worker",
            fence,
            agent.sandbox_id.clone(),
            true,
            NOW.into(),
        )
        .unwrap();
    let fence = launch
        .work
        .claim("launch-worker".into(), NOW, LATER.into())
        .unwrap();
    launch
        .record_worker_sandbox_ready(
            "launch-worker",
            fence,
            Some(agent.workspace_id.clone()),
            NOW.into(),
        )
        .unwrap();
    let fence = launch
        .work
        .claim("launch-worker".into(), NOW, LATER.into())
        .unwrap();
    launch
        .record_worker_workspace(
            "launch-worker",
            fence,
            agent.workspace_id.clone(),
            true,
            NOW.into(),
        )
        .unwrap();
    let fence = launch
        .work
        .claim("launch-worker".into(), NOW, LATER.into())
        .unwrap();
    launch
        .record_worker_workspace_ready("launch-worker", fence, NOW.into())
        .unwrap();
    let fence = launch
        .work
        .claim("launch-worker".into(), NOW, LATER.into())
        .unwrap();
    launch
        .record_worker_agent("launch-worker", fence, agent.id.clone(), true, NOW.into())
        .unwrap();
    let queued = agent.conversation.first_message.as_ref().unwrap();
    let fence = launch
        .work
        .claim("launch-worker".into(), NOW, LATER.into())
        .unwrap();
    launch
        .record_worker_agent_ready("launch-worker", fence, &agent.id, Some(queued), NOW.into())
        .unwrap();

    let outcome = service
        .dispatch_queued_first_message(&agent.id, work_authority(5))
        .await
        .unwrap();
    let FirstMessageDispatchOutcome::Accepted(record) = outcome else {
        panic!("real Agent operation service must accept the first message")
    };
    let fence = launch
        .work
        .claim(
            "launch-observer".into(),
            LATEST,
            "2026-09-05T00:00:03Z".into(),
        )
        .unwrap();
    launch
        .record_worker_first_message(
            "launch-observer",
            fence,
            &agent.id,
            &record,
            LATEST.into(),
            LATEST.into(),
        )
        .unwrap();
    let fence = launch
        .work
        .claim(
            "launch-finisher".into(),
            LATEST,
            "2026-09-05T00:00:03Z".into(),
        )
        .unwrap();
    launch
        .finish_worker_launch("launch-finisher", fence, LATEST.into())
        .unwrap();

    assert_eq!(launch.state, LaunchState::Ready);
    assert_eq!(launch.first_message, FirstMessageDelivery::WriteConfirmed);
    assert_eq!(
        launch.first_message_evidence.as_ref().unwrap().agent_id,
        agent.id
    );
    assert_eq!(inputs.lock().unwrap().len(), 1);
}

fn fenced_first_message_agent(dispatch_generation: u64) -> Agent {
    let mut agent = test_agent(AgentStatus::Ready, Some("Start here."));
    let record = agent.conversation.first_message.as_mut().unwrap();
    record
        .fence_dispatch_for(ACTIVE_TURN, 1, dispatch_generation, LATER.into())
        .unwrap();
    agent.status = AgentStatus::Working;
    agent.conversation.active_turn = Some(ACTIVE_TURN);
    agent.conversation.last_seq = 1;
    agent
}

async fn seed_log_to(store: &InMemorySandboxStore, id: &crate::AgentId, sequence: u64) {
    for _ in 0..sequence {
        store
            .append_agent_log(
                id,
                NOW.into(),
                AgentLogKind::OwnerMessage {
                    content: "seed".into(),
                },
            )
            .await
            .unwrap();
    }
}

fn first_message_fence_checking_sender(
    store: Arc<InMemorySandboxStore>,
    id: crate::AgentId,
    fence_seen: Arc<AtomicBool>,
    inputs: Arc<Mutex<Vec<ConversationInput>>>,
    dispatch_generation: u64,
) -> ConversationInputSender {
    ConversationInputSender::spawn(2, move |input| {
        let store = store.clone();
        let id = id.clone();
        let fence_seen = fence_seen.clone();
        let inputs = inputs.clone();
        async move {
            inputs.lock().unwrap().push(input);
            let agent = store
                .get_agent(&id)
                .await
                .map_err(|error| AdapterError::Rejected(error.to_string()))?
                .ok_or_else(|| AdapterError::Rejected("agent absent".into()))?;
            let fenced = agent.status == AgentStatus::Working
                && agent
                    .conversation
                    .first_message
                    .as_ref()
                    .is_some_and(|record| {
                        record.state == FirstMessageDelivery::DispatchFenced
                            && record.dispatch_generation == dispatch_generation
                    });
            fence_seen.store(fenced, Ordering::SeqCst);
            fenced
                .then_some(())
                .ok_or_else(|| AdapterError::Rejected("durable first-message fence absent".into()))
        }
    })
}

fn completing_ambiguous_sender(
    store: Arc<InMemorySandboxStore>,
    id: crate::AgentId,
) -> ConversationInputSender {
    let recorder = DurableAgentUpdateRecorder::new(store);
    ConversationInputSender::spawn(2, move |_| {
        let recorder = recorder.clone();
        let id = id.clone();
        async move {
            let mut assembly = AgentMessageAssembly::default();
            recorder
                .record_update(
                    &id,
                    3,
                    &mut assembly,
                    layer3_adapter::ConversationUpdate::TurnEnded(
                        layer3_adapter::PromptResponse::new(
                            layer3_adapter::TurnStopReason::EndTurn,
                        ),
                    ),
                    LATEST.into(),
                )
                .await
                .map_err(|error| AdapterError::Rejected(error.to_string()))?;
            Err(AdapterError::Rejected("write receipt lost".into()))
        }
    })
}
