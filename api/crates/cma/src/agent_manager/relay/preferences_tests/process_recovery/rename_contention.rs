use super::*;

#[tokio::test]
#[ignore = "requires a disposable loopback Mongo replica set with failpoints enabled"]
async fn non_owner_rename_retries_known_aborted_publication_once() {
    child::require_local_mongo().await;
    let (mongo, name, durable) =
        crate::store::sandbox_maintenance::mongo_integration_tests::database().await;
    let database = mongo.database(&name);
    let admin = mongo.database("admin");
    seed(&durable).await;
    let replicas = vec![
        Replica::open(&name, true).await,
        Replica::open(&name, true).await,
    ];
    let (owner, authority) = fixture::acquire(&replicas).await;
    let peer = 1 - owner;
    let armed = admin
        .run_command(doc! {
            "configureFailPoint": "failCommand", "mode": {"times": 1_i32},
            "data": {"failCommands": ["update"], "namespace": format!("{name}.agents"),
                "errorCode": 112_i32, "errorLabels": ["TransientTransactionError"]},
        })
        .await
        .unwrap();
    let intent = json!({"title": FIRST_TITLE, "expected_revision": 0});
    let primary = AssertUnwindSafe(tokio::time::timeout(Duration::from_secs(15), async {
        let accepted = replicas[peer]
            .request(
                Method::PATCH,
                Some("contention-rename"),
                Some(intent.clone()),
            )
            .await;
        assert_eq!(accepted.0, StatusCode::OK, "{}", accepted.1);
        assert_eq!(accepted.1["title"], FIRST_TITLE);
        assert_eq!(accepted.1["revision"], 1);
        assert_eq!(
            replicas[owner].request(Method::GET, None, None).await,
            accepted
        );
        assert_eq!(
            replicas[owner]
                .request(Method::PATCH, Some("contention-rename"), Some(intent))
                .await,
            accepted
        );
        assert!(replicas[peer].scopes.lock().unwrap().is_empty());
        assert_eq!(
            *replicas[owner].scopes.lock().unwrap(),
            vec![Some(authority.token.clone()); 2]
        );
        let events = title_events(&database).await;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].1, FIRST_TITLE);
    }))
    .catch_unwind()
    .await;
    let disarmed = admin
        .run_command(doc! {"configureFailPoint": "failCommand", "mode": "off"})
        .await;
    for replica in replicas {
        replica.close().await;
    }
    database.drop().await.unwrap();
    let disarmed = disarmed.expect("rename failpoint cleanup failed");
    match primary {
        Ok(Ok(())) => assert_eq!(count(&disarmed), count(&armed) + 1),
        Ok(Err(_)) => panic!("rename exceeded its known-abort retry bound"),
        Err(error) => std::panic::resume_unwind(error),
    }
}

fn count(result: &Document) -> i64 {
    result
        .get_i64("count")
        .or_else(|_| result.get_i32("count").map(i64::from))
        .unwrap()
}
