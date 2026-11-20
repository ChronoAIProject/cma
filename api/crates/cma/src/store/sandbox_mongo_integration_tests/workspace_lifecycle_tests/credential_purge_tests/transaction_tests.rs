use super::*;
use std::time::{Duration, SystemTime};

async fn protected_parent(f: &mut Fixture) -> Arc<MongoSandboxCredentialStore> {
    f.store = f.store.clone().with_renewable_work(true);
    f.parent = f
        .store
        .claim_sandbox_work(
            &f.parent.id,
            "issuance-owner",
            layer2_core::now_rfc3339(),
            layer2_core::rfc3339_at(SystemTime::now() + Duration::from_secs(60)),
        )
        .await
        .unwrap()
        .unwrap();
    Arc::new(
        MongoSandboxCredentialStore::open(&f.client, &f.name, "purge-contract", &encryption_key())
            .await
            .unwrap(),
    )
}

fn next_intent(f: &Fixture) -> CredentialIssuance {
    let mut intent = f.intent(IssuanceState::Unknown);
    let lease = f.parent.work.lease.as_ref().unwrap();
    intent.generation = f.parent.credential.next_generation().unwrap();
    intent.original_holder = lease.holder.clone();
    intent.original_fence = lease.fence;
    intent
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn issuance_parent_write_conflicts_with_a_purge_snapshot_that_saw_no_intent() {
    let mut f = Fixture::new("purge_snapshot_issuance").await;
    let custody = protected_parent(&mut f).await;
    let mut session = f.client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    let aggregate = f
        .rows("sandboxes")
        .find_one(doc! {"_id":f.parent.id.as_str()})
        .session(&mut session)
        .await
        .unwrap()
        .unwrap();
    assert!(!crate::store::sandbox_credentials::purge::required(
        &f.client.database(&f.name),
        &aggregate,
        &f.parent,
        &mut session,
    )
    .await
    .unwrap());
    assert!(custody
        .begin_issuance(&f.parent, &next_intent(&f))
        .await
        .unwrap());
    let result = f
        .rows("sandboxes")
        .delete_one(doc! {"_id":f.parent.id.as_str()})
        .session(&mut session)
        .await;
    let _ = session.abort_transaction().await;
    assert!(result.as_ref().is_err_and(is_transient), "{result:?}");
    let before = f.snapshot().await;
    assert_eq!(f.purge().await, Err(SandboxStoreError::Conflict));
    assert_eq!(f.snapshot().await, before);
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn committed_purge_prevents_a_late_protected_issuance_owner() {
    let mut f = Fixture::new("purge_before_issuance").await;
    let custody = protected_parent(&mut f).await;
    let intent = next_intent(&f);
    f.purge().await.unwrap();
    assert!(custody.begin_issuance(&f.parent, &intent).await.is_err());
    assert!(f.store.get_sandbox(&f.parent.id).await.unwrap().is_none());
    assert_eq!(
        f.rows("sandbox_credential_issuance")
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a replica set"]
async fn unknown_purge_commit_deletes_only_after_issuance_guard_allows_deletion() {
    let f = Fixture::new("purge_unknown_commit").await;
    f.seed_intent(&f.intent(IssuanceState::Unknown)).await;
    let app = format!("purge-issuance-{}", uuid::Uuid::new_v4());
    let commits = Arc::new(AtomicU64::new(0));
    let observed = commits.clone();
    let mut options =
        ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap())
            .await
            .unwrap();
    options.app_name = Some(app.clone());
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if matches!(event, CommandEvent::Started(event) if event.command_name == "commitTransaction")
        {
            observed.fetch_add(1, Ordering::SeqCst);
        }
    }));
    let client = Client::with_options(options).unwrap();
    let purger = MongoSandboxStore::open(&client, &f.name).await.unwrap();
    f.client
        .database("admin")
        .run_command(doc! {
            "configureFailPoint":"failCommand", "mode":"alwaysOn",
            "data":{"failCommands":["commitTransaction"],"appName":&app,
                "writeConcernError":{"code":50_i32,"codeName":"MaxTimeMSExpired",
                    "errmsg":"fixture lost purge acknowledgement"}}
        })
        .await
        .unwrap();
    let before = f.snapshot().await;
    let blocked = purger
        .purge_sandbox(&f.parent.id, f.parent.revision, NOW)
        .await;
    let blocked_commits = commits.load(Ordering::SeqCst);
    let after = f.snapshot().await;
    f.rows("sandbox_credential_issuance")
        .update_one(doc! {}, doc! {"$set":{"record.state":"accepted"}})
        .await
        .unwrap();
    let completed = purger
        .purge_sandbox(&f.parent.id, f.parent.revision, NOW)
        .await;
    f.client
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand", "mode":"off"})
        .await
        .unwrap();
    assert_eq!(blocked, Err(SandboxStoreError::Conflict));
    assert_eq!(
        blocked_commits, 0,
        "unresolved issuance cannot reach server commit"
    );
    assert_eq!(after, before);
    // Injected write-concern errors allow local commit without waiting for
    // majority visibility, so immediate reconciliation may remain unknown.
    match completed {
        Ok(()) => {}
        Err(SandboxStoreError::Unavailable(message)) => {
            assert!(message.starts_with("sandbox purge commit remained unknown:"));
        }
        result => panic!("unexpected purge result: {result:?}"),
    }
    assert!(commits.load(Ordering::SeqCst) >= 5);
    assert!(f
        .rows("sandboxes")
        .find_one(doc! {"_id":f.parent.id.as_str()})
        .await
        .unwrap()
        .is_none());
    tokio::time::timeout(Duration::from_secs(5), async {
        while f.store.get_sandbox(&f.parent.id).await.unwrap().is_some() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        f.rows("sandbox_credential_issuance")
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    f.cleanup().await;
}
