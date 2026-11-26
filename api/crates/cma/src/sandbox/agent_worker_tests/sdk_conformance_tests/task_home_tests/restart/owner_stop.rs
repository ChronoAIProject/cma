use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at an isolated local replica set"]
async fn owner_stop_settles_inflight_turn_without_runtime_restart_capability() {
    exercise_owner_stop_restart(false, false).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at an isolated local replica set"]
async fn historical_owner_stop_requires_close_proof_then_restarts_with_one_terminal_event() {
    for segmented in [false, true] {
        Box::pin(exercise_owner_stop_restart(true, segmented)).await;
    }
}

async fn exercise_owner_stop_restart(historical: bool, segmented: bool) {
    use crate::sandbox::agent_process_proof::RestartObservation;
    use crate::store::budget_test_fixture::OwnerStopBudgetFixture;
    use crate::store::skill_result_test_support::{assert_format, enable_segments};
    use layer2_core::sandbox::store::{AgentLogPublicationIdentity, AgentStopRequest};
    use layer2_core::sandbox::{AgentRestartControl, AgentRestartService, AgentServiceError};
    use layer2_core::Caller;
    use mongodb::bson::{doc, Document};

    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let client = mongodb::Client::with_uri_str(uri).await.unwrap();
    let database = client.database(&format!(
        "cma_owner_stop_restart_{}",
        uuid::Uuid::new_v4().simple()
    ));
    crate::store::MongoOperationStore::open_for_test(&client, database.name())
        .await
        .unwrap();
    let store = Arc::new(
        MongoSandboxStore::open(&client, database.name())
            .await
            .unwrap()
            .with_segment_adoption_enabled(segmented),
    );
    let skills = Arc::new(EmptyAgentSkillResolver);
    let mut harness =
        Harness::with_store_services_home(store.clone(), skills.clone(), 1, None, true).await;
    let sandbox = store
        .get_sandbox(&harness.workspace.sandbox_id)
        .await
        .unwrap()
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
            }),
            CodexSandboxSpec::default(),
        ),
        sandbox.lifecycle.home.as_deref().unwrap(),
    ));
    harness.adapter = adapter.clone();
    let operations = AgentOperationService::new(store.clone(), harness.registry.clone());
    let mut config = AgentWorkerConfig::new("agent-worker", DEPLOYMENT_ID);
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
    if segmented {
        enable_segments(&store, &ready).await;
        let marker = AgentLogKind::WorkStepCompleted {
            step: AgentWorkStep::None,
        };
        store
            .append_agent_log_keyed(
                &ready.id,
                &AgentLogPublicationIdentity {
                    key: "restart-adoption".into(),
                    appended_at: layer2_core::now_rfc3339(),
                    fingerprint: [2; 32],
                },
                marker,
            )
            .await
            .unwrap();
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
    operations
        .send_message(&caller, &ready.id, "Retain this test turn".into())
        .await
        .unwrap();
    let working = harness.stored(&agent.id).await;
    let turn = working.conversation.active_turn.unwrap();
    assert_eq!(working.status, AgentStatus::Working);
    let budget = if historical {
        Some(OwnerStopBudgetFixture::reserve(&store, &working, turn).await)
    } else {
        None
    };
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
    }
    let history = store.agent_log(&agent.id, 0, 50).await.unwrap();
    assert_eq!(history.iter().filter(|row| matches!(
        row.kind, AgentLogKind::TurnInterrupted { turn_sequence, .. } if turn_sequence == turn
    )).count(), 1);
    assert_eq!(close_proofs.load(Ordering::SeqCst), 1);
    database.drop().await.unwrap();
}
