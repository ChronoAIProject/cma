use super::*;
use layer2_core::sandbox::*;
use std::sync::atomic::Ordering;

fn pause(parent: &mut Sandbox) {
    parent.status = SandboxStatus::Paused;
    parent.runtime = None;
    parent.lifecycle.checkpoints.latest = Some(
        SandboxCheckpointRef::seal(
            &layer3_adapter::CheckpointReceipt {
                epoch: 1,
                object_reference: "settings-fixture-checkpoint".into(),
                sha256: "a".repeat(64),
                writers: layer3_adapter::WriterState::Resumed,
            },
            &[],
            parent.created_at.clone(),
        )
        .unwrap(),
    );
}

async fn queue_message(store: &MongoSandboxStore, agent: &Agent) {
    let original = store.get_sandbox(&agent.sandbox_id).await.unwrap().unwrap();
    let mut paused = original.clone();
    paused.revision += 1;
    pause(&mut paused);
    store
        .save_sandbox(original.revision, paused.clone())
        .await
        .unwrap();
    let durable = Arc::new(store.clone());
    let runtime = Arc::new(super::runtime::Runtime::new(durable.clone()));
    let service = AgentOperationService::new(durable, runtime.clone());
    let caller = layer2_core::Caller::new(
        layer2_core::SubjectId::new(OWNER),
        crate::auth::AuthState::trusted_authorization(),
    );
    let queued = service
        .send_message(&caller, &agent.id, "Undelivered message".into())
        .await
        .unwrap();
    assert!(matches!(queued, AgentMessageOutcome::Queued { .. }));
    assert!(runtime.resolutions.lock().unwrap().is_empty());
    let mut active = original;
    active.revision = paused.revision + 1;
    store.save_sandbox(paused.revision, active).await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn routed_settings_keep_busy_parent_and_generation_refusals() {
    for condition in [
        "turn",
        "queue",
        "first-message",
        "paused",
        "retiring",
        "generation",
    ] {
        let (mongo, name, durable) =
            crate::store::sandbox_maintenance::mongo_integration_tests::database().await;
        seed(&durable).await;
        let mut agent = durable
            .get_agent(&AgentId(AGENT.into()))
            .await
            .unwrap()
            .unwrap();
        match condition {
            "turn" => {
                agent.status = AgentStatus::Working;
                agent.conversation.active_turn = Some(1);
            }
            "queue" => queue_message(&durable, &agent).await,
            "first-message" => {
                agent.conversation.first_message = Some(
                    FirstMessageRecord::new(
                        "settings-first-message",
                        "Reserved task",
                        &agent.created_at,
                    )
                    .unwrap(),
                );
            }
            "paused" | "retiring" => {
                let mut parent = durable
                    .get_sandbox(&agent.sandbox_id)
                    .await
                    .unwrap()
                    .unwrap();
                let revision = parent.revision;
                parent.revision += 1;
                parent.status = if condition == "paused" {
                    SandboxStatus::Paused
                } else {
                    SandboxStatus::Retiring
                };
                if condition == "paused" {
                    pause(&mut parent);
                }
                durable.save_sandbox(revision, parent).await.unwrap();
            }
            "generation" => {}
            _ => unreachable!(),
        }
        if matches!(condition, "turn" | "first-message") {
            let revision = agent.revision;
            agent.revision += 1;
            durable.save_agent(revision, agent).await.unwrap();
        }
        let replicas = vec![
            Replica::open(&name, true).await,
            Replica::open(&name, true).await,
        ];
        let (owner, _) = fixture::acquire(&replicas).await;
        if condition == "generation" {
            replicas[owner]
                .runtime
                .target
                .lock()
                .unwrap()
                .as_mut()
                .unwrap()
                .runtime_generation = 2;
        }
        let before = durable
            .get_agent(&AgentId(AGENT.into()))
            .await
            .unwrap()
            .unwrap();
        let (status, problem) = replicas[1 - owner]
            .request(Method::PATCH, Some("refused-settings"), Some(changed()))
            .await;
        assert_eq!(
            status,
            if condition == "generation" {
                StatusCode::SERVICE_UNAVAILABLE
            } else {
                StatusCode::CONFLICT
            },
            "{condition}: {problem}"
        );
        assert_eq!(
            problem["code"],
            if condition == "generation" {
                "instance_service_unavailable"
            } else {
                "agent_not_ready"
            },
            "{condition}: {problem}"
        );
        assert_eq!(
            durable.get_agent(&before.id).await.unwrap().unwrap(),
            before
        );
        assert!(replicas
            .iter()
            .all(|replica| replica.runtime.inputs.lock().unwrap().is_empty()));
        assert!(replicas[1 - owner]
            .runtime
            .resolutions
            .lock()
            .unwrap()
            .is_empty());
        for replica in replicas {
            replica.close().await;
        }
        mongo.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn routed_settings_uncertain_confirmation_retains_one_pending_operation() {
    let (mongo, name, durable) =
        crate::store::sandbox_maintenance::mongo_integration_tests::database().await;
    seed(&durable).await;
    let replicas = vec![
        Replica::open(&name, true).await,
        Replica::open(&name, true).await,
    ];
    let (owner, _) = fixture::acquire(&replicas).await;
    replicas[owner]
        .runtime
        .ambiguous
        .store(true, Ordering::SeqCst);
    let result = replicas[1 - owner]
        .request(Method::PATCH, Some("uncertain-settings"), Some(changed()))
        .await;
    assert_eq!(result.0, StatusCode::SERVICE_UNAVAILABLE, "{}", result.1);
    assert_eq!(result.1["code"], "agent_delivery_ambiguous");
    let admitted = durable
        .get_agent(&AgentId(AGENT.into()))
        .await
        .unwrap()
        .unwrap();
    assert!(admitted.settings_pending());
    assert_eq!(
        admitted.conversation.operation.as_ref().unwrap().state,
        AgentOperationDelivery::RecoveryRequired
    );
    let retained: Agent = serde_json::from_slice(&serde_json::to_vec(&admitted).unwrap()).unwrap();
    assert_eq!(retained, admitted);
    for replica in &replicas {
        assert_eq!(
            replica
                .request(Method::PATCH, Some("uncertain-settings"), Some(changed()))
                .await,
            result
        );
        let (status, settings) = replica.request(Method::GET, None, None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(settings["pending"], true);
        assert_eq!(settings["reasoning_effort"], "medium");
    }
    let (status, problem) = replicas[1 - owner]
        .request(Method::PATCH, Some("second-intent"), Some(changed()))
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{problem}");
    assert_eq!(problem["code"], "agent_operation_in_flight");
    assert_eq!(
        durable.get_agent(&admitted.id).await.unwrap().unwrap(),
        admitted
    );
    assert_eq!(replicas[owner].runtime.inputs.lock().unwrap().len(), 1);
    assert!(replicas[1 - owner]
        .runtime
        .inputs
        .lock()
        .unwrap()
        .is_empty());
    for replica in replicas {
        replica.close().await;
    }
    mongo.database(&name).drop().await.unwrap();
}
