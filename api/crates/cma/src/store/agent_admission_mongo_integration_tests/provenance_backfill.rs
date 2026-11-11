use super::*;
use crate::store::agent_provenance_backfill::apply;
use futures_util::TryStreamExt;
use layer2_core::sandbox::store::{
    AgentProvenanceBackfillStore, AgentProvenanceStore, PageRequest, ProvenanceBackfillOutcome,
};

async fn legacy_fixture(
    suffix: &str,
) -> (
    Client,
    String,
    MongoSandboxStore,
    Sandbox,
    Workspace,
    AgentAdmission,
) {
    let (client, database, store, root, workspace, mut request) = fixture(suffix).await;
    request.creation_key = format!("manual:{}", request.agent_id);
    store.admit_agent(request.clone()).await.unwrap();
    store
        .agent_creation_provenance
        .delete_one(doc! {"_id":request.agent_id.as_str()})
        .await
        .unwrap();
    (client, database, store, root, workspace, request)
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn provenance_backfill_concurrent_retries_preserve_every_retained_record() {
    let (client, database, store, _, _, request) = legacy_fixture("backfill_concurrent").await;
    let id = &request.agent_id;
    let before = store.agent_provenance_snapshot(id).await.unwrap();
    let events: Vec<_> = store
        .sandbox_events
        .find(doc! {})
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    let (left, right) = tokio::join!(
        store.backfill_agent_provenance(id),
        store.backfill_agent_provenance(id)
    );
    let results = [left.unwrap(), right.unwrap()];
    assert_eq!(
        results
            .iter()
            .filter(|r| **r == ProvenanceBackfillOutcome::Recorded)
            .count(),
        1
    );
    assert_eq!(
        store
            .agent_creation_provenance
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    let after = store.agent_provenance_snapshot(id).await.unwrap();
    assert_eq!(after.resource, before.resource);
    assert_eq!(after.first_log, before.first_log);
    let facts = after.creation.unwrap();
    assert_eq!(
        facts.creation_origin,
        layer2_core::sandbox::AgentCreationOrigin::Manual
    );
    assert!(facts.created_by.is_none());
    assert_eq!(
        store
            .sandbox_events
            .find(doc! {})
            .await
            .unwrap()
            .try_collect::<Vec<_>>()
            .await
            .unwrap(),
        events
    );
    let reopened = MongoSandboxStore::open(&client, &database).await.unwrap();
    assert_eq!(
        reopened.backfill_agent_provenance(id).await.unwrap(),
        ProvenanceBackfillOutcome::Retained
    );
    assert_eq!(
        reopened
            .agent_provenance_snapshot(id)
            .await
            .unwrap()
            .creation
            .unwrap(),
        facts
    );
    assert_eq!(
        reopened
            .scan_agent_provenance_backfill(&PageRequest::new(1, None).unwrap())
            .await
            .unwrap()
            .items,
        vec![id.clone()]
    );
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn provenance_backfill_abort_and_concurrent_agent_update_leave_no_stale_facts() {
    let (client, database, store, _, _, request) = legacy_fixture("backfill_abort").await;
    let id = &request.agent_id;
    let before = store.agent_provenance_snapshot(id).await.unwrap();
    let mut session = store.client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    assert!(apply(&store, id, &mut session).await.is_ok());
    session.abort_transaction().await.unwrap();
    assert_eq!(store.agent_provenance_snapshot(id).await.unwrap(), before);
    start_transaction(&mut session).await.unwrap();
    crate::store::agent_provenance::read_snapshot(&store, id, &mut session)
        .await
        .unwrap_or_else(|_| panic!("valid snapshot"));
    let mut changed = store.get_agent(id).await.unwrap().unwrap();
    let revision = changed.revision;
    changed.revision += 1;
    changed.updated_at = "2026-09-05T00:01:00Z".into();
    store.save_agent(revision, changed.clone()).await.unwrap();
    assert!(
        matches!(apply(&store,id,&mut session).await, Err(crate::store::resource_access::ReadError::Driver(error)) if crate::store::sandbox_mongo::is_transient(&error))
    );
    session.abort_transaction().await.unwrap();
    assert_eq!(
        store
            .agent_creation_provenance
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    assert_eq!(store.get_agent(id).await.unwrap(), Some(changed.clone()));
    assert_eq!(
        store.backfill_agent_provenance(id).await.unwrap(),
        ProvenanceBackfillOutcome::Recorded
    );
    assert_eq!(store.get_agent(id).await.unwrap(), Some(changed));
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn provenance_backfill_corruption_does_not_block_scanning_later_agents() {
    let (client, database, store, _, workspace, request) = legacy_fixture("backfill_corrupt").await;
    let mut next = admission(&workspace, "zz_later");
    next.agent_id = AgentId("zz_later".into());
    next.creation_key = "unproven-import".into();
    store.admit_agent(next.clone()).await.unwrap();
    store
        .agent_creation_provenance
        .delete_one(doc! {"_id":next.agent_id.as_str()})
        .await
        .unwrap();
    store
        .agent_log
        .update_one(
            doc! {"agent_id":request.agent_id.as_str(),"seq":1_i64},
            doc! {"$set":{"record.appended_at":"2026-09-06T00:00:00Z"}},
        )
        .await
        .unwrap();
    let first = store
        .scan_agent_provenance_backfill(&PageRequest::new(1, None).unwrap())
        .await
        .unwrap();
    assert_eq!(first.items, vec![request.agent_id.clone()]);
    assert!(matches!(
        store.backfill_agent_provenance(&request.agent_id).await,
        Err(SandboxStoreError::Corrupt(_))
    ));
    let later = store
        .scan_agent_provenance_backfill(&PageRequest::new(1, first.next).unwrap())
        .await
        .unwrap();
    assert_eq!(later.items, vec![next.agent_id.clone()]);
    assert!(later.next.is_none());
    assert_eq!(
        store
            .backfill_agent_provenance(&next.agent_id)
            .await
            .unwrap(),
        ProvenanceBackfillOutcome::Recorded
    );
    assert_eq!(
        store
            .agent_provenance_snapshot(&next.agent_id)
            .await
            .unwrap()
            .creation
            .unwrap()
            .creation_origin,
        layer2_core::sandbox::AgentCreationOrigin::LegacyUnknown
    );
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn provenance_backfill_recovers_an_ambiguous_commit_without_rewriting_facts() {
    let (admin, database, _, _, _, request) = legacy_fixture("backfill_ambiguous").await;
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let app_name = format!("provenance-backfill-{}", uuid::Uuid::new_v4());
    let mut options = ClientOptions::parse(uri).await.unwrap();
    options.app_name = Some(app_name.clone());
    let client = Client::with_options(options).unwrap();
    let store = MongoSandboxStore::open(&client, &database).await.unwrap();
    let before = store
        .agent_provenance_snapshot(&request.agent_id)
        .await
        .unwrap();
    admin.database("admin").run_command(doc!{"configureFailPoint":"failCommand","mode":{"times":5_i32},"data":{"failCommands":["commitTransaction"],"appName":&app_name,"writeConcernError":{"code":50_i32,"codeName":"MaxTimeMSExpired","errmsg":"injected ambiguous backfill commit"}}}).await.unwrap();
    let outcome = store.backfill_agent_provenance(&request.agent_id).await;
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .unwrap();
    assert_eq!(outcome.unwrap(), ProvenanceBackfillOutcome::Retained);
    assert_eq!(
        store
            .agent_creation_provenance
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    let after = store
        .agent_provenance_snapshot(&request.agent_id)
        .await
        .unwrap();
    assert_eq!(after.resource, before.resource);
    assert_eq!(after.first_log, before.first_log);
    assert_eq!(
        after.creation.unwrap(),
        before.creation_provenance(&request.agent_id).unwrap()
    );
    client.database(&database).drop().await.unwrap();
}

#[path = "provenance_backfill/progress_tests.rs"]
mod progress_tests;
