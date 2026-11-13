use super::*;
use layer2_core::sandbox::store::{AgentConnectionRecoveryCandidate, Page};

mod measurement;

fn worker(store: Arc<MongoSandboxStore>, registry: Arc<LiveAgentRegistry>) -> Arc<AgentWorker> {
    AgentWorker::new(
        AgentWorkerDependencies {
            store: store.clone(),
            adapter: Arc::new(CodexAdapter::with_backend(
                InMemoryHostedBackend::default(),
                CodexSandboxSpec::default(),
            )),
            skills: Arc::new(EmptyAgentSkillResolver),
            registry: registry.clone(),
            updates: DurableAgentUpdateRecorder::new(store.clone()),
            operations: AgentOperationService::new(store, registry),
        },
        AgentWorkerConfig::new("validation-reuse", "validation-reuse"),
    )
}
async fn page(store: &MongoSandboxStore, limit: u16) -> Page<AgentConnectionRecoveryCandidate> {
    store
        .live_agent_connections(&PageRequest::new(limit, None).unwrap())
        .await
        .unwrap()
}
fn counts(store: &MongoSandboxStore) -> (u64, u64) {
    let cache = store.agent_connection_observations.as_ref().unwrap();
    (
        cache.validations.load(Ordering::Relaxed),
        cache.hits.load(Ordering::Relaxed),
    )
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn fresh_bytes_preserve_mutation_errors_lookahead_and_consumed_accounting() {
    let (client, name) = database("validation_reuse").await;
    let baseline = MongoSandboxStore::open(&client, &name).await.unwrap();
    let cached = baseline.clone().with_agent_connection_observations(true);
    let first = idle_connection_fixture(&baseline, "row_a", AgentStatus::Ready).await;
    let second = idle_connection_fixture(&baseline, "row_b", AgentStatus::Ready).await;
    let expected = page(&baseline, 1).await;
    crate::store::mongo_read_telemetry::isolated_test(async {
        let before = crate::store::mongo_read_telemetry::snapshot(AGENTS);
        let left = page(&baseline, 1).await;
        let middle = crate::store::mongo_read_telemetry::snapshot(AGENTS);
        let right = page(&cached, 1).await;
        let after = crate::store::mongo_read_telemetry::snapshot(AGENTS);
        assert_eq!(left.items, right.items);
        assert_eq!(left.next, right.next);
        for n in 0..4 {
            assert_eq!(middle[n] - before[n], after[n] - middle[n]);
        }
    })
    .await;
    assert_eq!(counts(&cached), (2, 0), "hidden limit+1 row is validated");
    assert_eq!(page(&cached, 1).await.items, expected.items);
    assert_eq!(counts(&cached), (2, 2));
    let hidden = if expected.items[0].agent_id == first.id {
        &second
    } else {
        &first
    };
    let filter = doc! {"_id":hidden.id.as_str()};
    // Admission can write this hydration input without advancing Agent revision.
    // A malformed sidecar on the hidden row must still fail the entire page.
    baseline
        .agents
        .update_one(
            filter.clone(),
            doc! {"$set":{"trigger_first_message_binding":{"invalid":true}}},
        )
        .await
        .unwrap();
    let request = PageRequest::new(1, None).unwrap();
    assert!(baseline.live_agent_connections(&request).await.is_err());
    assert!(cached.live_agent_connections(&request).await.is_err());
    assert!(
        cached.live_agent_connections(&request).await.is_err(),
        "errors are never reused"
    );
    baseline
        .agents
        .update_one(
            filter.clone(),
            doc! {"$unset":{"trigger_first_message_binding":""}},
        )
        .await
        .unwrap();
    assert_eq!(page(&cached, 1).await.items, expected.items);
    baseline
        .agents
        .update_one(
            filter.clone(),
            doc! {"$set":{"record.conversation.runtime_generation":9_i64}},
        )
        .await
        .unwrap();
    let current = page(&cached, 10).await;
    assert_eq!(
        current
            .items
            .iter()
            .find(|c| c.agent_id == hidden.id)
            .unwrap()
            .runtime_generation,
        9
    );
    baseline
        .agents
        .update_one(filter.clone(), doc! {"$set":{"revision":999_i64}})
        .await
        .unwrap();
    assert!(
        cached.live_agent_connections(&request).await.is_err(),
        "same-row invalid envelope cannot enter cache"
    );
    baseline.agents.delete_one(filter).await.unwrap();
    assert_eq!(page(&cached, 10).await.items.len(), 1);
    baseline
        .agents
        .insert_one(crate::store::agents::agent_document(hidden).unwrap())
        .await
        .unwrap();
    assert_eq!(
        page(&cached, 10).await.items,
        page(&baseline, 10).await.items
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn cached_candidates_keep_attachment_checks_and_two_replica_recovery_cas() {
    let (client, name) = database("validation_recovery").await;
    let store = Arc::new(
        MongoSandboxStore::open(&client, &name)
            .await
            .unwrap()
            .with_agent_connection_observations(true),
    );
    let agent = idle_connection_fixture(&store, "attachment", AgentStatus::Ready).await;
    let registry = Arc::new(LiveAgentRegistry::default());
    let (sender, receiver) = tokio::sync::mpsc::channel(1);
    let attachment = registry
        .attach(
            agent.id.clone(),
            agent.conversation.conversation_id.clone().unwrap(),
            agent.conversation.runtime_generation.unwrap(),
            agent.work.next_fence - 1,
            RuntimeCapabilities::new([RuntimeCapability::Conversation]),
            ConversationInputSender::spawn(1, |_| async { Ok(()) }),
            receiver,
            Arc::new(IdleRecorder),
        )
        .unwrap();
    registry.publish_and_activate(&attachment).unwrap();
    let attached = worker(store.clone(), registry.clone());
    for _ in 0..2 {
        assert_eq!(
            attached
                .reconcile_connections(SystemTime::now())
                .await
                .unwrap(),
            0
        );
    }
    assert_eq!(counts(&store), (1, 1));
    registry.fence(
        &agent.id,
        agent.conversation.runtime_generation,
        agent.work.next_fence,
    );
    assert_eq!(
        attached
            .reconcile_connections(SystemTime::now())
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        counts(&store),
        (1, 2),
        "attachment loss is checked on the identical cached observation"
    );
    let raced = idle_connection_fixture(&store, "second_replica", AgentStatus::Ready).await;
    let other = Arc::new(
        MongoSandboxStore::open(&client, &name)
            .await
            .unwrap()
            .with_agent_connection_observations(true),
    );
    let other_worker = worker(other.clone(), Arc::new(LiveAgentRegistry::default()));
    let (a, b) = tokio::join!(
        attached.reconcile_connections(SystemTime::now()),
        other_worker.reconcile_connections(SystemTime::now())
    );
    assert_eq!(
        a.unwrap() + b.unwrap(),
        1,
        "each replica still uses the durable CAS"
    );
    let current = store.get_agent(&agent.id).await.unwrap().unwrap();
    assert_eq!(current.revision, agent.revision + 1);
    assert_eq!(current.work.step, AgentWorkStep::PublishConnection);
    let recovered = store.get_agent(&raced.id).await.unwrap().unwrap();
    assert_eq!(recovered.revision, raced.revision + 1);
    assert_eq!(recovered.work.step, AgentWorkStep::PublishConnection);
    assert_eq!(
        counts(&other).0,
        2,
        "new replica fully validates its first observation"
    );
    drop(sender);
    client.database(&name).drop().await.unwrap();
}
