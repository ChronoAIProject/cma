use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn cancelled_or_failed_read_never_becomes_legacy_or_writes_authority() {
    let f = Fixture::new().await;
    f.seed().await;
    let admin = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap())
        .await
        .unwrap();
    f.commands.lock().unwrap().clear();
    admin.database("admin").run_command(doc! {"configureFailPoint":"failCommand","mode":{"times":1},
        "data":{"failCommands":["find"],"appName":&f.app,"blockConnection":true,"blockTimeMS":2000}}).await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(100), require_legacy_compatible(&f.db))
            .await
            .is_err()
    );
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .unwrap();
    require_legacy_compatible(&f.db).await.unwrap();
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand","mode":{"times":2},
        "data":{"failCommands":["find"],"appName":&f.app,"errorCode":13}})
        .await
        .unwrap();
    assert!(matches!(
        require_legacy_compatible(&f.db).await,
        Err(StoreError::Unavailable(_))
    ));
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .unwrap();
    let commands = f.commands.lock().unwrap().clone();
    assert!(commands
        .iter()
        .all(|command| command.contains_key("find") || command.contains_key("killCursors")));
    require_legacy_compatible(&f.db).await.unwrap();
    f.db.drop().await.unwrap();
}
