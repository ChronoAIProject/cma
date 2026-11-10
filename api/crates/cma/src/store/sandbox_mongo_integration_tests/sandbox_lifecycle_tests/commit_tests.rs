use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn lifecycle_commands_are_owner_bound_gap_free_and_concurrently_idempotent() {
    let (client, name) = database("lifecycle_command").await;
    let store = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
    let active = sandbox("sbx_lifecycle_command");
    store.create_sandbox(active.clone()).await.unwrap();

    let results = futures_util::future::join_all((0..16).map(|_| {
        let store = store.clone();
        let active = active.clone();
        async move {
            store
                .schedule_sandbox_lifecycle(SandboxLifecycleCommand::pause(
                    active.id,
                    active.owner,
                    NOW.into(),
                    None,
                ))
                .await
        }
    }))
    .await;
    let mut scheduled = 0;
    let mut replayed = 0;
    for result in results {
        let SandboxLifecycleCommandResult::Accepted(outcome) = result.unwrap() else {
            panic!("eligible pause was refused");
        };
        match outcome.disposition {
            SandboxLifecycleCommandDisposition::Scheduled => scheduled += 1,
            SandboxLifecycleCommandDisposition::AlreadyScheduled => replayed += 1,
            SandboxLifecycleCommandDisposition::AlreadySatisfied => {
                panic!("no worker completed the pause")
            }
        }
    }
    assert_eq!((scheduled, replayed), (1, 15));
    let stored = store.get_sandbox(&active.id).await.unwrap().unwrap();
    assert_eq!(stored.revision, 2);
    assert_eq!(stored.status, SandboxStatus::Pausing);
    assert_eq!(stored.lifecycle.next_authority, 2);
    assert!(matches!(
        stored.lifecycle.operation,
        Some(SandboxLifecycleOperation::Pause { .. })
    ));
    let events = store.sandbox_events(&active.id, 0, 10).await.unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].seq, 1);
    assert_eq!(
        store
            .schedule_sandbox_lifecycle(SandboxLifecycleCommand::pause(
                active.id.clone(),
                SubjectId("stranger".into()),
                MINUTE.into(),
                None,
            ))
            .await,
        Err(SandboxStoreError::NotFound)
    );

    let mut paused = sandbox("sbx_lifecycle_revive");
    paused.name = "lifecycle-revive".into();
    paused.status = SandboxStatus::Paused;
    paused.runtime = None;
    paused.work = SandboxWorkState::new(SandboxWorkStep::None, NOW.into());
    paused.lifecycle.checkpoints.latest = Some(checkpoint(7));
    store.create_sandbox(paused.clone()).await.unwrap();
    let SandboxLifecycleCommandResult::Accepted(revived) = store
        .schedule_sandbox_lifecycle(SandboxLifecycleCommand::revive(
            paused.id.clone(),
            paused.owner.clone(),
            MINUTE.into(),
            SandboxReviveCause::ExplicitRequest,
        ))
        .await
        .unwrap()
    else {
        panic!("eligible revive was refused");
    };
    assert_eq!(
        revived.disposition,
        SandboxLifecycleCommandDisposition::Scheduled
    );
    assert!(matches!(
        revived.sandbox.lifecycle.operation,
        Some(SandboxLifecycleOperation::Revive {
            checkpoint_epoch: Some(7),
            ..
        })
    ));
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn lifecycle_command_reconciles_an_ambiguous_transaction_commit_once() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI")
        .expect("CHRONO_TEST_MONGODB_FAILPOINT_URI must name a failpoint-enabled replica set");
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let app_name = format!("cma-sandbox-lifecycle-{suffix}");
    let database = format!("cma_812_lifecycle_{suffix}");
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(app_name.clone());
    let client = Client::with_options(options).unwrap();
    let store = MongoSandboxStore::open(&client, &database).await.unwrap();
    let active = sandbox("sbx_lifecycle_ambiguous");
    store.create_sandbox(active.clone()).await.unwrap();

    admin
        .database("admin")
        .run_command(doc! {
            "configureFailPoint": "failCommand",
            "mode": { "times": 5_i32 },
            "data": {
                "failCommands": ["commitTransaction"],
                "appName": &app_name,
                "writeConcernError": {
                    "code": 50_i32,
                    "codeName": "MaxTimeMSExpired",
                    "errmsg": "injected ambiguous lifecycle commit",
                },
            },
        })
        .await
        .unwrap();
    let result = store
        .schedule_sandbox_lifecycle(SandboxLifecycleCommand::pause(
            active.id.clone(),
            active.owner.clone(),
            NOW.into(),
            None,
        ))
        .await;
    admin
        .database("admin")
        .run_command(doc! { "configureFailPoint": "failCommand", "mode": "off" })
        .await
        .expect("commit failpoint must be disabled before assertions");

    let SandboxLifecycleCommandResult::Accepted(outcome) = result.unwrap() else {
        panic!("ambiguous commit did not reconcile");
    };
    assert_eq!(
        outcome.disposition,
        SandboxLifecycleCommandDisposition::Scheduled
    );
    assert_eq!(
        store
            .get_sandbox(&active.id)
            .await
            .unwrap()
            .unwrap()
            .revision,
        2
    );
    assert_eq!(
        store.sandbox_events(&active.id, 0, 10).await.unwrap().len(),
        1
    );
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn lifecycle_checkpoint_and_all_agent_phase_commit_atomically() {
    let (client, name) = database("lifecycle_execution").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (claimed, agents) = claimed_pause_fixture(&store, "execution").await;
    let checkpoint = checkpoint_for_agents(&agents, 1);

    store
        .publish_sandbox_checkpoint(checkpoint_mutation(&claimed, &agents, checkpoint.clone()))
        .await
        .unwrap();
    let claimed = store
        .claim_sandbox_work(&claimed.id, "worker", MINUTE.into(), TWO_MINUTES.into())
        .await
        .unwrap()
        .unwrap();
    store
        .commit_sandbox_lifecycle(pause_agents_mutation(&claimed, &agents))
        .await
        .unwrap();

    let stored = store.get_sandbox(&claimed.id).await.unwrap().unwrap();
    assert_eq!(stored.lifecycle.checkpoints.latest, Some(checkpoint));
    assert!(matches!(
        stored.lifecycle.operation,
        Some(SandboxLifecycleOperation::Pause {
            phase: SandboxPausePhase::TeardownRuntime,
            checkpoint_epoch: Some(1),
            ..
        })
    ));
    for original in agents {
        let parked = store.get_agent(&original.id).await.unwrap().unwrap();
        assert_eq!(parked.status, AgentStatus::Paused);
        assert_eq!(parked.revision, original.revision + 1);
        let log = store.agent_log(&parked.id, 0, 10).await.unwrap();
        assert_eq!(log.len(), 1);
        assert!(matches!(
            log[0].kind,
            AgentLogKind::StatusChanged {
                from: AgentStatus::Ready,
                to: AgentStatus::Paused,
            }
        ));
    }
    let events = store.sandbox_events(&stored.id, 0, 20).await.unwrap();
    assert_eq!(events.len(), 5);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn one_stale_agent_aborts_the_entire_lifecycle_phase() {
    let (client, name) = database("lifecycle_atomic_conflict").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (claimed, agents) = claimed_pause_fixture(&store, "atomic").await;
    store
        .publish_sandbox_checkpoint(checkpoint_mutation(
            &claimed,
            &agents,
            checkpoint_for_agents(&agents, 1),
        ))
        .await
        .unwrap();
    let claimed = store
        .claim_sandbox_work(&claimed.id, "worker", MINUTE.into(), TWO_MINUTES.into())
        .await
        .unwrap()
        .unwrap();
    let mut mutation = pause_agents_mutation(&claimed, &agents);
    mutation.agents[1].expected_revision += 1;
    let events_before = store.sandbox_events(&claimed.id, 0, 20).await.unwrap();

    assert_eq!(
        store.commit_sandbox_lifecycle(mutation).await,
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(
        store.get_sandbox(&claimed.id).await.unwrap(),
        Some(claimed.clone())
    );
    for agent in agents {
        assert_eq!(
            store.get_agent(&agent.id).await.unwrap().unwrap().status,
            AgentStatus::Ready
        );
        assert!(store.agent_log(&agent.id, 0, 10).await.unwrap().is_empty());
    }
    assert_eq!(
        store.sandbox_events(&claimed.id, 0, 20).await.unwrap(),
        events_before
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn checkpoint_rejects_agent_and_log_changes_after_capture() {
    let (client, name) = database("checkpoint_capture_guard").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    for change_log in [false, true] {
        let suffix = if change_log { "log" } else { "revision" };
        let (claimed, agents) = claimed_pause_fixture(&store, suffix).await;
        let mutation = checkpoint_mutation(&claimed, &agents, checkpoint_for_agents(&agents, 1));
        if change_log {
            store
                .append_agent_log(
                    &agents[0].id,
                    MINUTE.into(),
                    AgentLogKind::AgentMessage {
                        content: "output completed after capture".into(),
                    },
                )
                .await
                .unwrap();
        } else {
            let mut next = agents[0].clone();
            next.revision += 1;
            next.updated_at = MINUTE.into();
            store.save_agent(agents[0].revision, next).await.unwrap();
        }
        let events = store.sandbox_events(&claimed.id, 0, 20).await.unwrap();
        assert_eq!(
            store.publish_sandbox_checkpoint(mutation).await,
            Err(SandboxStoreError::Conflict)
        );
        assert_eq!(
            store.get_sandbox(&claimed.id).await.unwrap(),
            Some(claimed.clone())
        );
        assert_eq!(
            store.sandbox_events(&claimed.id, 0, 20).await.unwrap(),
            events
        );
        assert_eq!(
            store
                .checkpoints
                .count_documents(doc! { "sandbox_id": claimed.id.as_str() })
                .await
                .unwrap(),
            0
        );
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn checkpoint_refuses_missing_persisted_agent_log_evidence() {
    let (client, name) = database("checkpoint_log_corruption").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (claimed, mut agents) = claimed_pause_fixture(&store, "corrupt").await;
    store
        .append_agent_log(
            &agents[0].id,
            NOW.into(),
            AgentLogKind::AgentMessage {
                content: "output included in capture".into(),
            },
        )
        .await
        .unwrap();
    agents[0] = store.get_agent(&agents[0].id).await.unwrap().unwrap();
    let mutation = checkpoint_mutation(&claimed, &agents, checkpoint_for_agents(&agents, 1));
    store
        .agent_log
        .delete_many(doc! { "agent_id": agents[0].id.as_str() })
        .await
        .unwrap();
    assert!(matches!(
        store.publish_sandbox_checkpoint(mutation).await,
        Err(SandboxStoreError::Corrupt(_))
    ));
    assert_eq!(store.get_sandbox(&claimed.id).await.unwrap(), Some(claimed));
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn lifecycle_phase_reconciles_an_ambiguous_commit_without_duplicate_logs() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI")
        .expect("CHRONO_TEST_MONGODB_FAILPOINT_URI must name a failpoint-enabled replica set");
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let app_name = format!("cma-sandbox-lifecycle-phase-{suffix}");
    let database = format!("cma_812_lifecycle_phase_{suffix}");
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(app_name.clone());
    let client = Client::with_options(options).unwrap();
    let store = MongoSandboxStore::open(&client, &database).await.unwrap();
    let (claimed, agents) = claimed_pause_fixture(&store, "ambiguous_phase").await;
    store
        .publish_sandbox_checkpoint(checkpoint_mutation(
            &claimed,
            &agents,
            checkpoint_for_agents(&agents, 1),
        ))
        .await
        .unwrap();
    let claimed = store
        .claim_sandbox_work(&claimed.id, "worker", MINUTE.into(), TWO_MINUTES.into())
        .await
        .unwrap()
        .unwrap();

    admin
        .database("admin")
        .run_command(doc! {
            "configureFailPoint": "failCommand",
            "mode": { "times": 5_i32 },
            "data": {
                "failCommands": ["commitTransaction"],
                "appName": &app_name,
                "writeConcernError": {
                    "code": 50_i32,
                    "codeName": "MaxTimeMSExpired",
                    "errmsg": "injected ambiguous lifecycle phase commit",
                },
            },
        })
        .await
        .unwrap();
    let result = store
        .commit_sandbox_lifecycle(pause_agents_mutation(&claimed, &agents))
        .await;
    admin
        .database("admin")
        .run_command(doc! { "configureFailPoint": "failCommand", "mode": "off" })
        .await
        .expect("commit failpoint must be disabled before assertions");

    result.unwrap();
    for agent in agents {
        assert_eq!(store.agent_log(&agent.id, 0, 10).await.unwrap().len(), 1);
    }
    assert_eq!(
        store
            .sandbox_events(&claimed.id, 0, 20)
            .await
            .unwrap()
            .len(),
        5
    );
    client.database(&database).drop().await.unwrap();
}
