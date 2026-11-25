use super::*;
use crate::store::native_prepared::tests::open_ready_fixture::adoption_policy;
use layer2_core::sandbox::store::AgentLogStore;
use layer2_core::sandbox::store::AGENT_LOG_CAPACITY_FAILURE;
use layer2_core::sandbox::{AgentLogEntry, AgentLogKind, FirstMessageDelivery};
use mongodb::bson::{doc, serialize_to_bson, Document};

mod adoption_races;
mod review_tests;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn prepared_connection_settles_capacity_under_its_new_claim() {
    let fixture = Fixture::new().await;
    assert!(fixture.publish().await.unwrap());
    let mut full = fixture
        .store
        .get_agent(&fixture.agent.id)
        .await
        .unwrap()
        .unwrap();
    let revision = full.revision;
    full.conversation.first_message = Some(
        FirstMessageRecord::new(&full.creation_key, "retained input", rfc3339_at(at())).unwrap(),
    );
    full.revision += 1;
    fixture
        .store
        .save_agent(revision, full.clone())
        .await
        .unwrap();
    fill_count_boundary(&fixture, &full).await;

    finish(&fixture, &fixture.context).await;
    assert_capacity_failure(&fixture, &full).await;
    fixture.cleanup().await;
}

async fn fill_count_boundary(fixture: &Fixture, full: &Agent) {
    let entries: Vec<_> = (full.conversation.last_seq + 1..=10_000)
        .map(|seq| {
            let entry = AgentLogEntry {
                agent_id: full.id.clone(),
                seq,
                appended_at: full.updated_at.clone(),
                kind: AgentLogKind::WorkStepCompleted {
                    step: AgentWorkStep::PublishConnection,
                },
            };
            doc! {"_id":format!("{}:{seq:020}", full.id), "document_schema_version":1_i64,
            "agent_id":full.id.as_str(), "seq":seq as i64, "record":serialize_to_bson(&entry).unwrap()}
        })
        .collect();
    let database = fixture.client.database(&fixture.name);
    database
        .collection::<Document>("agent_log")
        .insert_many(entries)
        .await
        .unwrap();
    let result = database
        .collection::<Document>("agents")
        .update_one(
            doc! {"_id":full.id.as_str(), "revision":full.revision as i64},
            doc! {"$set":{"last_seq":10_000_i64, "record.conversation.last_seq":10_000_i64}},
        )
        .await
        .unwrap();
    assert_eq!(result.matched_count, 1);
}

async fn assert_capacity_failure(fixture: &Fixture, full: &Agent) {
    let failed = fixture.store.get_agent(&full.id).await.unwrap().unwrap();
    assert_eq!(failed.status, AgentStatus::Failed);
    assert_eq!(
        failed.failure.as_ref().unwrap().class,
        AGENT_LOG_CAPACITY_FAILURE
    );
    assert_eq!(failed.work.step, AgentWorkStep::None);
    assert!(failed.work.lease.is_none());
    assert_eq!(failed.work.next_fence, fixture.agent.work.next_fence + 1);
    assert_eq!(failed.work.receipts, full.work.receipts);
    assert_eq!(failed.work.cleanup_debt, full.work.cleanup_debt);
    assert_eq!(
        failed.conversation.conversation_id,
        full.conversation.conversation_id
    );
    assert_eq!(
        failed.conversation.runtime_generation,
        full.conversation.runtime_generation
    );
    assert_eq!(failed.conversation.last_seq, 10_001);
    let first = failed.conversation.first_message.as_ref().unwrap();
    assert_eq!(first.state, FirstMessageDelivery::Failed);
    assert_eq!(
        first.text,
        full.conversation.first_message.as_ref().unwrap().text
    );
    assert_eq!(first.attempt, 0);
    assert_eq!(fixture.sends.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert!(!fixture
        .worker
        .dependencies
        .registry
        .target_is_attached(&target(fixture)));
    let reopened = MongoSandboxStore::open(&fixture.client, &fixture.name)
        .await
        .unwrap();
    assert_eq!(reopened.get_agent(&full.id).await.unwrap(), Some(failed));
}

async fn current(fixture: &Fixture) -> Agent {
    fixture
        .store
        .get_agent(&fixture.agent.id)
        .await
        .unwrap()
        .unwrap()
}

async fn initialize_connection(fixture: &Fixture) -> Agent {
    assert!(fixture.publish().await.unwrap());
    let mut full = current(fixture).await;
    let revision = full.revision;
    full.conversation.first_message = Some(
        FirstMessageRecord::new(&full.creation_key, "retained input", rfc3339_at(at())).unwrap(),
    );
    full.revision += 1;
    fixture
        .store
        .save_agent(revision, full.clone())
        .await
        .unwrap();
    fill_count_boundary(fixture, &full).await;
    current(fixture).await
}

async fn finish_now(fixture: &Fixture) -> Result<(), SandboxStoreError> {
    fixture
        .worker
        .publish_prepared_connection(
            &fixture.agent,
            &fixture.context,
            &fixture.attachment,
            SystemTime::now(),
        )
        .await
}

async fn preserved_prefix(fixture: &Fixture, through: i64) -> Vec<Document> {
    use futures_util::TryStreamExt;
    fixture
        .client
        .database(&fixture.name)
        .collection::<Document>("agent_log")
        .find(doc! {"agent_id": fixture.agent.id.as_str(), "seq": {"$lte": through}})
        .sort(doc! {"seq": 1})
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap()
}

async fn check_publication(fixture: &Fixture, previous: &Agent, batch: i64) -> Agent {
    let through = previous.conversation.last_seq as i64;
    let published = current(fixture).await;
    assert_eq!(published.conversation.last_seq as i64, through + batch);
    assert_eq!(
        published.conversation.conversation_id,
        previous.conversation.conversation_id
    );
    assert_eq!(
        published.conversation.runtime_generation,
        previous.conversation.runtime_generation
    );
    assert_eq!(published.work.receipts, previous.work.receipts);
    assert_eq!(
        published.conversation.first_message,
        previous.conversation.first_message
    );
    assert!(published.work.lease.is_none());
    let database = fixture.client.database(&fixture.name);
    let seal = database
        .collection::<Document>("agent_log_segments")
        .find_one(doc! {})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(seal.get_i64("start_seq").unwrap(), 1);
    assert_eq!(seal.get_i64("end_seq").unwrap(), through);
    let head = database
        .collection::<Document>("agent_log_heads")
        .find_one(doc! {})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(head.get_i64("active_ordinal").unwrap(), 1);
    assert_eq!(head.get_i64("active_start_seq").unwrap(), through + 1);
    assert_eq!(head.get_i64("active_count").unwrap(), batch);
    let publication = database
        .collection::<Document>("agent_log_publications")
        .find_one(doc! {})
        .await
        .unwrap()
        .unwrap();
    let key = publication.get_str("publication_key").unwrap();
    assert!(key.starts_with("pending-worker:v1:"));
    let admission = database
        .collection::<Document>("agent_log_admissions")
        .find_one(doc! {"publication_key": key})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(admission.get_str("kind").unwrap(), "activation");
    assert_eq!(
        admission.get("recovery_gate"),
        Some(&mongodb::bson::Bson::Null)
    );
    assert_eq!(admission.get("fingerprint"), publication.get("fingerprint"));
    assert_eq!(publication.get_i64("first_seq").unwrap(), through + 1);
    assert_eq!(publication.get_i64("last_seq").unwrap(), through + batch);
    let log = preserved_prefix(fixture, through + batch).await;
    let kinds: Vec<_> = log[through as usize..]
        .iter()
        .map(|row| {
            mongodb::bson::deserialize_from_bson::<AgentLogEntry>(
                row.get("record").unwrap().clone(),
            )
            .unwrap()
            .kind
        })
        .collect();
    if batch == 1 {
        assert_eq!(
            kinds,
            [AgentLogKind::WorkStepCompleted {
                step: AgentWorkStep::PublishConnection
            }]
        );
    } else {
        assert_eq!(
            kinds,
            [
                AgentLogKind::ConversationOpened {
                    conversation_id: published.conversation.conversation_id.clone().unwrap(),
                    runtime_generation: published.conversation.runtime_generation.unwrap(),
                },
                AgentLogKind::StatusChanged {
                    from: AgentStatus::Starting,
                    to: AgentStatus::Ready
                },
            ]
        );
    }
    assert_eq!(fixture.sends.load(std::sync::atomic::Ordering::SeqCst), 0);
    let compatible = MongoSandboxStore::open(&fixture.client, &fixture.name)
        .await
        .unwrap();
    assert_eq!(
        compatible.get_agent(&published.id).await.unwrap().as_ref(),
        Some(&published)
    );
    let page = compatible
        .agent_log(&published.id, through as u64 - 1, (batch + 1) as u16)
        .await
        .unwrap();
    assert_eq!(page.first().unwrap().seq, through as u64);
    assert_eq!(page.last().unwrap().seq, published.conversation.last_seq);
    published
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn selected_full_legacy_connection_publishes_the_original_step() {
    let fixture = Fixture::with_adoption(None, true).await;
    let full = initialize_connection(&fixture).await;
    adoption_policy(&fixture.client, &fixture.name, full.id.as_str(), 0, true).await;
    let prefix = preserved_prefix(&fixture, 10_000).await;
    finish_now(&fixture).await.unwrap();
    let done = check_publication(&fixture, &full, 1).await;
    assert_eq!(done.work.step, AgentWorkStep::DispatchFirstMessage);
    assert!(fixture
        .worker
        .dependencies
        .registry
        .published_target_is_attached(&target(&fixture)));
    let transferred = fixture
        .worker
        .dependencies
        .registry
        .attachment(&target(&fixture), done.work.next_fence - 1)
        .unwrap()
        .unwrap();
    assert_eq!(
        transferred.identity().nonce,
        fixture.attachment.identity().nonce
    );
    assert_eq!(preserved_prefix(&fixture, 10_000).await, prefix);
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn selected_full_legacy_ready_publishes_existing_open_receipt() {
    let fixture = Fixture::with_adoption(None, true).await;
    let request = layer3_adapter::AgentOpenRequest::new(
        format!("agent:{}:open:v1", fixture.agent.id),
        crate::sandbox::runtime_options(
            &fixture.agent,
            &fixture.context.sandbox,
            &fixture.context.worktree,
        ),
        None,
    );
    fixture
        .worker
        .retain_completed_open_after_conflict(
            &fixture.agent,
            &fixture.context.sandbox,
            &request,
            &fixture.attachment,
            at(),
        )
        .await
        .unwrap();
    let ready = current(&fixture).await;
    assert_eq!(ready.work.step, AgentWorkStep::PublishReady);
    fill_count_boundary(&fixture, &ready).await;
    adoption_policy(&fixture.client, &fixture.name, ready.id.as_str(), 0, true).await;
    let now = SystemTime::now();
    let claimed = fixture
        .store
        .claim_agent_work(
            &ready.id,
            "open-worker",
            rfc3339_at(now),
            rfc3339_at(now + std::time::Duration::from_secs(300)),
        )
        .await
        .unwrap()
        .unwrap();
    let prefix = preserved_prefix(&fixture, 10_000).await;
    fixture
        .worker
        .publish_ready_once(claimed.clone(), &fixture.context, &target(&fixture), now)
        .await
        .unwrap();
    let done = check_publication(&fixture, &claimed, 2).await;
    assert_eq!(done.status, AgentStatus::Ready);
    assert_eq!(done.work.step, AgentWorkStep::PublishConnection);
    assert_eq!(preserved_prefix(&fixture, 10_000).await, prefix);
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn selected_connection_crosses_the_byte_budget_below_the_row_limit() {
    let fixture = Fixture::with_adoption(None, true).await;
    assert!(fixture.publish().await.unwrap());
    let full = crate::store::fill_agent_capacity_to_fixture(
        &fixture.store,
        current(&fixture).await,
        None,
        Some(64 * 1024 * 1024 - 1),
    )
    .await;
    assert!(full.conversation.last_seq < 10_000);
    adoption_policy(&fixture.client, &fixture.name, full.id.as_str(), 0, true).await;
    finish_now(&fixture).await.unwrap();
    check_publication(&fixture, &full, 1).await;
    fixture.cleanup().await;
}
