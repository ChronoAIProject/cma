use super::*;
use mongodb::options::ClientOptions;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn uncertain_erasure_and_receipt_commits_reconcile_the_original_identity_after_restart() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let name = format!("home_deletion_unknown_{}", uuid::Uuid::new_v4().simple());
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(name.clone());
    let client = Client::with_options(options).unwrap();
    let db = admin.database(&name);
    crate::store::migrations::home_backup_catalog::prepare_for_test(&db).await;
    let store = MongoHomeBackupStore::open(&client, &name).await.unwrap();
    let value = expiry::expired();
    store
        .backups
        .insert_one(backup_document(&value).unwrap())
        .await
        .unwrap();
    let mut parent = schedule();
    parent.state = HomeBackupScheduleState::Erasing;
    store
        .schedules
        .insert_one(schedule_document(&parent).unwrap())
        .await
        .unwrap();
    fail_commit(&admin, &name).await;
    let result = store
        .claim_erasure(&value.id, &layer2_core::now_rfc3339(), &holder(), &until())
        .await;
    disable(&admin).await;
    assert_eq!(result, Err(HomeBackupError::CommitUnknown));
    let reopened = MongoHomeBackupStore::open(&admin, &name).await.unwrap();
    let admitted = reconcile(&reopened, &value.id, HomeBackupState::Expiring).await;
    assert_eq!(admitted.epoch, value.epoch);
    assert_eq!(admitted.storage, value.storage);
    assert_eq!(
        reopened
            .get_schedule(&parent.id)
            .await
            .unwrap()
            .unwrap()
            .revision,
        parent.revision + 1
    );
    assert_eq!(
        reopened
            .claim_erasure(&value.id, &layer2_core::now_rfc3339(), &holder(), &until())
            .await,
        Err(HomeBackupError::Conflict)
    );
    assert_eq!(
        reopened.get_backup(&value.id).await.unwrap(),
        Some(admitted.clone())
    );

    let observation = expiry::proof(&admitted);
    fail_commit(&admin, &name).await;
    let result = store
        .save_deletion(&admitted, &observation, &layer2_core::now_rfc3339())
        .await;
    disable(&admin).await;
    assert_eq!(result, Err(HomeBackupError::CommitUnknown));
    let restarted = MongoHomeBackupStore::open(&admin, &name).await.unwrap();
    let deleted = reconcile(&restarted, &value.id, HomeBackupState::Deleted).await;
    assert_eq!(deleted.epoch, value.epoch);
    assert_eq!(deleted.storage, value.storage);
    assert_eq!(
        deleted.erasure.as_ref().unwrap().receipt,
        observation.receipt
    );
    assert_eq!(
        restarted
            .save_deletion(&admitted, &observation, &layer2_core::now_rfc3339())
            .await,
        Err(HomeBackupError::Conflict)
    );
    assert!(restarted
        .claim_expiring(&layer2_core::now_rfc3339(), &holder(), &until())
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        restarted.get_backup(&value.id).await.unwrap(),
        Some(deleted)
    );
    assert_eq!(restarted.backups.count_documents(doc! {}).await.unwrap(), 1);
    db.drop().await.unwrap();
}

async fn fail_commit(admin: &Client, app: &str) {
    admin.database("admin").run_command(doc! {
        "configureFailPoint":"failCommand", "mode":"alwaysOn",
        "data":{"failCommands":["commitTransaction"], "appName":app,
            "writeConcernError":{"code":50,"codeName":"MaxTimeMSExpired","errmsg":"synthetic lost deletion acknowledgement"}}
    }).await.unwrap();
}

async fn disable(admin: &Client) {
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .unwrap();
}

async fn reconcile(store: &MongoHomeBackupStore, id: &str, state: HomeBackupState) -> HomeBackup {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(row) = store.get_backup(id).await.unwrap() {
                if row.state == state {
                    break row;
                }
            }
            // Neither an absent read nor an uncertain reply permits a new identity.
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("original committed identity must become majority visible")
}
