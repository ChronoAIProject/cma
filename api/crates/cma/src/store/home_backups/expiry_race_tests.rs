use super::*;
use mongodb::options::ClientOptions;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn erasure_rechecks_parent_fence_and_restore_pins_after_reading_its_snapshot() {
    for change in ["parent_fence", "restore_pin"] {
        let fixture = Fixture::new().await;
        let value = expiry::expired();
        fixture
            .store
            .backups
            .insert_one(backup_document(&value).unwrap())
            .await
            .unwrap();
        let mut parent = schedule();
        parent.state = HomeBackupScheduleState::Erasing;
        fixture
            .store
            .schedules
            .insert_one(schedule_document(&parent).unwrap())
            .await
            .unwrap();
        let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
        let app = format!("erasure-race-{}", uuid::Uuid::new_v4().simple());
        let mut options = ClientOptions::parse(&uri).await.unwrap();
        options.app_name = Some(app.clone());
        let client = Client::with_options(options).unwrap();
        let contender = MongoHomeBackupStore::open(&client, &fixture.database)
            .await
            .unwrap();
        let admin = Client::with_uri_str(&uri).await.unwrap().database("admin");
        let configured = admin
            .run_command(doc! {
                "configureFailPoint":"failCommand", "mode":{"times":1},
                "data":{"failCommands":["update"], "appName":&app,
                    "blockConnection":true, "blockTimeMS":5000}
            })
            .await
            .unwrap();
        let entered = configured
            .get_i64("count")
            .or_else(|_| configured.get_i32("count").map(i64::from))
            .unwrap();
        let id = value.id.clone();
        let claim = tokio::spawn(async move {
            contender
                .claim_erasure(&id, &layer2_core::now_rfc3339(), &holder(), &until())
                .await
        });
        let reached = admin
            .run_command(doc! {
                "waitForFailPoint":"failCommand", "timesEntered":entered+1, "maxTimeMS":5000
            })
            .await;
        let mut pinned = value.clone();
        let mutation = if reached.is_ok() {
            // Mutate authority while the server pauses its first update after both reads.
            Some(
                tokio::time::timeout(Duration::from_secs(2), async {
                    if change == "parent_fence" {
                        parent.next_fence += 1;
                        parent.revision += 1;
                        fixture
                            .store
                            .schedules
                            .replace_one(
                                doc! {"_id":&parent.id},
                                schedule_document(&parent).unwrap(),
                            )
                            .await
                    } else {
                        pinned.restore_pins = 1;
                        pinned.revision += 1;
                        fixture
                            .store
                            .backups
                            .replace_one(doc! {"_id":&pinned.id}, backup_document(&pinned).unwrap())
                            .await
                    }
                })
                .await,
            )
        } else {
            None
        };
        let result = claim.await.unwrap();
        admin
            .run_command(doc! {"configureFailPoint":"failCommand", "mode":"off"})
            .await
            .unwrap();
        reached.expect("admission must reach the server pause");
        assert_eq!(mutation.unwrap().unwrap().unwrap().matched_count, 1);
        assert_eq!(result, Err(HomeBackupError::Conflict), "{change}");
        assert_eq!(
            fixture.store.get_backup(&value.id).await.unwrap(),
            Some(pinned)
        );
        assert_eq!(
            fixture.store.get_schedule(&parent.id).await.unwrap(),
            Some(parent)
        );
        fixture.cleanup().await;
    }
}
