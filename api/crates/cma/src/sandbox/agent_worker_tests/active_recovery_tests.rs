//! Exercise real AgentWorker recovery through the production Bridge transport.
use super::*;
use layer2_core::sandbox::store::{
    AgentConnectionRecoveryCandidate, AgentConnectionRecoveryOutcome,
};
use layer2_core::sandbox::{AgentMessageAssembly, AgentUpdateRecorder};

mod recorder;
mod transport;
use transport::Policy::{self, Coordinated, Legacy, Shared};

async fn fixture(
    chunks: u64,
    bounded: bool,
    policy: Policy,
    hold_handshake: bool,
) -> (
    Harness,
    Agent,
    Arc<transport::Backend>,
    Arc<transport::Journal>,
    tokio::task::JoinHandle<()>,
) {
    fixture_with_settings(chunks, bounded, policy, hold_handshake, false).await
}

async fn fixture_with_settings(
    chunks: u64,
    bounded: bool,
    policy: Policy,
    hold_handshake: bool,
    configured: bool,
) -> (
    Harness,
    Agent,
    Arc<transport::Backend>,
    Arc<transport::Journal>,
    tokio::task::JoinHandle<()>,
) {
    let mut h = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    h.now = SystemTime::now();
    let admitted = h
        .admit_with_first_message(
            "active-recovery",
            vec![],
            (!configured).then_some("original input"),
        )
        .await;
    for _ in 0..if configured { 5 } else { 6 } {
        h.run(&admitted.id, h.now).await;
    }
    if configured {
        let caller = layer2_core::Caller::new(
            admitted.owner.clone(),
            layer2_core::Authorization::new([layer2_core::Capability::UseOwnSandboxes]),
        );
        let operations = AgentOperationService::new(h.store.clone(), h.registry.clone());
        operations
            .update_settings(
                &caller,
                &admitted.id,
                layer2_core::sandbox::AgentSettings {
                    model: "gpt-6".into(),
                    reasoning_effort: ThinkingEffort::XHigh,
                    permission_mode: PermissionLevel::Guarded,
                    system_prompt: None,
                },
            )
            .await
            .unwrap();
        operations
            .send_message(&caller, &admitted.id, "original input".into())
            .await
            .unwrap();
    }
    let agent = h.stored(&admitted.id).await;
    assert_eq!(agent.status, AgentStatus::Working);
    assert!(agent.conversation.active_turn.is_some());
    h.registry.fence(
        &agent.id,
        agent.conversation.runtime_generation,
        agent.work.next_fence - 1,
    );
    h.updates
        .record_update(
            &agent.id,
            1,
            &mut AgentMessageAssembly::default(),
            ConversationUpdate::Said(layer3_adapter::SessionUpdate::AgentMessageChunk(
                layer3_adapter::ContentChunk::new("saved prefix".into()),
            )),
            rfc3339_at(h.now),
        )
        .await
        .unwrap();
    let agent = h.stored(&agent.id).await;
    assert_eq!(
        h.store
            .schedule_agent_connection_recovery(
                AgentConnectionRecoveryCandidate::from_agent(&agent)
                    .unwrap()
                    .restart_request(rfc3339_at(h.now))
            )
            .await
            .unwrap(),
        AgentConnectionRecoveryOutcome::Scheduled
    );
    let (backend, journal, server) =
        transport::start(&h, agent.clone(), chunks, bounded, policy, hold_handshake).await;
    install_worker(&mut h, backend.clone());
    (h, agent, backend, journal, server)
}

fn install_worker(h: &mut Harness, backend: Arc<transport::Backend>) {
    let adapter = Arc::new(CodexAdapter::new(
        backend.clone(),
        CodexSandboxSpec::default(),
    ));
    let mut config = AgentWorkerConfig::new("agent-worker", DEPLOYMENT_ID);
    config.open_timeout = Duration::from_secs(2);
    config.lease_duration = Duration::from_secs(30);
    config.retry_delay = Duration::from_secs(1);
    h.worker = AgentWorker::new(
        AgentWorkerDependencies {
            store: h.store.clone(),
            adapter,
            skills: Arc::new(EmptyAgentSkillResolver),
            registry: h.registry.clone(),
            updates: h.updates.clone(),
            operations: AgentOperationService::new(h.store.clone(), h.registry.clone()),
        },
        config,
    );
}

#[tokio::test]
async fn active_recovery_drains_retained_backlog_with_the_worker_consumer() {
    for (chunks, bounded, policy) in [
        (600, true, Legacy),
        (600, true, Shared),
        (600, true, Coordinated),
        (600, false, Legacy),
        (64, true, Legacy),
    ] {
        let (h, original, backend, journal, server) = fixture(chunks, bounded, policy, false).await;
        h.run(&original.id, h.now).await;
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if journal.acked.lock().unwrap().last() == Some(&(chunks + 3)) {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the production worker records and ACKs the original backlog");
        let finished = h.stored(&original.id).await;
        assert_eq!(finished.status, AgentStatus::Ready);
        assert_eq!(finished.work.step, AgentWorkStep::None);
        assert_eq!(finished.conversation.active_turn, None);
        assert_eq!(
            finished.conversation.conversation_id,
            original.conversation.conversation_id
        );
        assert_eq!(
            finished.conversation.runtime_generation,
            original.conversation.runtime_generation
        );
        let first = finished.conversation.first_message.as_ref().unwrap();
        let original_first = original.conversation.first_message.as_ref().unwrap();
        assert_eq!(first.text, original_first.text);
        assert_eq!(first.attempt, original_first.attempt);
        assert_eq!(first.turn_sequence, original_first.turn_sequence);
        assert_eq!(first.state, FirstMessageDelivery::Completed);
        assert_eq!(
            *journal.acked.lock().unwrap(),
            (1..=chunks + 3).collect::<Vec<_>>()
        );
        let logs = h.store.agent_log(&original.id, 0, 200).await.unwrap();
        assert_eq!(
            logs.iter()
                .filter_map(|entry| match entry.kind {
                    AgentLogKind::TurnCompleted { turn_sequence } => Some(turn_sequence),
                    _ => None,
                })
                .collect::<Vec<_>>(),
            [original.conversation.active_turn.unwrap()]
        );
        assert_eq!(
            logs.iter()
                .filter_map(|entry| match &entry.kind {
                    AgentLogKind::AgentMessage { content } => Some(content.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>(),
            [format!("saved prefix{}", "x".repeat(chunks as usize))]
        );
        use layer3_adapter::hosted::HostedControlTransport as _;
        backend.transport.retire(&h.runtime).await.unwrap();
        server.abort();
    }
}

async fn acknowledged(journal: &transport::Journal) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if journal.acked.lock().unwrap().last() == Some(&(journal.chunks + 3)) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("original retained stream durably recorded");
}

#[tokio::test]
async fn active_recovery_keeps_pending_record_through_handoff_and_commit_retry() {
    for (chunks, cursor, fault) in [(64, 1, 1), (600, 1, 2), (600, 601, 3)] {
        let (mut h, original, backend, journal, server) =
            fixture(chunks, true, Legacy, false).await;
        let gate = recorder::install(&mut h, backend.clone(), cursor, fault);
        let worker = h.worker.clone();
        let id = original.id.clone();
        let now = h.now;
        let run = tokio::spawn(async move { worker.run_one(&id, now).await });
        tokio::time::timeout(Duration::from_secs(5), gate.entered.notified())
            .await
            .unwrap();
        assert!(journal
            .acked
            .lock()
            .unwrap()
            .iter()
            .all(|acked| *acked < cursor));
        if chunks == 64 {
            tokio::time::timeout(Duration::from_secs(1), async {
                while !run.is_finished() {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("publication transfers the same held recorder without waiting for its write");
        }
        gate.release.notify_one();
        assert!(run.await.unwrap().unwrap());
        acknowledged(&journal).await;
        let finished = h.stored(&original.id).await;
        assert_eq!(finished.status, AgentStatus::Ready);
        let logs = h.store.agent_log(&original.id, 0, 200).await.unwrap();
        assert_eq!(
            logs.iter()
                .filter(|entry| matches!(entry.kind, AgentLogKind::TurnCompleted { .. }))
                .count(),
            1
        );
        assert_eq!(
            *journal.acked.lock().unwrap(),
            (1..=chunks + 3).collect::<Vec<_>>()
        );
        use layer3_adapter::hosted::HostedControlTransport as _;
        backend.transport.retire(&h.runtime).await.unwrap();
        server.abort();
    }
}

#[tokio::test]
async fn active_recovery_open_timeout_retries_original_unacknowledged_turn() {
    let (h, original, backend, journal, server) = fixture(600, true, Legacy, true).await;
    h.run(&original.id, h.now).await;
    let failed = h.stored(&original.id).await;
    assert_eq!(failed.work.step, AgentWorkStep::PublishConnection);
    assert!(failed.work.lease.is_none());
    assert_eq!(
        failed.conversation.active_turn,
        original.conversation.active_turn
    );
    assert!(journal.acked.lock().unwrap().is_empty());
    tokio::time::timeout(Duration::from_secs(3), journal.closed.notified())
        .await
        .unwrap();
    let retry_at = SystemTime::UNIX_EPOCH
        + Duration::from_secs(layer2_core::seconds_from_rfc3339(&failed.work.ready_at).unwrap());
    h.run(&original.id, retry_at).await;
    acknowledged(&journal).await;
    assert_eq!(h.stored(&original.id).await.status, AgentStatus::Ready);
    use layer3_adapter::hosted::HostedControlTransport as _;
    backend.transport.retire(&h.runtime).await.unwrap();
    server.abort();
}

#[tokio::test]
async fn active_recovery_cancellation_and_fence_leave_output_for_the_successor() {
    for cancel in [true, false] {
        let (mut h, original, backend, journal, server) = fixture(600, true, Legacy, false).await;
        let gate = recorder::install(&mut h, backend.clone(), 1, 1);
        let worker = h.worker.clone();
        let id = original.id.clone();
        let now = h.now;
        let run = tokio::spawn(async move { worker.run_one(&id, now).await });
        tokio::time::timeout(Duration::from_secs(5), gate.entered.notified())
            .await
            .unwrap();
        assert!(journal.acked.lock().unwrap().is_empty());
        assert!(!h
            .registry
            .published_target_is_attached(&Harness::target(&original)));
        if cancel {
            run.abort();
            let _ = run.await;
        } else {
            let claimed = h.stored(&original.id).await;
            h.registry.fence(
                &original.id,
                original.conversation.runtime_generation,
                claimed.work.lease.unwrap().fence,
            );
            assert!(run.await.unwrap().unwrap());
        }
        tokio::time::timeout(Duration::from_secs(3), journal.closed.notified())
            .await
            .unwrap();
        assert!(journal.acked.lock().unwrap().is_empty());
        gate.release.notify_one();
        h.run(&original.id, h.now + Duration::from_secs(180)).await;
        acknowledged(&journal).await;
        assert_eq!(h.stored(&original.id).await.status, AgentStatus::Ready);
        use layer3_adapter::hosted::HostedControlTransport as _;
        backend.transport.retire(&h.runtime).await.unwrap();
        server.abort();
    }
}

#[tokio::test]
async fn active_recovery_consumes_backlog_while_replaying_persisted_settings() {
    let (h, original, backend, journal, server) =
        fixture_with_settings(600, true, Coordinated, false, true).await;
    h.run(&original.id, h.now).await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while journal.acked.lock().unwrap().last() != Some(&606) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("settings replies and retained history must both progress before publication");
    assert_eq!(
        *journal.requests.lock().unwrap(),
        [
            "session/set_mode",
            "session/set_model",
            "session/set_config_option",
            "session/set_mode"
        ]
    );
    assert_eq!(h.stored(&original.id).await.work.step, AgentWorkStep::None);
    use layer3_adapter::hosted::HostedControlTransport as _;
    backend.transport.retire(&h.runtime).await.unwrap();
    server.abort();
}

#[tokio::test]
async fn recovery_retries_a_late_failure_without_undoing_the_recorded_terminal() {
    let (h, original, backend, journal, server) =
        fixture_with_settings(600, true, Legacy, false, true).await;
    journal.fail_settings.store(true, Ordering::SeqCst);
    h.run(&original.id, h.now).await;
    let retry = h.stored(&original.id).await;
    assert_eq!(retry.status, AgentStatus::Ready);
    assert_eq!(retry.conversation.active_turn, None);
    assert_eq!(retry.work.step, AgentWorkStep::PublishConnection);
    assert!(
        retry.work.lease.is_none(),
        "late failure releases only the original claim"
    );
    assert!(journal.acked.lock().unwrap().contains(&601));
    let logs = h.store.agent_log(&original.id, 0, 200).await.unwrap();
    assert_eq!(
        logs.iter()
            .filter(|entry| matches!(entry.kind, AgentLogKind::TurnCompleted { .. }))
            .count(),
        1
    );
    use layer3_adapter::hosted::HostedControlTransport as _;
    backend.transport.retire(&h.runtime).await.unwrap();
    server.abort();
}

#[tokio::test]
async fn recovery_retry_accepts_only_output_progress_of_the_original_claim() {
    use crate::sandbox::agent_worker::same_recovery_claim;
    for legacy_ready_at in [false, true] {
        let (h, original, _, _, server) = fixture(64, true, Legacy, false).await;
        let mut claimed = h
            .store
            .claim_agent_work(
                &original.id,
                "agent-worker",
                rfc3339_at(h.now),
                rfc3339_at(h.now + Duration::from_secs(60)),
            )
            .await
            .unwrap()
            .unwrap();
        h.updates
            .record_update(
                &original.id,
                1,
                &mut AgentMessageAssembly::default(),
                ConversationUpdate::TurnEndedForPrompt {
                    prompt_id: layer3_adapter::PromptIdentity::new(
                        original.conversation.conversation_id.as_ref().unwrap(),
                        1,
                        original.conversation.active_turn.unwrap(),
                    ),
                    response: layer3_adapter::PromptResponse::new(
                        layer3_adapter::TurnStopReason::EndTurn,
                    ),
                },
                rfc3339_at(h.now),
            )
            .await
            .unwrap();
        let settled = h.stored(&original.id).await;
        if legacy_ready_at {
            // Exercise the comparison separately: the memory store refuses
            // rewriting ready_at on a persisted conversation mutation.
            claimed.ready_at = None;
        }
        assert!(same_recovery_claim(
            &claimed,
            &settled,
            "agent-worker",
            h.now
        ));
        for case in 0..8 {
            let mut changed = settled.clone();
            match case {
                0 => changed.conversation.settings = Some(changed.settings()),
                1 => changed.conversation.active_turn = Some(999),
                2 => changed.work.lease.as_mut().unwrap().fence += 1,
                3 => changed.conversation.runtime_generation = Some(2),
                4 => changed.conversation.first_message.as_mut().unwrap().text = "new input".into(),
                5 => changed.conversation.first_message.as_mut().unwrap().attempt += 1,
                6 => changed.requested_stop_reason = Some(AgentStopReason::Owner),
                _ => changed.status = AgentStatus::Stopped,
            }
            assert!(
                !same_recovery_claim(&claimed, &changed, "agent-worker", h.now),
                "case {case}"
            );
        }
        server.abort();
    }
}
