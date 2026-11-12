use super::contracts::{proposal, seeded};
use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn repeated_failure_conflict_cache_rechecks_changed_revision_and_integrity() {
    for changed in ["proof", "revision", "quarantine"] {
        let f = seeded().await;
        let marker = f
            .store
            .agent_log_entry(&f.agent.id, f.agent.conversation.last_seq)
            .await
            .unwrap()
            .unwrap();
        let retry = proposal(&f.agent, &marker, TWO_MINUTES, "2026-09-05T00:03:00Z");
        let admin =
            Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap())
                .await
                .unwrap();
        // Block the known-aborted update after history verification. The peer
        // changes the next snapshot while that exact command remains in flight.
        admin
            .database("admin")
            .run_command(
                doc! {"configureFailPoint": "failCommand", "mode": {"times": 1},
                "data": {"failCommands": ["update"], "appName": &f.app,
                    "blockConnection": true, "blockTimeMS": 500, "errorCode": 112,
                    "errorLabels": ["TransientTransactionError"]}},
            )
            .await
            .unwrap();
        let before_updates = f.updates.load(Ordering::SeqCst);
        scans::isolated_test(async {
            let (result, ()) = tokio::join!(
                f.store.commit_agent_work_repeated_failure(retry.clone(), None),
                async {
                    tokio::time::timeout(Duration::from_secs(5), async {
                        while f.updates.load(Ordering::SeqCst) == before_updates {
                            tokio::time::sleep(Duration::from_millis(1)).await;
                        }
                    }).await.unwrap();
                    let update = if changed == "revision" {
                        doc! {"$inc": {"revision": 1_i64, "record.revision": 1_i64, "agent_log_integrity_v1.bound_agent_revision": 1_i64}}
                    } else if changed == "quarantine" {
                        doc! {"$set": {QUARANTINE_FIELD: Bson::Null}}
                    } else {
                        doc! {"$inc": {"agent_log_integrity_v1.bound_agent_revision": -1_i64}}
                    };
                    admin.database(&f.name).collection::<Document>(AGENTS)
                        .update_one(doc! {"_id": f.agent.id.as_str()}, update).await.unwrap();
                }
            );
            if changed == "revision" {
                assert_eq!(result, Err(SandboxStoreError::Conflict));
            } else if changed == "quarantine" {
                assert!(matches!(result, Err(SandboxStoreError::Corrupt(_))));
            } else {
                result.unwrap();
                assert_eq!(f.store.get_agent(&f.agent.id).await.unwrap(), Some(retry.mutation.agent.clone()));
            }
            assert_eq!(scans::snapshot(CALLER, true)[0], if changed == "quarantine" { 1 } else { 2 });
        }).await;
        admin
            .database("admin")
            .run_command(doc! {"configureFailPoint": "failCommand", "mode": "off"})
            .await
            .unwrap();
        f.cleanup().await;
    }
}
