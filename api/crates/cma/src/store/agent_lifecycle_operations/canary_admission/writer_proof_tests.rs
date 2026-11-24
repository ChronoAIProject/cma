use super::{super::writer_tests as fixtures, *};
use mongodb::{options::ReadConcern, Client};

async fn snapshot(store: &MongoAgentLifecycleStore, id: &str) -> Timestamp {
    let mut session = store
        .operations
        .client()
        .start_session()
        .snapshot(true)
        .await
        .unwrap();
    store
        .operations
        .find_one(doc! { "_id": id })
        .read_concern(ReadConcern::snapshot())
        .session(&mut session)
        .await
        .unwrap();
    session.snapshot_time().unwrap()
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn canary_commit_proof_requires_grant_and_exact_redemption_at_the_observation_time() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let client = Client::with_uri_str(uri).await.unwrap();
    let name = format!("cma_canary_proof_{}", uuid::Uuid::new_v4().simple());
    let database = client.database(&name);
    let store = MongoAgentLifecycleStore::open(&database).await.unwrap();
    let owner = SubjectId::new("canary-owner-fixture");
    let grant_request = fixtures::issue(&owner, fixtures::release('1', "first"), 1, 0);
    CanaryPolicy::configure(
        &database,
        0,
        true,
        grant_request.fresh.as_ref().unwrap().catalog_sha256.clone(),
        grant_request
            .fresh
            .as_ref()
            .unwrap()
            .owner_scope_sha256
            .clone(),
    )
    .await
    .unwrap();
    let grant = store
        .issue_canary_grant(&database, &grant_request)
        .await
        .unwrap();
    let request = fixtures::redeem(&grant_request, &grant, "snapshot-proof");
    let key = format!("{:x}", Sha256::digest(request.key.as_bytes()));
    let id = uuid::Uuid::new_v4().simple().to_string();
    let before = snapshot(&store, &id).await;

    let mut session = client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    let Mutation::Fresh(receipt) = store
        .redeem_once(&database, &request, &id, &key, &mut session)
        .await
        .unwrap()
    else {
        panic!("fixture requires a fresh admission");
    };
    assert!(matches!(
        commit_transaction(&mut session).await,
        CommitOutcome::Committed
    ));
    let committed = snapshot(&store, &id).await;
    assert!(matches!(
        store
            .prove_canary_commit(&database, &request, &receipt, before)
            .await,
        Err(CanaryWriteError::Unresolved)
    ));
    assert!(matches!(
        store.prove_canary_commit(&database, &request, &receipt, committed).await,
        Ok(CreateResult::Created(observed)) if observed == id
    ));

    store
        .operations
        .update_one(
            doc! { "_id": &id },
            doc! { "$unset": { "canary_redemption_key": "" } },
        )
        .await
        .unwrap();
    let changed = snapshot(&store, &id).await;
    assert!(matches!(
        store
            .prove_canary_commit(&database, &request, &receipt, changed)
            .await,
        Err(CanaryWriteError::Unresolved)
    ));
    assert!(matches!(
        store.prove_canary_commit(&database, &request, &receipt, committed).await,
        Ok(CreateResult::Created(observed)) if observed == id
    ));
    database.drop().await.unwrap();
}
