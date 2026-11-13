use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn runtime_loss_rolls_back_failed_log_writes_and_reconciles_ambiguous_commits_once() {
    for enabled in [false, true] {
        let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
        let suffix = uuid::Uuid::new_v4().simple().to_string();
        let app_name = format!("cma-runtime-loss-{suffix}");
        let name = format!("cma_runtime_loss_{suffix}");
        let admin = Client::with_uri_str(&uri).await.unwrap();
        let mut options = ClientOptions::parse(&uri).await.unwrap();
        options.app_name = Some(app_name.clone());
        let client = Client::with_options(options).unwrap();
        let store = MongoSandboxStore::open(&client, &name)
            .await
            .unwrap()
            .with_runtime_loss_bounded_proof(enabled);
        let (sandbox, agent) = checkpoint_fixture(&store, "sbx_rollback").await;
        admin
            .database("admin")
            .run_command(doc! {
                "configureFailPoint": "failCommand", "mode": { "times": 1_i32 },
                "data": { "failCommands": ["insert"], "appName": &app_name, "errorCode": 2_i32 },
            })
            .await
            .unwrap();
        let failed = store.recover_runtime_loss(loss(&sandbox)).await;
        disable_failpoint(&admin).await;
        assert!(matches!(failed, Err(SandboxStoreError::Unavailable(_))));
        assert_eq!(
            store.get_sandbox(&sandbox.id).await.unwrap(),
            Some(sandbox.clone())
        );
        assert_eq!(
            store.get_agent(&agent.id).await.unwrap(),
            Some(agent.clone())
        );
        assert_eq!(store.agent_log(&agent.id, 0, 100).await.unwrap().len(), 1);
        assert!(store
            .sandbox_events(&sandbox.id, 0, 100)
            .await
            .unwrap()
            .is_empty());
        for mode in ["transient", "failed", "revive"] {
            let (mut sandbox, agent) = if mode == "failed" {
                fixture(&store, "sbx_unknown_failed").await
            } else {
                checkpoint_fixture(&store, &format!("sbx_unknown_{mode}")).await
            };
            if mode == "revive" {
                sandbox.status = SandboxStatus::Paused;
                sandbox.runtime = None;
                sandbox.work.schedule(SandboxWorkStep::None, BEFORE.into());
                let revision = sandbox.revision;
                sandbox
                    .begin_revive(SandboxReviveCause::ExplicitRequest, BEFORE.into())
                    .unwrap();
                sandbox.runtime = Some(RuntimeBinding {
                    boot_id: None,
                    runtime_id: "replacement".into(),
                    runtime_version: "image-v2".into(),
                    provision_generation: Some(2),
                    bound_at: BEFORE.into(),
                });
                sandbox.lifecycle.runtime_generation_high_water = 2;
                store.save_sandbox(revision, sandbox.clone()).await.unwrap();
                let mut parked = agent.clone();
                parked.status = AgentStatus::Paused;
                store
                    .agents
                    .update_one(
                        doc! { "_id": parked.id.as_str() },
                        doc! { "$set": agent_replacement_fields(&parked).unwrap() },
                    )
                    .await
                    .unwrap();
            }
            let (times, data) = if mode == "transient" {
                (
                    1_i32,
                    doc! {
                        "failCommands": ["update"], "appName": &app_name,
                        "errorCode": 112_i32, "errorLabels": ["TransientTransactionError"],
                    },
                )
            } else {
                (
                    5_i32,
                    doc! {
                        "failCommands": ["commitTransaction"], "appName": &app_name,
                        "writeConcernError": { "code": 50_i32, "codeName": "MaxTimeMSExpired", "errmsg": "injected ambiguous runtime-loss commit" },
                    },
                )
            };
            admin
                .database("admin")
                .run_command(doc! {
                    "configureFailPoint": "failCommand", "mode": { "times": times }, "data": data,
                })
                .await
                .unwrap();
            let result = store.recover_runtime_loss(loss(&sandbox)).await;
            disable_failpoint(&admin).await;
            assert_eq!(
                result.unwrap(),
                if mode == "failed" {
                    SandboxRuntimeLossOutcome::Failed
                } else {
                    SandboxRuntimeLossOutcome::RecoveryScheduled
                }
            );
            let recovered = store.get_sandbox(&sandbox.id).await.unwrap().unwrap();
            assert_eq!(recovered.revision, sandbox.revision + 1);
            assert_eq!(recovered.runtime, None);
            let logs = store.agent_log(&agent.id, 0, 100).await.unwrap();
            assert_eq!(
                store.recover_runtime_loss(loss(&sandbox)).await.unwrap(),
                SandboxRuntimeLossOutcome::Superseded
            );
            assert_eq!(store.agent_log(&agent.id, 0, 100).await.unwrap(), logs);
            if mode == "revive" {
                assert!(store
                    .sandbox_events(&sandbox.id, 0, 100)
                    .await
                    .unwrap()
                    .is_empty());
            }
        }
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn runtime_loss_concurrent_identical_observations_publish_one_recovery() {
    let (client, name, store) = database().await;
    let (sandbox, agent) = checkpoint_fixture(&store, "sbx_concurrent_loss").await;
    let results = futures_util::future::join_all((0..8).map(|_| {
        let store = store.clone();
        let request = loss(&sandbox);
        async move { store.recover_runtime_loss(request).await.unwrap() }
    }))
    .await;
    assert_eq!(
        results
            .iter()
            .filter(|outcome| **outcome == SandboxRuntimeLossOutcome::RecoveryScheduled)
            .count(),
        1
    );
    assert_eq!(
        results
            .iter()
            .filter(|outcome| **outcome == SandboxRuntimeLossOutcome::Superseded)
            .count(),
        7
    );
    assert_eq!(
        store.get_agent(&agent.id).await.unwrap().unwrap().revision,
        agent.revision + 1
    );
    assert_eq!(
        store
            .get_sandbox(&sandbox.id)
            .await
            .unwrap()
            .unwrap()
            .revision,
        sandbox.revision + 1
    );
    assert_eq!(
        store
            .sandbox_events(&sandbox.id, 0, 10)
            .await
            .unwrap()
            .len(),
        2
    );
    client.database(&name).drop().await.unwrap();
}
