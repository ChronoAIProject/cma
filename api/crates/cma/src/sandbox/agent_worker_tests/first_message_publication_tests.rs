use super::*;
use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::sandbox::{AgentWorkAuthority, FirstMessageDispatchOutcome};

async fn claimed_dispatch(harness: &Harness, prompt: Option<&str>) -> Agent {
    let admitted = harness
        .admit_with_first_message("dispatch-publication", vec![], prompt)
        .await;
    for _ in 0..5 {
        harness.run(&admitted.id, harness.now).await;
    }
    let mut current = harness.stored(&admitted.id).await;
    if current.work.step == AgentWorkStep::None {
        let revision = current.revision;
        current
            .work
            .schedule(AgentWorkStep::DispatchFirstMessage, rfc3339_at(harness.now));
        current.revision += 1;
        harness.store.save_agent(revision, current).await.unwrap();
    }
    harness
        .store
        .claim_agent_work(
            &admitted.id,
            "agent-worker",
            rfc3339_at(harness.now),
            rfc3339_at(harness.now + Duration::from_secs(18 * 60)),
        )
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn completed_first_dispatch_retries_revision_contention_without_resending() {
    for prompt in [None, Some("Run once")] {
        let mut harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
        harness.now = SystemTime::now();
        let claimed = claimed_dispatch(&harness, prompt).await;
        let authority = AgentWorkAuthority::new(
            "agent-worker".into(),
            claimed.work.lease.as_ref().unwrap().fence,
        )
        .unwrap();
        let outcome = AgentOperationService::new(harness.store.clone(), harness.registry.clone())
            .dispatch_queued_first_message(&claimed.id, authority)
            .await
            .unwrap();
        let calls = AtomicUsize::new(0);
        harness
            .worker
            .retry_dispatch_publication(&claimed, &outcome, harness.now, |candidate, at| {
                let attempt = calls.fetch_add(1, Ordering::SeqCst);
                let harness = &harness;
                async move {
                    if attempt < 3 {
                        let mut concurrent = candidate.clone();
                        concurrent.title = Some("Owner title during first turn".into());
                        concurrent.revision += 1;
                        harness
                            .store
                            .save_agent(candidate.revision, concurrent)
                            .await
                            .unwrap();
                    }
                    harness
                        .worker
                        .finish_dispatch_publication(candidate, at)
                        .await
                }
            })
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 4);
        let completed = harness.stored(&claimed.id).await;
        assert_eq!(completed.work.step, AgentWorkStep::None);
        assert!(completed.work.lease.is_none());
        assert_eq!(
            completed.title.as_deref(),
            Some("Owner title during first turn")
        );
        assert_eq!(
            harness
                .store
                .agent_log(&claimed.id, 0, 100)
                .await
                .unwrap()
                .iter()
                .filter(|entry| matches!(entry.kind, AgentLogKind::OwnerMessage { .. }))
                .count(),
            usize::from(prompt.is_some())
        );
        if let Some(record) = completed.conversation.first_message {
            assert_eq!(record.attempt, 1);
            assert_eq!(record.state, FirstMessageDelivery::WriteConfirmed);
        }
    }
}

#[tokio::test]
async fn completed_dispatch_refuses_replaced_authority_and_new_prompt() {
    for changed in ["holder", "fence", "step", "runtime", "stop", "prompt"] {
        let mut harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
        harness.now = SystemTime::now();
        let claimed = claimed_dispatch(&harness, None).await;
        let calls = AtomicUsize::new(0);
        let result = harness
            .worker
            .retry_dispatch_publication(
                &claimed,
                &FirstMessageDispatchOutcome::NotConfigured,
                harness.now,
                |mut candidate, _| {
                    calls.fetch_add(1, Ordering::SeqCst);
                    let store = harness.store.clone();
                    async move {
                        let revision = candidate.revision;
                        candidate.revision += 1;
                        match changed {
                            "holder" => {
                                candidate.work.lease.as_mut().unwrap().holder = "replacement".into()
                            }
                            "fence" => {
                                candidate.work.lease.as_mut().unwrap().fence += 1;
                                candidate.work.next_fence += 1;
                            }
                            "step" => candidate.work.step = AgentWorkStep::PublishConnection,
                            "runtime" => candidate.conversation.runtime_generation = Some(99),
                            "stop" => {
                                candidate.requested_stop_reason = Some(AgentStopReason::Owner)
                            }
                            "prompt" => {
                                candidate.conversation.first_message = Some(
                                    FirstMessageRecord::new(
                                        "late-prompt",
                                        "Do not discard this prompt",
                                        candidate.updated_at.clone(),
                                    )
                                    .unwrap(),
                                )
                            }
                            _ => unreachable!(),
                        }
                        store.save_agent(revision, candidate).await.unwrap();
                        Err(SandboxStoreError::Conflict)
                    }
                },
            )
            .await;
        assert_eq!(result, Err(SandboxStoreError::Conflict), "{changed}");
        assert_eq!(calls.load(Ordering::SeqCst), 1, "{changed}");
        assert!(harness.stored(&claimed.id).await.work.lease.is_some());
    }
}

#[tokio::test(start_paused = true)]
async fn completed_dispatch_bounds_contention_and_respects_lease_expiry() {
    for expire in [false, true] {
        let mut harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
        harness.now = SystemTime::now();
        let claimed = claimed_dispatch(&harness, None).await;
        let calls = AtomicUsize::new(0);
        let result = harness
            .worker
            .retry_dispatch_publication(
                &claimed,
                &FirstMessageDispatchOutcome::NotConfigured,
                harness.now,
                |_, _| {
                    calls.fetch_add(1, Ordering::SeqCst);
                    async move {
                        if expire {
                            tokio::time::sleep(Duration::from_secs(18 * 60)).await;
                        }
                        Err(SandboxStoreError::Conflict)
                    }
                },
            )
            .await;
        assert_eq!(result, Err(SandboxStoreError::Conflict));
        assert_eq!(calls.load(Ordering::SeqCst), if expire { 1 } else { 8 });
        assert_eq!(harness.stored(&claimed.id).await.work, claimed.work);
    }
}

#[tokio::test]
async fn completed_dispatch_cannot_retire_a_different_prompt_attempt() {
    let mut harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    harness.now = SystemTime::now();
    let claimed = claimed_dispatch(&harness, Some("Run once")).await;
    let authority = AgentWorkAuthority::new(
        "agent-worker".into(),
        claimed.work.lease.as_ref().unwrap().fence,
    )
    .unwrap();
    let outcome = AgentOperationService::new(harness.store.clone(), harness.registry.clone())
        .dispatch_queued_first_message(&claimed.id, authority)
        .await
        .unwrap();
    let mut current = harness.stored(&claimed.id).await;
    let revision = current.revision;
    current.conversation.first_message.as_mut().unwrap().attempt += 1;
    current.revision += 1;
    harness
        .store
        .save_agent(revision, current.clone())
        .await
        .unwrap();
    let calls = AtomicUsize::new(0);
    let result = harness
        .worker
        .retry_dispatch_publication(&claimed, &outcome, harness.now, |_, _| {
            calls.fetch_add(1, Ordering::SeqCst);
            async { Ok(()) }
        })
        .await;
    assert_eq!(result, Err(SandboxStoreError::Conflict));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(harness.stored(&claimed.id).await, current);
}
