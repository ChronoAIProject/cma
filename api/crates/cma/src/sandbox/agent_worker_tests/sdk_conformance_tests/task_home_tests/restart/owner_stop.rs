use super::*;

#[path = "owner_stop_scheduler.rs"]
mod scheduler;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at an isolated local replica set"]
async fn owner_stop_without_capability_then_restart_with_retained_history() {
    for segmented in [false, true] {
        Box::pin(exercise_owner_stop_restart(false, segmented, false, None)).await;
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at an isolated local replica set"]
async fn historical_owner_stop_requires_close_proof_then_restarts_with_one_terminal_event() {
    for segmented in [false, true] {
        Box::pin(exercise_owner_stop_restart(true, segmented, false, None)).await;
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at an isolated local replica set"]
async fn owner_restart_publishes_ready_after_sandbox_activity() {
    for segmented in [false, true] {
        Box::pin(exercise_owner_stop_restart(false, segmented, true, None)).await;
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at an isolated local replica set"]
async fn owner_stop_from_peer_ingress_settles_via_scheduler_then_restarts() {
    for segmented in [false, true] {
        for fault in [
            scheduler::Fault::OwnershipConflict,
            scheduler::Fault::LostCommitReply,
        ] {
            Box::pin(exercise_owner_stop_restart(
                false,
                segmented,
                false,
                Some(fault),
            ))
            .await;
        }
    }
}

async fn exercise_owner_stop_restart(
    historical: bool,
    segmented: bool,
    touch_sandbox: bool,
    scheduler_fault: Option<scheduler::Fault>,
) {
    use crate::sandbox::agent_process_proof::RestartObservation;
    use crate::store::budget_test_fixture::OwnerStopBudgetFixture;
    use crate::store::skill_result_test_support::{assert_format, enable_segments};
    use layer2_core::sandbox::store::{AgentLogPublicationIdentity, AgentStopRequest};
    use layer2_core::sandbox::{AgentRestartControl, AgentRestartService, AgentServiceError};
    use layer2_core::Caller;
    use mongodb::bson::{doc, Document};

    let uri = std::env::var(if scheduler_fault.is_some() {
        "CHRONO_TEST_MONGODB_FAILPOINT_URI"
    } else {
        "CHRONO_TEST_MONGODB_URI"
    })
    .unwrap();
    let name = format!("cma_owner_stop_restart_{}", uuid::Uuid::new_v4().simple());
    let mut options = mongodb::options::ClientOptions::parse(uri).await.unwrap();
    options.app_name = Some(name.clone());
    let client = mongodb::Client::with_options(options).unwrap();
    let database = client.database(&name);
    crate::store::MongoOperationStore::open_for_test(&client, database.name())
        .await
        .unwrap();
    let store = MongoSandboxStore::open(&client, database.name())
        .await
        .unwrap()
        .with_segment_adoption_enabled(segmented);
    let (store, coordinator) = if scheduler_fault.is_some() {
        let store = store
            .enable_agent_ownership("stop-worker".into())
            .await
            .unwrap();
        let coordinator = scheduler::register(&store, "stop-worker").await;
        (store, Some(coordinator))
    } else {
        (store, None)
    };
    let store = Arc::new(store);
    let skills = Arc::new(EmptyAgentSkillResolver);
    let mut harness =
        Harness::with_store_services_home(store.clone(), skills.clone(), 1, None, true).await;
    if let Some(coordinator) = &coordinator {
        harness.registry.install_ownership(coordinator.clone());
    }
    let mut sandbox = store
        .get_sandbox(&harness.workspace.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    // The production access-tool source runs before the native successor Open.
    // A synthetic runtime alone cannot catch a refusal at that boundary.
    use base64::Engine as _;
    let encoded = base64::engine::general_purpose::STANDARD.encode([9_u8; 32]);
    let encryption = crate::store::AuthEncryptionKey::from_base64(&encoded).unwrap();
    let access_tools = crate::store::MongoAgentAccessToolSource::new(
        store.clone(),
        "https://control.example.test",
        &encryption,
    )
    .unwrap();
    sandbox.credential.current.as_mut().unwrap().expires_at =
        rfc3339_at(SystemTime::now() + Duration::from_secs(3600));
    sandbox.revision += 1;
    store
        .save_sandbox(sandbox.revision - 1, sandbox.clone())
        .await
        .unwrap();
    let close_proofs = Arc::new(AtomicUsize::new(0));
    let close_refusal = Arc::new(Mutex::new(
        historical.then_some("agent_process_close_runtime_changed"),
    ));
    let restart_supported = Arc::new(AtomicBool::new(false));
    let adapter = Arc::new(skill_home::with_bound_home(
        CodexAdapter::new(
            Arc::new(RuntimeBoundary {
                backend: harness.backend.clone(),
                prompts: Arc::new(Mutex::new(Vec::new())),
                lose_prompt_response: false,
                close_proofs: Some(close_proofs.clone()),
                restart_supported: restart_supported.clone(),
                close_refusal: close_refusal.clone(),
                replay_restart_history: !historical,
                touch_sandbox_on_restart: touch_sandbox
                    .then(|| (store.clone(), sandbox.id.clone())),
            }),
            CodexSandboxSpec::default(),
        )
        .with_access_tools(access_tools),
        sandbox.lifecycle.home.as_deref().unwrap(),
    ));
    harness.adapter = adapter.clone();
    let operations = AgentOperationService::new(store.clone(), harness.registry.clone());
    let mut config =
        AgentWorkerConfig::new("agent-worker", DEPLOYMENT_ID).with_ownership(coordinator.clone());
    config.owner_stop_settlement_enabled = !historical;
    harness.worker = AgentWorker::new(
        AgentWorkerDependencies {
            store: store.clone(),
            adapter: adapter.clone(),
            skills,
            registry: harness.registry.clone(),
            updates: harness.updates.clone(),
            operations: operations.clone(),
        },
        config,
    );
    harness.now = SystemTime::now();
    let agent = harness
        .admit("owner-stop-without-restart", Vec::new())
        .await;
    let ready = harness.drive_ready(&agent.id).await;
    let authority = if let Some(coordinator) = &coordinator {
        Some(
            coordinator
                .local(&agent.id)
                .await
                .expect("worker owns the live Agent"),
        )
    } else {
        None
    };
    let tool_records = database.collection::<Document>("agent_access_tools");
    let original_open = layer3_adapter::AgentProcessAction::Open.operation_id(agent.id.as_str(), 1);
    let original_tool = tool_records
        .find_one(doc! {
            "agent_id": agent.id.as_str(),
            "record.request.operation_id": original_open,
            "record.conversation": ready.conversation.conversation_id.as_deref().unwrap(),
        })
        .await
        .unwrap()
        .expect("the original Open installed the managed access tool");
    if segmented {
        enable_segments(&store, &ready).await;
        let marker = AgentLogKind::WorkStepCompleted {
            step: AgentWorkStep::None,
        };
        let identity = AgentLogPublicationIdentity {
            key: "restart-adoption".into(),
            appended_at: layer2_core::now_rfc3339(),
            fingerprint: [2; 32],
        };
        let append = store.append_agent_log_keyed(&ready.id, &identity, marker);
        if let Some(authority) = &authority {
            authority.run(append).await.unwrap().unwrap();
        } else {
            append.await.unwrap();
        }
    }
    assert_format(&store, &ready.id, segmented).await;
    assert!(
        !adapter
            .runtime_status(&harness.runtime)
            .await
            .unwrap()
            .agent_process_restart
    );
    let caller = Caller {
        subject: ready.owner.clone(),
        authorization: Authorization::new([Capability::UseOwnSandboxes]),
    };
    let send = operations.send_message(&caller, &ready.id, "Retain this test turn".into());
    if let Some(authority) = &authority {
        authority.run(send).await.unwrap().unwrap();
    } else {
        send.await.unwrap();
    }
    let working = harness.stored(&agent.id).await;
    let turn = working.conversation.active_turn.unwrap();
    assert_eq!(working.status, AgentStatus::Working);
    let budget = if historical || scheduler_fault.is_some() {
        Some(OwnerStopBudgetFixture::reserve(&store, &working, turn).await)
    } else {
        None
    };
    if let (Some(coordinator), Some(fault)) = (&coordinator, scheduler_fault) {
        scheduler::exercise(
            (&client, &database),
            &harness,
            &working,
            &caller,
            &sandbox,
            coordinator,
            &restart_supported,
            budget.as_ref().unwrap(),
            fault,
        )
        .await;
        // A lost commit acknowledgement may replay the proof-only Close request.
        // Durable terminal publication and interruption remain exactly once.
        let history = store.agent_log(&agent.id, 0, 100).await.unwrap();
        assert_eq!(
            history.iter().filter(|row| matches!(
                row.kind, AgentLogKind::TurnInterrupted { turn_sequence, .. } if turn_sequence == turn
            )).count(),
            1,
        );
        database.drop().await.unwrap();
        return;
    }
    let now = SystemTime::now();
    store
        .schedule_agent_stop(AgentStopRequest {
            target: AgentControlTarget::from(&working),
            reason: AgentStopReason::Owner,
            requested_at: rfc3339_at(now),
        })
        .await
        .unwrap();
    for _ in 0..3 {
        harness.run(&agent.id, now).await;
    }
    let stopped = harness.stored(&agent.id).await;
    assert_eq!(stopped.status, AgentStatus::Stopped);
    assert_eq!(stopped.conversation.active_turn, historical.then_some(turn));
    assert_eq!(
        stopped.conversation.conversation_id,
        ready.conversation.conversation_id
    );
    assert_eq!(stopped.worktree_id, ready.worktree_id);
    let restart = AgentRestartService::new(
        store.clone(),
        Arc::new(RestartObservation {
            adapter,
            deployment_id: DEPLOYMENT_ID.into(),
        }),
    );
    let refusal = if historical {
        AgentServiceError::RestartTurnUnsettled
    } else {
        AgentServiceError::RestartRuntimeUnsupported
    };
    let key = "stop-restart-fixture";
    assert_eq!(restart.restart(&caller, &agent.id, key).await, Err(refusal));
    assert_eq!(harness.stored(&agent.id).await, stopped);
    if let Some(budget) = budget {
        budget.assert_held(&store).await;
        assert_eq!(close_proofs.load(Ordering::SeqCst), 0);
        restart_supported.store(true, Ordering::SeqCst);
        assert_eq!(
            restart.restart(&caller, &agent.id, key).await,
            Err(AgentServiceError::ControlConflict)
        );
        assert_eq!(harness.stored(&agent.id).await, stopped);
        budget.assert_held(&store).await;
        *close_refusal.lock().unwrap() = Some("agent_process_close_unproved");
        assert_eq!(
            restart.restart(&caller, &agent.id, key).await,
            Err(AgentServiceError::RestartTurnUnsettled)
        );
        assert_eq!(harness.stored(&agent.id).await, stopped);
        budget.assert_held(&store).await;
        *close_refusal.lock().unwrap() = None;
        let acceptance = restart.restart(&caller, &agent.id, key).await.unwrap();
        if !segmented {
            let row = database
                .collection::<Document>("agents")
                .find_one(doc! {"_id": agent.id.as_str()})
                .await
                .unwrap()
                .unwrap();
            let expected = i64::try_from(stopped.conversation.last_seq + 1).unwrap();
            let conversation = row
                .get_document("record")
                .unwrap()
                .get_document("conversation")
                .unwrap();
            assert_eq!(row.get_i64("last_seq").unwrap(), expected);
            assert_eq!(conversation.get_i64("last_seq").unwrap(), expected);
        }
        let starting = harness.stored(&agent.id).await;
        assert_eq!(starting.status, AgentStatus::Starting);
        assert_eq!(starting.work.step, AgentWorkStep::RestartConversation);
        assert_eq!(starting.conversation.active_turn, None);
        assert_eq!(
            starting.conversation.last_seq,
            stopped.conversation.last_seq + 1
        );
        assert_eq!(
            starting.conversation.conversation_id,
            stopped.conversation.conversation_id
        );
        assert_eq!(starting.worktree_id, stopped.worktree_id);
        assert_eq!(starting.sandbox_id, stopped.sandbox_id);
        budget.assert_settled_once(&store).await;
        assert_eq!(
            restart.restart(&caller, &agent.id, key).await.unwrap(),
            acceptance
        );
        harness.run(&agent.id, SystemTime::now()).await;
        let restarted = harness.stored(&agent.id).await;
        assert_eq!(restarted.status, AgentStatus::Ready);
        assert_eq!(restarted.work.step, AgentWorkStep::None);
        assert_eq!(restarted.id, stopped.id);
        assert_eq!(restarted.worktree_id, stopped.worktree_id);
        assert_eq!(
            restarted.conversation.conversation_id,
            stopped.conversation.conversation_id
        );
        let successor_open =
            layer3_adapter::AgentProcessAction::Open.operation_id(agent.id.as_str(), 2);
        assert!(tool_records
            .find_one(doc! {
                "_id": original_tool.get("_id").unwrap().clone(),
                "record.request.operation_id": successor_open,
                "record.conversation": restarted.conversation.conversation_id.as_deref().unwrap(),
            })
            .await
            .unwrap()
            .is_some());
        let retained = store
            .get_sandbox(&restarted.sandbox_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(retained.lifecycle.home, sandbox.lifecycle.home);
        assert_eq!(retained.runtime, sandbox.runtime);
        assert_eq!(
            restart.restart(&caller, &agent.id, key).await.unwrap(),
            acceptance
        );
        budget.assert_settled_once(&store).await;
    } else {
        restart_supported.store(true, Ordering::SeqCst);
        let acceptance = restart.restart(&caller, &agent.id, key).await.unwrap();
        assert_eq!(
            restart.restart(&caller, &agent.id, key).await.unwrap(),
            acceptance
        );
        let parent_before_restart = store
            .get_sandbox(&stopped.sandbox_id)
            .await
            .unwrap()
            .unwrap();
        harness.run(&agent.id, SystemTime::now()).await;
        let ready = harness.stored(&agent.id).await;
        assert_eq!(ready.status, AgentStatus::Ready);
        assert_eq!(ready.work.step, AgentWorkStep::None);
        assert_eq!(ready.id, stopped.id);
        assert_eq!(ready.worktree_id, stopped.worktree_id);
        assert_eq!(
            ready.conversation.conversation_id,
            stopped.conversation.conversation_id
        );
        assert_eq!(
            ready.conversation.last_seq,
            stopped.conversation.last_seq + 2
        );
        let checkpoint = store
            .agent_message_checkpoint(&agent.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(checkpoint.runtime_generation, 2);
        assert_eq!(checkpoint.output.unwrap().cursor, 4);
        assert_eq!(
            restart.restart(&caller, &agent.id, key).await.unwrap(),
            acceptance
        );
        assert_eq!(ready.sandbox_id, stopped.sandbox_id);
        let retained = store.get_sandbox(&ready.sandbox_id).await.unwrap().unwrap();
        assert_eq!(retained.lifecycle.home, sandbox.lifecycle.home);
        assert_eq!(retained.runtime, sandbox.runtime);
        if touch_sandbox {
            assert_eq!(retained.revision, parent_before_restart.revision + 1);
            assert!(retained.last_activity_at > parent_before_restart.last_activity_at);
            let open = layer3_adapter::AgentProcessAction::Open.operation_id(agent.id.as_str(), 2);
            let requests = harness.backend.requests().await;
            assert_eq!(
                requests
                    .iter()
                    .filter(|(_, request)| matches!(request,
                        RequestFrame::OpenAgent(request) if request.request.operation_id == open
                    ))
                    .count(),
                1
            );
        }
        operations
            .send_message(&caller, &agent.id, "Continue the same conversation".into())
            .await
            .unwrap();
        let continued = harness.stored(&agent.id).await;
        assert_eq!(continued.status, AgentStatus::Working);
        assert_eq!(
            continued.conversation.conversation_id,
            ready.conversation.conversation_id
        );
    }
    let history = store.agent_log(&agent.id, 0, 50).await.unwrap();
    assert_eq!(history.iter().filter(|row| matches!(
        row.kind, AgentLogKind::TurnInterrupted { turn_sequence, .. } if turn_sequence == turn
    )).count(), 1);
    assert_eq!(
        close_proofs.load(Ordering::SeqCst),
        if historical { 1 } else { 2 }
    );
    database.drop().await.unwrap();
}

pub(super) async fn replay_retained_history(
    conversation: &str,
    updates: &mpsc::Sender<ConversationUpdate>,
) -> layer3_adapter::Result<()> {
    use layer3_adapter::{
        RuntimeOutputReceipt, RuntimeOutputSource, RuntimeOutputUpdate, SessionUpdate,
    };
    let (ack, mut acknowledged) = mpsc::channel(1);
    let chunk = || layer3_adapter::ContentChunk::new(ContentBlock::from("prior turn"));
    // Match the real ACP bridge: initialize and load replies have receipts
    // but no effect, and historical assistant output has no prompt authority.
    let effects = [
        None,
        Some(ConversationUpdate::Said(SessionUpdate::UserMessageChunk(
            chunk(),
        ))),
        Some(
            ConversationUpdate::Said(SessionUpdate::AgentMessageChunk(chunk()))
                .without_prompt_evidence(),
        ),
        None,
    ];
    let stream_id = uuid::Uuid::new_v4().to_string();
    for (index, effect) in effects.into_iter().enumerate() {
        let source = RuntimeOutputSource {
            stream_id: stream_id.clone(),
            cursor: index as u64 + 1,
            native_conversation_id: conversation.into(),
            payload_sha256: format!("{index:064x}"),
        };
        updates
            .send(ConversationUpdate::RuntimeOutput {
                receipt: RuntimeOutputReceipt::new(source.clone(), ack.clone()),
                update: effect.map(|update| RuntimeOutputUpdate::new(update, true)),
            })
            .await
            .map_err(|_| AdapterError::Unavailable("restart output receiver closed".into()))?;
        let receipt = tokio::time::timeout(Duration::from_secs(3), acknowledged.recv())
            .await
            .map_err(|_| {
                AdapterError::Unavailable("restart output acknowledgment timed out".into())
            })?;
        assert_eq!(receipt, Some(source));
    }
    Ok(())
}
