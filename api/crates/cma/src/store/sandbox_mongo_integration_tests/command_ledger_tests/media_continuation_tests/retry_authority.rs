#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn media_continuation_retry_requires_exact_atomic_no_write_pair_and_current_lifecycle() {
    let (f, claimed) = deferred_fixture("media_retry_proof").await;
    let saved = f
        .store
        .get_command(&claimed.receipt.envelope.command_id)
        .await
        .unwrap()
        .unwrap();
    let agent = f.store.get_agent(&f.agent.id).await.unwrap().unwrap();
    let logs = f
        .store
        .agent_log_prefix(&f.agent.id, agent.conversation.last_seq)
        .await
        .unwrap();
    let reads = AgentReadService::new(f.store.clone(), Arc::new(Resources))
        .with_resource_access(f.access.clone());
    let authorization = reads
        .authorize_event_stream(
            &f.caller.context,
            Some(f.caller.credential.as_ref().unwrap().value()),
            &f.agent.id,
        )
        .await
        .unwrap();
    let reading = f
        .commands
        .response_for_stream(&authorization, saved.receipt.response_id.as_ref().unwrap())
        .await
        .unwrap();
    let visible = reads.event_stream_snapshot(&authorization).await.unwrap();
    let failed = visible
        .history
        .iter()
        .find(|entry| entry.deferred_failure_detail.is_some())
        .unwrap();
    assert_eq!(
        failed.deferred_failure_detail,
        reading.deferred_failure_detail
    );
    assert!(
        matches!(&failed.kind,Some(AgentLogKind::TurnFailed {failure,..})
            if failure.class == SOURCE_DEFERRED_FAILURE
                && failure.detail == "The Agent input could not be authorized before it was sent.")
    );
    let deferred = logs.iter().position(|entry| matches!(&entry.kind, AgentLogKind::TurnFailed { failure, .. } if failure.class == SOURCE_DEFERRED_FAILURE)).unwrap();
    let prewrite = deferred - 1;
    for case in [
        "valid",
        "generic",
        "wrong-fingerprint",
        "missing-pair",
        "wrong-operation",
        "different-commit",
        "unknown-write",
        "binding-revoked",
        "stop",
        "expired",
    ] {
        let mut evidence_logs = logs.clone();
        let mut current = saved.clone();
        let mut current_agent = agent.clone();
        match case {
            "generic" => {
                let AgentLogKind::TurnFailed { failure, .. } = &mut evidence_logs[deferred].kind
                else {
                    unreachable!()
                };
                failure.class = "input_not_written".into();
            }
            "wrong-fingerprint" => {
                let AgentLogKind::TurnFailed { failure, .. } = &mut evidence_logs[deferred].kind
                else {
                    unreachable!()
                };
                failure.detail.push('x');
            }
            "missing-pair" => {
                evidence_logs[prewrite].kind = AgentLogKind::OwnerMessage {
                    content: "no durable delivery proof".into(),
                }
            }
            "wrong-operation" => {
                let AgentLogKind::OperationDeliveryChanged {
                    operation_sequence, ..
                } = &mut evidence_logs[prewrite].kind
                else {
                    unreachable!()
                };
                *operation_sequence += 100;
            }
            "different-commit" => {
                evidence_logs[deferred].appended_at =
                    layer2_core::rfc3339_at(SystemTime::now() + Duration::from_secs(5))
            }
            "unknown-write" => {
                evidence_logs.truncate(prewrite + 1);
                let AgentLogKind::OperationDeliveryChanged { state, .. } =
                    &mut evidence_logs[prewrite].kind
                else {
                    unreachable!()
                };
                *state = AgentOperationDelivery::RecoveryRequired;
                current_agent.conversation.last_seq = evidence_logs.last().unwrap().seq;
                current_agent.status = AgentStatus::Working;
                current_agent.conversation.active_turn = current
                    .work
                    .dispatch
                    .as_ref()
                    .map(|dispatch| dispatch.turn_sequence);
            }
            "stop" => current_agent.requested_stop_reason = Some(AgentStopReason::Owner),
            "expired" => {
                current.receipt.admitted_at =
                    layer2_core::rfc3339_at(SystemTime::now() - Duration::from_secs(3600));
                current.queue_deadline =
                    layer2_core::rfc3339_at(SystemTime::now() - Duration::from_secs(1));
            }
            _ => {}
        }
        let mut evidence = CommandEvidence::new(&current).unwrap();
        for entry in &evidence_logs {
            evidence.observe(entry).unwrap();
        }
        let mut lane = CommandLane {
            last_order: current.receipt.agent_order,
            next_fence: 2,
            lease: current.work.lease.clone(),
        };
        evidence
            .apply_with_continuation_retry(
                &mut current,
                &mut lane,
                &current_agent,
                case != "binding-revoked",
            )
            .unwrap();
        if case == "valid" {
            assert_eq!(current.status, AgentResponseStatus::Queued);
            assert_eq!(current.work.deferred_attempts.len(), 1);
            assert_eq!(current.receipt, saved.receipt);
            assert!(current.work.dispatch.is_none());
            let mut unbounded = current.clone();
            unbounded.work.deferred_attempts = vec![current.work.deferred_attempts[0].clone(); 257];
            assert_eq!(unbounded.validate(), Err(CommandLedgerError::Corrupt));
        } else {
            assert!(current.work.deferred_attempts.is_empty(), "{case}");
            assert_eq!(current.work.dispatch, saved.work.dispatch, "{case}");
            assert_eq!(
                current.status,
                if case == "unknown-write" {
                    AgentResponseStatus::InProgress
                } else {
                    AgentResponseStatus::Failed
                },
                "{case}"
            );
        }
    }
    // A new process recovers the committed no-write logs without a saved requeue.
    let reopened = MongoSandboxStore::open(&f.client, &f.name).await.unwrap();
    let queued = reopened
        .reconcile_command(&claimed.receipt.envelope.command_id)
        .await
        .unwrap();
    assert_eq!(queued.status, AgentResponseStatus::Queued);
    assert_eq!(queued.work.deferred_attempts.len(), 1);
    assert_eq!(
        reopened
            .reconcile_command(&claimed.receipt.envelope.command_id)
            .await
            .unwrap(),
        queued
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn media_continuation_retirement_or_stop_fences_requeue_and_revocation_blocks_retry() {
    for change in ["binding", "stop-race", "source-after-requeue"] {
        let (f, claimed) = deferred_fixture("media_retry_fences").await;
        let id = &claimed.receipt.envelope.command_id;
        if change == "binding" {
            let mut binding = f.store.binding(&f.path.binding_id).await.unwrap().unwrap();
            let revision = binding.revision;
            binding.state = BindingState::Retired;
            binding.work = BindingWork::RetireCurrentConversations;
            binding
                .advance_revision(layer2_core::now_rfc3339())
                .unwrap();
            f.store.save_binding(Some(revision), binding).await.unwrap();
            let terminal = f.store.reconcile_command(id).await.unwrap();
            assert_eq!(terminal.status, AgentResponseStatus::Failed);
            assert_eq!(
                terminal.work.failure.unwrap().code,
                CommandFailureCode::AuthorityRevoked
            );
            assert!(terminal.work.deferred_attempts.is_empty());
        } else if change == "stop-race" {
            let mut stopped = f.store.get_agent(&f.agent.id).await.unwrap().unwrap();
            let revision = stopped.revision;
            stopped
                .request_stop(AgentStopReason::Owner, layer2_core::now_rfc3339())
                .unwrap();
            let (stop, requeue) = tokio::join!(
                f.store.save_agent(revision, stopped),
                f.store.reconcile_command(id)
            );
            stop.unwrap();
            let result = requeue.unwrap();
            assert!(matches!(
                result.status,
                AgentResponseStatus::Queued | AgentResponseStatus::Failed
            ));
            f.peer.outage.store(false, Ordering::SeqCst);
            if result.status == AgentResponseStatus::Queued {
                tokio::time::sleep(result.continuation_retry_delay(&layer2_core::now_rfc3339()))
                    .await;
                let retry = f
                    .store
                    .claim_next_command(claim_request(
                        &f.agent,
                        "stop-fenced-retry",
                        &layer2_core::now_rfc3339(),
                    ))
                    .await
                    .unwrap()
                    .unwrap();
                let authority = f
                    .auth
                    .authorize_resource_command(&f.access, &retry.receipt.envelope)
                    .await;
                if let Ok(authority) = authority {
                    let (runtime, sends) = runtime(&f, usize::MAX, false);
                    assert!(AgentOperationService::new(f.store.clone(), runtime)
                        .execute_resource_claim(
                            &authority.with_source_guard(f.service.clone()),
                            &retry
                        )
                        .await
                        .is_err());
                    assert_eq!(sends.load(Ordering::SeqCst), 0);
                }
            }
            let agent = f.store.get_agent(&f.agent.id).await.unwrap().unwrap();
            assert_eq!(agent.requested_stop_reason, Some(AgentStopReason::Owner));
        } else {
            let queued = f.store.reconcile_command(id).await.unwrap();
            f.peer.outage.store(false, Ordering::SeqCst);
            f.peer.active.store(false, Ordering::SeqCst);
            assert_eq!(
                f.service.check(&queued.receipt.envelope).await,
                Err(CommandAuthorizationError::Revoked)
            );
        }
        f.close().await;
    }
}
