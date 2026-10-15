use super::*;

use layer2_core::sandbox::store::{AgentLogStore, AgentStore};
use layer2_core::sandbox::{SandboxLifecycleOperation, SandboxRevivePhase};
use mongodb::{bson::doc, options::ClientOptions, Client};

use super::lifecycle_queue_tests::{begin_queue_phase, ready_agent};
use crate::store::MongoSandboxStore;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn lifecycle_queue_write_conflicts_release_the_original_lease_and_settle_once() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    // Five conflicts exhaust the queue transaction; fifteen also exhaust its
    // first retry publication and the outer worker's first lease-release try.
    for failures in [1_i32, 5, 15] {
        let suffix = uuid::Uuid::new_v4().simple().to_string();
        let app = format!("cma-lifecycle-contention-{suffix}");
        let database = format!("cma_lifecycle_contention_{suffix}");
        let admin = Client::with_uri_str(&uri).await.unwrap();
        let mut options = ClientOptions::parse(&uri).await.unwrap();
        options.app_name = Some(app.clone());
        let client = Client::with_options(options).unwrap();
        let store = Arc::new(MongoSandboxStore::open(&client, &database).await.unwrap());
        let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
        let created = harness.create().await;
        let now = start_time(&created);
        harness.drive_active(&created.id, now).await;
        let agent = ready_agent(&created, now);
        harness.store.create_agent(agent.clone()).await.unwrap();
        let queue_at = now + Duration::from_secs(1);
        begin_queue_phase(&harness, &created.id, queue_at).await;
        // Copy the valid phase fixture as this disposable store's first row;
        // create_sandbox requires initial persistence revision one.
        let mut queue = harness.stored(&created.id).await;
        queue.revision = 1;
        store.create_sandbox(queue).await.unwrap();
        store.create_agent(agent.clone()).await.unwrap();
        let mut config =
            SandboxWorkerConfig::new("worker-a", "test", runtime_image(), harness.policy.clone());
        config.retry_delay = Duration::from_secs(30);
        let lease_duration = config.lease_duration;
        assert!(lease_duration >= Duration::from_secs(15 * 60));
        let worker = SandboxWorker::new(
            SandboxWorkerDependencies {
                store: store.clone(),
                adapter: harness.adapter.clone(),
                credentials: harness.credentials.clone(),
                workspaces: Arc::new(WorkspaceRecorder {
                    actions: harness.actions.clone(),
                }),
                agents: Arc::new(AgentStopRecorder {
                    actions: harness.actions.clone(),
                }),
                registry: harness.registry.clone(),
                updates: DurableAgentUpdateRecorder::new(store.clone()),
                operations: AgentOperationService::new(store.clone(), harness.registry.clone()),
            },
            config,
        );
        let claimed = store
            .claim_sandbox_work(
                &created.id,
                "worker-a",
                rfc3339_at(queue_at),
                rfc3339_at(queue_at + lease_duration),
            )
            .await
            .unwrap()
            .unwrap();
        let requests = harness.backend.requests().await;
        admin
            .database("admin")
            .run_command(doc! {
                "configureFailPoint": "failCommand", "mode": { "times": failures },
                "data": { "failCommands": ["update"], "appName": &app,
                    "errorCode": 112_i32, "errorLabels": ["TransientTransactionError"] },
            })
            .await
            .unwrap();
        let result = worker.run_claimed_for_test(claimed.clone(), queue_at).await;
        admin
            .database("admin")
            .run_command(doc! { "configureFailPoint": "failCommand", "mode": "off" })
            .await
            .expect("disable owned failpoint before assertions");
        assert!(result.unwrap());
        let stored = store.get_sandbox(&created.id).await.unwrap().unwrap();
        assert!(stored.work.lease.is_none());
        let settle_at = if failures >= 5 {
            assert_eq!(stored.lifecycle.operation, claimed.lifecycle.operation);
            assert_eq!(stored.runtime, claimed.runtime);
            assert_eq!(stored.credential, claimed.credential);
            assert_eq!(stored.work.attempts, 1);
            assert_eq!(stored.work.next_fence, claimed.work.next_fence);
            assert_eq!(
                store.get_agent(&agent.id).await.unwrap(),
                Some(agent.clone())
            );
            let retry_at = SystemTime::UNIX_EPOCH
                + Duration::from_secs(seconds_from_rfc3339(&stored.work.ready_at).unwrap());
            let delay = if failures == 5 { 5 } else { 30 };
            assert!(retry_at >= queue_at + Duration::from_secs(delay));
            assert!(retry_at <= queue_at + Duration::from_secs(delay + 3));
            assert!(!worker.run_one(&created.id, queue_at).await.unwrap());
            assert!(worker.run_one(&created.id, retry_at).await.unwrap());
            retry_at
        } else {
            queue_at
        };
        let settled = store.get_sandbox(&created.id).await.unwrap().unwrap();
        assert!(matches!(
            settled.lifecycle.operation,
            Some(SandboxLifecycleOperation::Revive {
                phase: SandboxRevivePhase::PublishActive,
                ..
            })
        ));
        assert!(worker.run_one(&created.id, settle_at).await.unwrap());
        let active = store.get_sandbox(&created.id).await.unwrap().unwrap();
        assert_eq!(active.status, SandboxStatus::Active);
        assert_eq!(active.runtime, claimed.runtime);
        assert!(active.work.lease.is_none());
        let events = store.sandbox_events(&created.id, 0, 50).await.unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event.kind, SandboxEventKind::Revived))
                .count(),
            1
        );
        let retained = store.get_agent(&agent.id).await.unwrap().unwrap();
        assert_eq!(retained.conversation, agent.conversation);
        assert!(store.agent_log(&agent.id, 0, 20).await.unwrap().is_empty());
        assert_eq!(harness.backend.requests().await, requests);
        client.database(&database).drop().await.unwrap();
    }
}
