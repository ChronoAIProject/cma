use super::*;

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set with test commands"]
async fn cancelling_an_inflight_commit_retains_one_replayable_progress_outcome() {
    let (client, name, store) = database().await;
    let (policy, admission) = fixture(&store).await;
    let state = store
        .admit_pre_exclusion(policy.clone(), admission.clone())
        .await
        .unwrap();
    let input = progress(&admission, state);
    let app = format!("ledger-pre-exclusion-cancellation-{}", uuid::Uuid::new_v4());
    let mut options =
        mongodb::options::ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
            .await
            .unwrap();
    options.app_name = Some(app.clone());
    let worker_client = mongodb::Client::with_options(options).unwrap();
    let worker = MongoSandboxStore::open(&worker_client, &name)
        .await
        .unwrap();
    let configured = client
        .database("admin")
        .run_command(doc! {
            "configureFailPoint":"failCommand", "mode":{"times":1_i32},
            "data":{"failCommands":["commitTransaction"], "appName":app,
                "blockConnection":true, "blockTimeMS":5000_i32},
        })
        .await
        .unwrap();
    let entered = configured
        .get_i64("count")
        .or_else(|_| configured.get_i32("count").map(i64::from))
        .unwrap();
    let original = input.clone();
    let task_policy = policy.clone();
    let task =
        tokio::spawn(async move { worker.progress_pre_exclusion(task_policy, original).await });
    // This barrier proves the server received the committing transaction; the
    // cancellation is not inferred from a sleep or an explicit abort operation.
    let reached = client
        .database("admin")
        .run_command(doc! {
            "waitForFailPoint":"failCommand", "timesEntered":entered+1,
            "maxTimeMS":10000_i32,
        })
        .await;
    task.abort();
    let cancelled = task.await;
    client
        .database("admin")
        .run_command(doc! {
            "configureFailPoint":"failCommand", "mode":"off",
        })
        .await
        .unwrap();
    reached.unwrap();
    assert!(cancelled.unwrap_err().is_cancelled());
    // Cancellation may race a real commit or its abort. Retry the same original
    // command until the old transaction settles; never assume it rolled back.
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    let state = tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            match reopened
                .progress_pre_exclusion(policy.clone(), input.clone())
                .await
            {
                Ok(state) => break state,
                Err(Domain::Unavailable | Domain::PendingCommit) => {
                    tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                }
                Err(error) => panic!("same original progress must resolve: {error}"),
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(state.phase, Phase::Draining);
    assert_eq!(state.record_revision, 2);
    assert_eq!(state.writer_barrier_epoch, 1);
    assert_eq!(
        state.accepted_admission_bytes,
        input.expected.accepted_admission_bytes
    );
    let before = documents(&store).await;
    assert_eq!(before[6].len(), 2);
    for table in before.iter().take(5) {
        assert_eq!(table[0].get_i64("ledger_maintenance_fence_v1").unwrap(), 2);
    }
    assert_eq!(
        reopened
            .progress_pre_exclusion(policy, input)
            .await
            .unwrap(),
        state
    );
    assert_eq!(documents(&store).await, before);
    client.database(&name).drop().await.unwrap();
}
