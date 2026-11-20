use super::*;
use mongodb::options::ClientOptions;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn uncertain_capture_publication_reconciles_the_same_catalog_identity_after_restart() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let name = format!("home_backup_unknown_{}", uuid::Uuid::new_v4().simple());
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(name.clone());
    let client = Client::with_options(options).unwrap();
    let db = admin.database(&name);
    crate::store::migrations::home_backup_catalog::prepare_for_test(&db).await;
    let store = MongoHomeBackupStore::open(&client, &name).await.unwrap();
    store.ensure_schedule(&schedule()).await.unwrap();
    let now = layer2_core::now_rfc3339();
    let claim = store
        .claim_due(&now, &holder(), &until())
        .await
        .unwrap()
        .unwrap();
    let intent = backup(&claim);
    admin.database("admin").run_command(doc! {
        "configureFailPoint":"failCommand","mode":"alwaysOn",
        "data":{"failCommands":["commitTransaction"],"appName":&name,
            "writeConcernError":{"code":50,"codeName":"MaxTimeMSExpired","errmsg":"synthetic lost capture acknowledgement"}},
    }).await.unwrap();
    let result = store.plan_capture(&claim, &intent, &now).await;
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .unwrap();
    assert_eq!(result, Err(HomeBackupError::CommitUnknown));
    let reopened = MongoHomeBackupStore::open(&admin, &name).await.unwrap();
    let retained = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(record) = reopened.get_backup(&intent.id).await.unwrap() {
                break record;
            }
            // An uncertain acknowledgement does not establish majority visibility.
            // Reconcile only the original identity; absence cannot authorize replay.
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("the committed capture must become visible to its majority reader");
    let retained_schedule = reopened
        .get_schedule(&claim.schedule.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retained.id, intent.id);
    assert_eq!(retained.epoch, intent.epoch);
    assert_eq!(
        retained_schedule.in_flight.as_ref().unwrap().backup_id,
        intent.id
    );
    assert_eq!(reopened.backups.count_documents(doc! {}).await.unwrap(), 1);
    assert!(matches!(
        reopened.plan_capture(&claim, &intent, &now).await,
        Err(HomeBackupError::Conflict)
    ));
    assert_eq!(
        reopened.get_backup(&intent.id).await.unwrap(),
        Some(retained)
    );
    assert_eq!(
        reopened.get_schedule(&claim.schedule.id).await.unwrap(),
        Some(retained_schedule)
    );
    db.drop().await.unwrap();
}
