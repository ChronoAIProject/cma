//! Real conflicting writes establish retry liveness and bounded refusal.
use super::*;
use std::time::Duration;

async fn contended(suffix: &str, release: bool) {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    // Driver retries and aborts can emit several failures for one transaction.
    // Count unique lsid + transaction identities, not individual commands.
    let conflicts = Arc::new(Mutex::new((
        std::collections::BTreeMap::<i32, String>::new(),
        std::collections::BTreeSet::<String>::new(),
    )));
    let observed = conflicts.clone();
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        let mut state = observed.lock().unwrap();
        match event {
            CommandEvent::Started(event)
                if matches!(event.command.get_bool("autocommit"), Ok(false)) =>
            {
                let identity = format!(
                    "{:?}:{:?}",
                    event.command.get("lsid"),
                    event.command.get("txnNumber")
                );
                state.0.insert(event.request_id, identity);
            }
            CommandEvent::Failed(event) => {
                if let Some(identity) = state.0.remove(&event.request_id) {
                    if super::super::super::sandbox_mongo::is_transient(&event.failure) {
                        state.1.insert(identity);
                    }
                }
            }
            CommandEvent::Succeeded(event) => {
                state.0.remove(&event.request_id);
            }
            _ => {}
        }
    }));
    let client = Client::with_options(options).unwrap();
    let database = format!(
        "cma_adm_contention_{suffix}_{}",
        uuid::Uuid::new_v4().simple()
    );
    let store = MongoSandboxStore::open(&client, &database).await.unwrap();
    let parent = sandbox(suffix);
    store.create_sandbox(parent.clone()).await.unwrap();
    let workspace = workspace(&parent, suffix);
    store.create_workspace(workspace.clone()).await.unwrap();
    let admission = admission(&workspace, suffix);
    let mut holder = client.start_session().await.unwrap();
    start_transaction(&mut holder).await.unwrap();
    store
        .workspaces
        .update_one(
            doc! {"_id":workspace.id.as_str()},
            doc! {"$inc":{"owned_contention_fixture":1_i64}},
        )
        .session(&mut holder)
        .await
        .unwrap();
    let contender = store.clone();
    let request = admission.clone();
    let started = tokio::time::Instant::now();
    let task = tokio::spawn(async move { contender.admit_agent(request).await });
    if release {
        tokio::time::timeout(Duration::from_secs(4), async {
            while conflicts.lock().unwrap().1.len() < 6 && !task.is_finished() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        // The original five yield-only attempts stop before this transaction
        // releases. Passing requires surviving at least six actual conflicts.
        holder.abort_transaction().await.unwrap();
    }
    let result = tokio::time::timeout(Duration::from_secs(9), task)
        .await
        .unwrap()
        .unwrap();
    if !release {
        holder.abort_transaction().await.unwrap();
    }
    let attempts = conflicts.lock().unwrap().1.len();
    let elapsed = started.elapsed();
    let count = store
        .agents
        .count_documents(doc! {"sandbox_id":parent.id.as_str()})
        .await
        .unwrap();
    if release {
        let created =
            result.expect("admission must survive the competing transaction and create once");
        let reopened = MongoSandboxStore::open(&client, &database).await.unwrap();
        let replay = reopened.admit_agent(admission).await.unwrap();
        assert_eq!(created.creation().agent.id, replay.creation().agent.id);
        assert_eq!(created.creation().worktree, replay.creation().worktree);
        assert_eq!(count, 1);
        assert!(
            attempts >= 6,
            "causal fixture observed only {attempts} conflicts"
        );
    } else {
        assert!(matches!(result, Err(SandboxStoreError::Unavailable(_))));
        assert_eq!(
            count, 0,
            "retry exhaustion cannot invent committed creation"
        );
        assert!(
            attempts >= 6,
            "contention must receive a meaningful retry budget"
        );
        assert!(elapsed >= Duration::from_secs(4) && elapsed < Duration::from_secs(9));
        let recovered = store.admit_agent(admission.clone()).await.unwrap();
        let replay = store.admit_agent(admission).await.unwrap();
        assert_eq!(recovered.creation().agent.id, replay.creation().agent.id);
    }
    assert_eq!(
        store
            .worktrees
            .count_documents(doc! {"workspace_id":workspace.id.as_str()})
            .await
            .unwrap(),
        1
    );
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn admission_survives_more_than_five_real_write_conflicts() {
    contended("released", true).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn persistent_contention_exhausts_bounded_budget_and_same_key_recovers() {
    contended("persistent", false).await;
}
