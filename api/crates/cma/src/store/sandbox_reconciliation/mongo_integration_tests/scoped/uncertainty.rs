use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn root_reconciles_committed_unknown_and_refuses_uncommitted_unknown_without_replanning() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let admin = Client::with_uri_str(&uri).await.unwrap();
    for mode in ["committed", "uncommitted", "transient", "abort"] {
        let suffix = uuid::Uuid::new_v4().simple().to_string();
        let name = format!("cma_scoped_loss_{suffix}");
        let app_name = format!("scoped-loss-{suffix}");
        let mut options = ClientOptions::parse(&uri).await.unwrap();
        options.app_name = Some(app_name.clone());
        let client = Client::with_options(options).unwrap();
        let store = MongoSandboxStore::open(&client, &name).await.unwrap();
        let (sandbox, agent) = adopted(&store, "sbx_uncertain").await;
        let original = row(&store, &agent).await;
        let (times, data) = match mode {
            "committed" => (
                5_i32,
                doc! {
                    "failCommands":["commitTransaction"], "appName":&app_name,
                    "writeConcernError":{"code":50_i32,"codeName":"MaxTimeMSExpired","errmsg":"injected scoped commit uncertainty"}
                },
            ),
            "uncommitted" => (
                5_i32,
                doc! {
                    "failCommands":["commitTransaction"], "appName":&app_name,
                    "errorCode":50_i32,"errorLabels":["UnknownTransactionCommitResult"]
                },
            ),
            "transient" => (
                1_i32,
                doc! {
                    "failCommands":["update"], "appName":&app_name,
                    "errorCode":112_i32,"errorLabels":["TransientTransactionError"]
                },
            ),
            "abort" => (
                1_i32,
                doc! {
                    "failCommands":["insert"], "appName":&app_name,"errorCode":2_i32
                },
            ),
            _ => unreachable!(),
        };
        admin
            .database("admin")
            .run_command(doc! {
                "configureFailPoint":"failCommand", "mode":{"times":times},"data":data
            })
            .await
            .unwrap();
        let outcome = store.recover_runtime_loss(loss(&sandbox)).await;
        disable_failpoint(&admin).await;
        if matches!(mode, "uncommitted" | "abort") {
            assert!(
                matches!(outcome, Err(SandboxStoreError::Unavailable(_))),
                "{mode}: {outcome:?}"
            );
            unchanged(&store, &sandbox, &agent, &original).await;
        } else {
            assert_eq!(
                outcome.unwrap(),
                SandboxRuntimeLossOutcome::RecoveryScheduled,
                "{mode}"
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
            assert_eq!(publication_count(&store).await, 2);
            assert_eq!(
                store
                    .sandbox_events(&sandbox.id, 0, 100)
                    .await
                    .unwrap()
                    .len(),
                2
            );
            assert_eq!(
                store.recover_runtime_loss(loss(&sandbox)).await.unwrap(),
                SandboxRuntimeLossOutcome::Superseded
            );
            assert_eq!(publication_count(&store).await, 2);
        }
        client.database(&name).drop().await.unwrap();
    }
}
