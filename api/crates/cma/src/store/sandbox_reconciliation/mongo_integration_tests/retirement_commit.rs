use super::retirement::finality_loss;
use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn retirement_finality_reconciles_commit_uncertainty_without_duplicate_history() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let admin = Client::with_uri_str(&uri).await.unwrap();
    for segmented in [false, true] {
        for mode in ["committed", "uncommitted", "transient", "abort"] {
            let suffix = uuid::Uuid::new_v4().simple().to_string();
            let name = format!("retirement_commit_{suffix}");
            let app_name = format!("retirement-{suffix}");
            let mut options = ClientOptions::parse(&uri).await.unwrap();
            options.app_name = Some(app_name.clone());
            let client = Client::with_options(options).unwrap();
            let store = MongoSandboxStore::open(&client, &name).await.unwrap();
            let (sandbox, children) = retirement::retiring(&store, segmented).await;
            let child = &children[0];
            let history = store.agent_log(&child.id, 0, 100).await.unwrap();
            let (times, data) = match mode {
                "committed" => (
                    5_i32,
                    doc! {"failCommands":["commitTransaction"], "appName":&app_name,
                    "writeConcernError":{"code":50_i32,"codeName":"MaxTimeMSExpired","errmsg":"injected retirement commit uncertainty"}},
                ),
                "uncommitted" => (
                    5_i32,
                    doc! {"failCommands":["commitTransaction"], "appName":&app_name,
                    "errorCode":50_i32,"errorLabels":["UnknownTransactionCommitResult"]},
                ),
                "transient" => (
                    1_i32,
                    doc! {"failCommands":["update"], "appName":&app_name,
                    "errorCode":112_i32,"errorLabels":["TransientTransactionError"]},
                ),
                "abort" => (
                    1_i32,
                    doc! {"failCommands":["insert"], "appName":&app_name,"errorCode":2_i32},
                ),
                _ => unreachable!(),
            };
            admin
                .database("admin")
                .run_command(
                    doc! {"configureFailPoint":"failCommand", "mode":{"times":times},"data":data},
                )
                .await
                .unwrap();
            let outcome = store.recover_runtime_loss(finality_loss(&sandbox)).await;
            disable_failpoint(&admin).await;
            if matches!(mode, "uncommitted" | "abort") {
                assert!(
                    matches!(outcome, Err(SandboxStoreError::Unavailable(_))),
                    "{mode}: {outcome:?}"
                );
                assert_eq!(
                    store.get_sandbox(&sandbox.id).await.unwrap(),
                    Some(sandbox.clone())
                );
                for child in &children {
                    assert_eq!(
                        store.get_agent(&child.id).await.unwrap(),
                        Some(child.clone())
                    );
                }
                assert_eq!(store.agent_log(&child.id, 0, 100).await.unwrap(), history);
                assert_eq!(
                    store
                        .recover_runtime_loss(finality_loss(&sandbox))
                        .await
                        .unwrap(),
                    SandboxRuntimeLossOutcome::RecoveryScheduled
                );
            } else {
                assert_eq!(
                    outcome.unwrap(),
                    SandboxRuntimeLossOutcome::RecoveryScheduled
                );
            }
            let logs = store.agent_log(&child.id, 0, 100).await.unwrap();
            assert_eq!(&logs[..history.len()], history);
            assert_eq!(
                logs.iter()
                    .filter(|l| l.kind == AgentLogKind::TurnInterrupted { turn_sequence: 5 })
                    .count(),
                1
            );
            let restarted = MongoSandboxStore::open(&client, &name).await.unwrap();
            assert_eq!(
                restarted
                    .recover_runtime_loss(finality_loss(&sandbox))
                    .await
                    .unwrap(),
                SandboxRuntimeLossOutcome::Superseded
            );
            assert_eq!(restarted.agent_log(&child.id, 0, 100).await.unwrap(), logs);
            client.database(&name).drop().await.unwrap();
        }
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn retirement_finality_child_write_invalidates_transaction_snapshot() {
    for segmented in [false, true] {
        let (client, name, store) = database().await;
        let (sandbox, children) = retirement::retiring(&store, segmented).await;
        let mut session = client.start_session().await.unwrap();
        let mut scope = TransactionScope::start(&mut session).await.unwrap();
        scope
            .find_one(&store.sandboxes, doc! {"_id":sandbox.id.as_str()})
            .await
            .unwrap()
            .unwrap();
        let mut child = children[1].clone();
        child.revision += 1;
        child
            .work
            .cleanup_debt
            .push(layer2_core::sandbox::AgentCleanupDebt {
                step: AgentWorkStep::DispatchFirstMessage,
                operation_id: "concurrent-external".into(),
                request_sha256: "c".repeat(64),
            });
        store
            .save_agent(children[1].revision, child.clone())
            .await
            .unwrap();
        let attempted =
            transaction::apply_scoped(&store, &finality_loss(&sandbox), None, scope).await;
        assert!(matches!(attempted, Err(ApplyError::Driver(error)) if is_transient(&error)));
        session.abort_transaction().await.unwrap();
        assert_eq!(
            store
                .recover_runtime_loss(finality_loss(&sandbox))
                .await
                .unwrap(),
            SandboxRuntimeLossOutcome::NotEligible
        );
        assert_eq!(store.get_sandbox(&sandbox.id).await.unwrap(), Some(sandbox));
        assert_eq!(store.get_agent(&child.id).await.unwrap(), Some(child));
        client.database(&name).drop().await.unwrap();
    }
}
