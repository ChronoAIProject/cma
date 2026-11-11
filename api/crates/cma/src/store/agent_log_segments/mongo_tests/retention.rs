use super::*;
use layer2_core::sandbox::{
    store::{SandboxStore, SandboxStoreError, SandboxWorkerStore},
    SandboxStatus, SandboxWorkState, SandboxWorkStep,
};

fn retired(id: &str) -> layer2_core::sandbox::Sandbox {
    let mut sandbox = crate::store::sandbox_mongo_integration_tests::sandbox(id);
    sandbox.status = SandboxStatus::Retired;
    sandbox.credential.delivered = false;
    sandbox.credential.current = None;
    sandbox.work = SandboxWorkState::new(SandboxWorkStep::Purge, NOW.into());
    sandbox.retired_at = Some(NOW.into());
    sandbox.purge_after = Some(NOW.into());
    sandbox
}

async fn contents(client: &Client, database: &str) -> Vec<(String, Vec<Document>)> {
    let db = client.database(database);
    let mut names = db.list_collection_names().await.unwrap();
    names.sort();
    let mut contents = Vec::new();
    for name in names {
        let rows = db
            .collection::<Document>(&name)
            .find(doc! {})
            .sort(doc! {"_id":1})
            .await
            .unwrap()
            .try_collect()
            .await
            .unwrap();
        contents.push((name, rows));
    }
    contents
}

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn retired_sandbox_retains_segmented_and_damaged_history_without_mutation() {
    let (client, name) = database("retention").await;
    schema::migrate(&client.database(&name)).await.unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (agent, _) = segmented(&store, 3).await;
    let sandbox = retired(agent.sandbox_id.as_str());
    store.create_sandbox(sandbox.clone()).await.unwrap();
    for version in [Some(2_i64), Some(3), Some(1), None] {
        let update = match version {
            Some(version) => doc! {"$set":{"document_schema_version":version}},
            None => doc! {"$unset":{"document_schema_version":""}},
        };
        store
            .agents
            .update_one(doc! {"_id":agent.id.as_str()}, update)
            .await
            .unwrap();
        let before = contents(&client, &name).await;
        assert_eq!(
            store
                .purge_sandbox(&sandbox.id, sandbox.revision, NOW)
                .await,
            Err(SandboxStoreError::Conflict)
        );
        assert_eq!(contents(&client, &name).await, before);
    }
    // An apparent downgrade is still held by retained sidecars. Even a lost
    // head cannot turn remaining sealed history into a purgeable legacy log.
    store.agents.update_one(doc! {"_id":agent.id.as_str()}, doc! {
        "$set":{"document_schema_version":1_i64},
        "$unset":{"agent_log_format":"","agent_log_incarnation":"","agent_log_head_revision":""},
    }).await.unwrap();
    for remove_head in [false, true] {
        if remove_head {
            collection(&store, HEADS)
                .delete_many(doc! {})
                .await
                .unwrap();
        }
        let before = contents(&client, &name).await;
        assert_eq!(
            store
                .purge_sandbox(&sandbox.id, sandbox.revision, NOW)
                .await,
            Err(SandboxStoreError::Conflict)
        );
        assert_eq!(contents(&client, &name).await, before);
    }
    // A different retired legacy hierarchy keeps its existing purge policy.
    let legacy = retired("sbx_legacy_retention_fixture");
    store.create_sandbox(legacy.clone()).await.unwrap();
    let mut legacy_agent = agent.clone();
    legacy_agent.id = layer2_core::ids::AgentId("agt_legacy_retention_fixture".into());
    legacy_agent.creation_key = "legacy-retention-fixture".into();
    legacy_agent.sandbox_id = legacy.id.clone();
    legacy_agent.worktree_id = layer2_core::ids::WorktreeId("wkt_legacy_retention_fixture".into());
    legacy_agent.conversation.last_seq = 0;
    store.create_agent(legacy_agent.clone()).await.unwrap();
    store
        .purge_sandbox(&legacy.id, legacy.revision, NOW)
        .await
        .unwrap();
    assert!(store.get_agent(&legacy_agent.id).await.unwrap().is_none());
    assert!(store.get_sandbox(&sandbox.id).await.unwrap().is_some());
    assert!(
        collection(&store, SEGMENTS)
            .count_documents(doc! {})
            .await
            .unwrap()
            > 0
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn exact_admission_retains_legacy_lineage_but_global_policy_does_not() {
    use segments::{
        publication::PublicationIdentity,
        writer_admission::{self, AdmissionKind},
        writer_gate::{self, GateName, Policy},
    };
    let (client, name) = database("admit_hold").await;
    let db = client.database(&name);
    schema::migrate(&db).await.unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let current = agent();
    let sandbox = retired(current.sandbox_id.as_str());
    store.create_sandbox(sandbox.clone()).await.unwrap();
    store.create_agent(current.clone()).await.unwrap();
    let mut session = client.start_session().await.unwrap();
    crate::store::sandbox_mongo::start_transaction(&mut session)
        .await
        .unwrap();
    writer_gate::replace(
        &db,
        GateName::Segments,
        0,
        Policy {
            enabled: true,
            agents: vec![current.id.0.clone()],
            ..Policy::default()
        },
        &mut session,
    )
    .await
    .unwrap();
    session.commit_transaction().await.unwrap();
    assert!(
        !segments::retention::required(&store, sandbox.id.as_str(), &mut session)
            .await
            .unwrap()
    );
    crate::store::sandbox_mongo::start_transaction(&mut session)
        .await
        .unwrap();
    writer_admission::admit(
        &db,
        &current,
        &PublicationIdentity {
            key: "retained-admission-fixture".into(),
            fingerprint: digest::binary(vec![4; 32]),
            appended_at: NOW.into(),
        },
        AdmissionKind::Activation,
        NOW,
        &mut session,
    )
    .await
    .unwrap();
    session.commit_transaction().await.unwrap();
    let before = contents(&client, &name).await;
    assert_eq!(
        store
            .purge_sandbox(&sandbox.id, sandbox.revision, NOW)
            .await,
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(contents(&client, &name).await, before);
    assert!(store.get_agent(&current.id).await.unwrap().is_some());
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn recovery_carrier_retains_legacy_lineage_even_when_the_carrier_is_damaged() {
    let (client, name) = database("recovery_hold").await;
    let db = client.database(&name);
    schema::migrate(&db).await.unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let current = agent();
    let sandbox = retired(current.sandbox_id.as_str());
    store.create_sandbox(sandbox.clone()).await.unwrap();
    store.create_agent(current.clone()).await.unwrap();
    let mut session = client.start_session().await.unwrap();
    assert!(
        !segments::retention::required(&store, sandbox.id.as_str(), &mut session)
            .await
            .unwrap()
    );
    // Deliberately simulate damaged persisted evidence. A validator/codec
    // failure must never turn an accepted-work lineage into purge permission.
    let damaged = doc! {"_id":"damaged-recovery-fixture","agent_id":current.id.as_str(),
    "format_version":2_i64};
    collection(&store, RECOVERY_WORK)
        .insert_one(damaged.clone())
        .bypass_document_validation(true)
        .await
        .unwrap();
    assert!(
        segments::retention::required(&store, sandbox.id.as_str(), &mut session)
            .await
            .unwrap()
    );
    let before = contents(&client, &name).await;
    assert_eq!(
        store
            .purge_sandbox(&sandbox.id, sandbox.revision, NOW)
            .await,
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(contents(&client, &name).await, before);
    assert_eq!(
        collection(&store, RECOVERY_WORK)
            .find_one(doc! {})
            .await
            .unwrap()
            .unwrap(),
        damaged
    );
    assert!(store.get_agent(&current.id).await.unwrap().is_some());
    // The exact Agent join does not hold an unrelated legacy hierarchy.
    let unrelated = retired("sbx_recovery_unrelated_fixture");
    store.create_sandbox(unrelated.clone()).await.unwrap();
    let mut other = current.clone();
    other.id = layer2_core::ids::AgentId("agt_recovery_unrelated_fixture".into());
    other.sandbox_id = unrelated.id.clone();
    other.creation_key = "unrelated-recovery-retention-fixture".into();
    other.worktree_id = layer2_core::ids::WorktreeId("wkt_recovery_unrelated_fixture".into());
    store.create_agent(other.clone()).await.unwrap();
    store
        .purge_sandbox(&unrelated.id, unrelated.revision, NOW)
        .await
        .unwrap();
    assert!(store.get_agent(&other.id).await.unwrap().is_none());
    assert_eq!(
        collection(&store, RECOVERY_WORK)
            .find_one(doc! {})
            .await
            .unwrap()
            .unwrap(),
        damaged
    );
    db.drop().await.unwrap();
}
