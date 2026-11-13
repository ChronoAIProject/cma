//! Actual admission roots on adopted Agents with the durable gate closed.
use super::*;
use crate::store::agent_log_segments::{publication, schema, writer_gate};
use layer2_core::sandbox::store::{AgentLogPublicationIdentity, AgentLogStore};

async fn gate(store: &MongoSandboxStore, id: &AgentId, revision: i64, enabled: bool) {
    let database = store.client.database(&store.agents.namespace().db);
    let mut session = store.client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    if let Err(error) = writer_gate::replace(
        &database,
        writer_gate::GateName::Segments,
        revision,
        writer_gate::Policy {
            enabled,
            agents: if enabled {
                vec![id.to_string()]
            } else {
                vec![]
            },
            ..Default::default()
        },
        &mut session,
    )
    .await
    {
        panic!("gate: {error:?}")
    }
    session.commit_transaction().await.unwrap();
}

async fn append(store: &MongoSandboxStore, id: &AgentId, key: &str) {
    let kind = AgentLogKind::ModeChanged {
        mode: "code".into(),
    };
    let identity = AgentLogPublicationIdentity {
        key: key.into(),
        fingerprint: publication::request_digest(&kind)
            .unwrap()
            .bytes
            .try_into()
            .unwrap(),
        appended_at: NOW.into(),
    };
    store
        .append_agent_log_keyed(id, &identity, kind)
        .await
        .unwrap();
}

async fn adopted(
    suffix: &str,
) -> (
    Client,
    String,
    MongoSandboxStore,
    AgentAdmission,
    layer2_core::sandbox::store::AgentCreation,
) {
    let (client, database, store, _, _, original) = fixture(suffix).await;
    let created = store
        .admit_agent(original.clone())
        .await
        .unwrap()
        .creation()
        .clone();
    schema::migrate(&client.database(&database)).await.unwrap();
    gate(&store, &created.agent.id, 0, true).await;
    append(&store, &created.agent.id, "adopt-before-reuse").await;
    gate(&store, &created.agent.id, 1, false).await;
    let mut request = original;
    request.agent_id = AgentId::generate();
    request.worktree_id = WorktreeId::generate();
    request.creation_key = format!("trigger-reuse-{suffix}");
    request.origin = AgentOrigin::Trigger {
        trigger_id: format!("trg_{suffix}"),
        launch_id: format!("lch_{suffix}"),
    };
    request.first_message = Some(
        layer2_core::sandbox::FirstMessageRecord::new(
            request.creation_key.clone(),
            "Original prompt",
            NOW,
        )
        .unwrap(),
    );
    (client, database, store, request, created)
}

#[tokio::test]
#[ignore = "requires an owned disposable Mongo replica set"]
async fn segmented_reuse_publishes_once_and_replays_after_later_appends() {
    let (client, database, store, request, created) = adopted("segmented_reuse").await;
    let reused = store
        .admit_agent_reusing_profile(request.clone())
        .await
        .unwrap();
    assert_eq!(reused.creation().agent.id, created.agent.id);
    assert_eq!(reused.creation().initial_log, created.initial_log);
    assert_eq!(reused.creation().agent.conversation.last_seq, 3);
    assert_eq!(
        reused
            .creation()
            .agent
            .conversation
            .first_message
            .as_ref()
            .unwrap()
            .text,
        "Original prompt"
    );
    append(&store, &created.agent.id, "later-append").await;
    let reopened = MongoSandboxStore::open(&client, &database).await.unwrap();
    assert_eq!(
        reopened
            .admit_agent_reusing_profile(request.clone())
            .await
            .unwrap()
            .creation()
            .agent
            .conversation
            .last_seq,
        4
    );
    let row = store
        .agents
        .find_one(doc! {"_id":created.agent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.get_i64("document_schema_version").unwrap(), 2);
    assert!(!row.contains_key("agent_log_integrity_v1"));
    let mut foreign = request.clone();
    foreign.first_message.as_mut().unwrap().text = "Changed prompt".into();
    assert!(reopened.admit_agent_reusing_profile(foreign).await.is_err());
    let receipts = client
        .database(&database)
        .collection::<Document>("agent_log_publications");
    let binding = store
        .agent_reuse_bindings
        .find_one(doc! {"_id":crate::store::agent_admission_records::binding_id(&request).unwrap()})
        .await
        .unwrap()
        .unwrap();
    let publication = binding.get_document("log_publication").unwrap();
    receipts
        .delete_one(doc! {"_id":publication.get_str("_id").unwrap()})
        .await
        .unwrap();
    assert!(reopened.admit_agent_reusing_profile(request).await.is_err());
    assert_eq!(
        store
            .agent_log
            .count_documents(doc! {"agent_id":created.agent.id.as_str()})
            .await
            .unwrap(),
        4
    );
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires an owned disposable Mongo replica set"]
async fn segmented_metadata_reuse_preserves_format_and_original_history() {
    let (client, database, store, mut request, created) = adopted("segmented_metadata").await;
    request.first_message = None;
    let before = store.get_agent(&created.agent.id).await.unwrap().unwrap();
    let reused = store
        .admit_agent_reusing_profile(request.clone())
        .await
        .unwrap();
    assert_eq!(reused.creation().agent, before);
    assert_eq!(reused.creation().initial_log, created.initial_log);
    assert_eq!(
        store
            .admit_agent_reusing_profile(request)
            .await
            .unwrap()
            .creation()
            .agent,
        before
    );
    assert_eq!(store.agent_log.count_documents(doc! {}).await.unwrap(), 2);
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires an owned disposable Mongo replica set"]
async fn segmented_reuse_aborts_all_effects_after_late_domain_failure() {
    let (client, database, store, request, created) = adopted("segmented_abort").await;
    let before = store
        .agents
        .find_one(doc! {"_id":created.agent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    store
        .worktrees
        .delete_one(doc! {"_id":created.agent.worktree_id.as_str()})
        .await
        .unwrap();
    assert!(store
        .admit_agent_reusing_profile(request.clone())
        .await
        .is_err());
    assert_eq!(
        store
            .agents
            .find_one(doc! {"_id":created.agent.id.as_str()})
            .await
            .unwrap()
            .unwrap(),
        before
    );
    assert_eq!(
        store
            .agent_reuse_bindings
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    assert_eq!(store.agent_log.count_documents(doc! {}).await.unwrap(), 2);
    assert_eq!(
        client
            .database(&database)
            .collection::<Document>("agent_log_publications")
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    client.database(&database).drop().await.unwrap();
}

#[path = "segmented/bounds.rs"]
mod bounds;

#[path = "segmented/bindings.rs"]
mod bindings;
