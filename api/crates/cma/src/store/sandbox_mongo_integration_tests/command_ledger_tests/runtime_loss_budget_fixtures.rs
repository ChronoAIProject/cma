use super::*;
use crate::store::agent_log_segments::{self as segments, publication, writer_gate as gate};
use crate::store::sandbox_mongo::{commit_transaction, start_transaction, CommitOutcome};
use mongodb::bson::Document;

pub(super) async fn running_with(
    client: &Client,
    name: &str,
    segmented: bool,
    sharded: bool,
    retained: bool,
) -> (Arc<MongoSandboxStore>, Agent, Sandbox, BudgetPolicy) {
    let mut parent = active_sandbox("sbx_commands");
    if retained {
        parent.lifecycle.home = Some(Box::new(
            crate::store::sandbox_unpublished_recovery::mongo_integration_tests::home(&parent.id),
        ));
        let runtime = parent.runtime.as_mut().unwrap();
        runtime.runtime_id = "agent-sandbox:member:sandbox-uid".into();
        runtime.boot_id = Some("b".repeat(32));
        runtime.runtime_version = format!("registry.example/runtime@sha256:{}", "a".repeat(64));
        parent.profile.content.runtime_image = Some(runtime.runtime_version.clone());
        parent.credential.delivered = true;
    }
    let (original, agent, parent, execution, _, sends) = composed_with_history(
        client,
        name,
        Arc::new(FixedSandboxCredentialPrerequisite::Ready),
        parent,
        Some((6, false)),
    )
    .await;
    let store = Arc::new((*original).clone().with_platform_counter_shards(sharded));
    let policy = configure_metered(&store, &agent.owner).await;
    let admitted = record(
        store
            .reserve_command(live_request(&agent, &parent, "runtime-loss"))
            .await
            .unwrap(),
    );
    AgentCommandWorker::new(store.clone(), execution, "budget-recovery-worker".into())
        .unwrap()
        .run_one(&admitted.receipt.envelope.command_id)
        .await
        .unwrap();
    assert_eq!(sends.load(Ordering::SeqCst), 1);
    let mut agent = store.get_agent(&agent.id).await.unwrap().unwrap();
    assert!(agent.conversation.active_turn.is_some());
    if segmented {
        agent = adopt(&store, &agent, AgentLogKind::ConversationGap { frames: 1 }).await;
    }
    (store, agent, parent, policy)
}

pub(in super::super) async fn adopt(
    store: &MongoSandboxStore,
    agent: &Agent,
    kind: AgentLogKind,
) -> Agent {
    let db = store.client.database(&store.agents.namespace().db);
    segments::schema::migrate(&db).await.unwrap();
    crate::store::migrations::agent_log_source_keys::prepare_for_test(&db).await;
    let mut session = store.client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    gate::replace(
        &db,
        gate::GateName::Segments,
        0,
        gate::Policy {
            enabled: true,
            agents: vec![agent.id.to_string()],
            ..Default::default()
        },
        &mut session,
    )
    .await
    .unwrap();
    assert!(matches!(
        commit_transaction(&mut session).await,
        CommitOutcome::Committed
    ));
    store
        .append_agent_log_keyed(
            &agent.id,
            &store::AgentLogPublicationIdentity {
                key: "adopt-budgeted-turn".into(),
                appended_at: layer2_core::now_rfc3339(),
                fingerprint: publication::request_digest(&kind)
                    .unwrap()
                    .bytes
                    .try_into()
                    .unwrap(),
            },
            kind,
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .agents
            .find_one(doc! {"_id":agent.id.as_str()})
            .await
            .unwrap()
            .unwrap()
            .get_i64("document_schema_version")
            .unwrap(),
        2
    );
    store.get_agent(&agent.id).await.unwrap().unwrap()
}

pub(super) async fn turn(store: &MongoSandboxStore, agent: &Agent) -> Document {
    let key = format!(
        "turn:{}:{}",
        agent.id,
        agent.conversation.active_turn.unwrap()
    );
    store
        .model_budget
        .find_one(doc! {"_id":key})
        .await
        .unwrap()
        .unwrap()
        .get_document("record")
        .unwrap()
        .clone()
}

pub(super) async fn settled(store: &MongoSandboxStore, agent: &Agent, before: &Document) {
    let mut expected = before.clone();
    let mut reservation = expected.get_document("reservation").unwrap().clone();
    let amount = reservation.get_i64("reserved_units").unwrap();
    assert!(!reservation.get_bool("settled").unwrap());
    reservation.insert("settled", true);
    expected.insert("reservation", reservation);
    assert_eq!(
        turn(store, agent).await,
        expected,
        "only conservative settlement may change the saved obligation"
    );
    let charged = owner(store, agent).await;
    assert_eq!(charged.reserved, 0);
    assert_eq!(charged.spent, amount as u64);
    let period = before.get_i64("period_start").unwrap();
    let key = match before
        .get("period_shard")
        .and_then(|value| value.as_i64().or_else(|| value.as_i32().map(i64::from)))
    {
        Some(shard) => format!("platform-counter:{period}:{shard}"),
        None => crate::store::budget::period_key(period as u64),
    };
    let counter = store
        .model_budget
        .find_one(doc! {"_id":key})
        .await
        .unwrap()
        .unwrap();
    let record = counter.get_document("record").unwrap();
    assert_eq!(record.get_i64("reserved").unwrap(), 0);
    assert_eq!(record.get_i64("spent").unwrap(), amount);
    assert!(store
        .get_agent(&agent.id)
        .await
        .unwrap()
        .unwrap()
        .conversation
        .active_turn
        .is_none());
}

pub(super) async fn ledger(store: &MongoSandboxStore) -> Vec<Document> {
    store
        .model_budget
        .find(doc! {})
        .sort(doc! {"_id":1})
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap()
}
