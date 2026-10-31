use super::*;

use mongodb::bson::doc;
use mongodb::options::ClientOptions;
use mongodb::Client;

use crate::store::MongoSandboxStore;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn first_message_write_conflicts_retry_without_retaining_eighteen_minute_lease() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    // One conflict is absorbed by the transaction loop; forty exhaust all five
    // transaction attempts in each of the eight fresh operation observations.
    // Forty-five also exhausts the first lease-release publication, proving
    // that transient contention there does not strand the 18-minute lease.
    for failures in [1_i32, 40, 45] {
        let suffix = uuid::Uuid::new_v4().simple().to_string();
        let app = format!("cma-first-message-contention-{suffix}");
        let database = format!("cma_first_message_contention_{suffix}");
        let admin = Client::with_uri_str(&uri).await.unwrap();
        let mut options = ClientOptions::parse(&uri).await.unwrap();
        options.app_name = Some(app.clone());
        let client = Client::with_options(options).unwrap();
        let store = Arc::new(MongoSandboxStore::open(&client, &database).await.unwrap());
        let mut harness =
            Harness::with_store(store.clone(), Arc::new(EmptyAgentSkillResolver), 1).await;
        harness.now = SystemTime::now();
        let agent = harness
            .admit_with_first_message("write-conflict", Vec::new(), Some("Run checks once"))
            .await;
        for _ in 0..5 {
            // Real Mongo claims advance ready_at by elapsed IO time. Use a
            // fresh observation for each step, as the production scheduler does.
            harness.run(&agent.id, SystemTime::now()).await;
        }
        let dispatching = harness.stored(&agent.id).await;
        assert_eq!(dispatching.work.step, AgentWorkStep::DispatchFirstMessage);
        let config = AgentWorkerConfig::new("agent-worker", DEPLOYMENT_ID);
        assert_eq!(config.lease_duration, Duration::from_secs(18 * 60));
        harness.worker = AgentWorker::new(
            AgentWorkerDependencies {
                store: store.clone(),
                adapter: harness.adapter.clone(),
                skills: Arc::new(EmptyAgentSkillResolver),
                registry: harness.registry.clone(),
                updates: harness.updates.clone(),
                operations: AgentOperationService::new(store.clone(), harness.registry.clone()),
            },
            config,
        );
        let sends = Arc::new(AtomicUsize::new(0));
        let sent = sends.clone();
        let input = ConversationInputSender::spawn(1, move |_| {
            sent.fetch_add(1, Ordering::SeqCst);
            async { Ok(()) }
        });
        let (_updates, receiver) = mpsc::channel(4);
        let attachment = harness
            .registry
            .attach(
                agent.id.clone(),
                dispatching.conversation.conversation_id.clone().unwrap(),
                1,
                dispatching.work.next_fence,
                RuntimeCapabilities::new([RuntimeCapability::Conversation]),
                input,
                receiver,
                Arc::new(NoopRecorder),
            )
            .unwrap();
        harness.registry.publish_and_activate(&attachment).unwrap();
        let before = SystemTime::now();
        let claimed = store
            .claim_agent_work(
                &agent.id,
                "agent-worker",
                rfc3339_at(before),
                rfc3339_at(before + Duration::from_secs(18 * 60)),
            )
            .await
            .unwrap()
            .unwrap();
        admin
            .database("admin")
            .run_command(doc! {
                "configureFailPoint": "failCommand", "mode": { "times": failures },
                "data": { "failCommands": ["update"], "appName": &app,
                    "errorCode": 112_i32, "errorLabels": ["TransientTransactionError"] },
            })
            .await
            .unwrap();
        let result = harness.worker.dispatch_first_message(claimed, before).await;
        admin
            .database("admin")
            .run_command(doc! { "configureFailPoint": "failCommand", "mode": "off" })
            .await
            .expect("disable owned failpoint before assertions");
        result.unwrap();
        if failures >= 40 {
            let retrying = harness.stored(&agent.id).await;
            assert_eq!(retrying.work.step, AgentWorkStep::DispatchFirstMessage);
            assert!(retrying.work.lease.is_none());
            assert_eq!(retrying.work.attempts, 1);
            assert_eq!(
                retrying.conversation.first_message.as_ref().unwrap().state,
                FirstMessageDelivery::Queued
            );
            assert_eq!(sends.load(Ordering::SeqCst), 0);
            let retry_at = SystemTime::UNIX_EPOCH
                + Duration::from_secs(
                    layer2_core::seconds_from_rfc3339(&retrying.work.ready_at).unwrap(),
                );
            assert!(retry_at <= SystemTime::now() + Duration::from_secs(1));
            assert!(retry_at >= before);
            harness.run(&agent.id, retry_at).await;
        }
        let accepted = harness.stored(&agent.id).await;
        assert_eq!(accepted.work.step, AgentWorkStep::None);
        assert!(accepted.work.lease.is_none());
        assert_eq!(
            accepted.conversation.first_message.as_ref().unwrap().state,
            FirstMessageDelivery::WriteConfirmed
        );
        assert_eq!(
            accepted
                .conversation
                .first_message
                .as_ref()
                .unwrap()
                .attempt,
            1
        );
        assert_eq!(sends.load(Ordering::SeqCst), 1);
        assert_eq!(
            store
                .agent_log(&agent.id, 0, 100)
                .await
                .unwrap()
                .iter()
                .filter(|entry| matches!(entry.kind, AgentLogKind::OwnerMessage { .. }))
                .count(),
            1
        );
        client.database(&database).drop().await.unwrap();
    }
}
