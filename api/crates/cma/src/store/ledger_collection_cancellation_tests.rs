use super::*;
use std::time::Duration;

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set with test failpoints"]
async fn bounded_collection_timeout_and_caller_cancellation_remain_read_only() {
    let (admin, name) = database("collection_cancel").await;
    for cancel_caller in [false, true] {
        let db = admin.database(&format!("{name}_{cancel_caller}"));
        let documents = collection_documents();
        install(&db, &documents).await;
        indexes(&db).await;
        let app = format!("collection-cancel-{}", uuid::Uuid::new_v4());
        let (client, trace) = traced_client_named(Some(app.clone())).await;
        client
            .database("admin")
            .run_command(doc! {"ping":1})
            .await
            .unwrap();
        trace.lock().unwrap().clear();
        let configured = admin
            .database("admin")
            .run_command(doc! {
                "configureFailPoint":"failCommand","mode":{"times":1},
                "data":{"failCommands":["find"],"appName":app,
                "blockConnection":true,"blockTimeMS":1_500}
            })
            .await
            .unwrap();
        let count = configured
            .get_i64("count")
            .or_else(|_| configured.get_i32("count").map(i64::from))
            .unwrap();
        let peer = client.clone();
        let peer_db = client.database(db.name());
        let request = request(&documents);
        let pending = tokio::spawn(async move {
            crate::store::ledger_collection::observe(&peer, &peer_db, &request).await
        });
        admin
            .database("admin")
            .run_command(doc! {
                "waitForFailPoint":"failCommand","timesEntered":count+1,"maxTimeMS":5_000
            })
            .await
            .unwrap();
        if cancel_caller {
            pending.abort();
            assert!(pending.await.unwrap_err().is_cancelled());
        } else {
            // The real Mongo find is confirmed blocked first. Advance only the
            // client clock so this tests the five-second observer bound without
            // relying on machine scheduling or a server-side maxTimeMS error.
            tokio::time::pause();
            tokio::time::advance(Duration::from_secs(5)).await;
            tokio::task::yield_now().await;
            let finished = pending.is_finished();
            tokio::time::resume();
            assert!(finished, "observer did not enforce its own deadline");
            assert_eq!(
                pending.await.unwrap().unwrap_err(),
                CollectionReadRefusal::AuthorityUnavailable
            );
        }
        admin
            .database("admin")
            .run_command(doc! {
                "configureFailPoint":"failCommand","mode":"off"
            })
            .await
            .unwrap();
        for (collection, document) in &documents {
            assert_eq!(
                db.collection::<Document>(collection)
                    .find_one(doc! {"_id":document.get_str("_id").unwrap()})
                    .await
                    .unwrap()
                    .unwrap(),
                *document
            );
        }
        // A later independent read still succeeds; no partial observation or
        // marker is accepted as completion after the interrupted attempt.
        let request = super::request(&documents);
        assert!(crate::store::ledger_collection::observe(
            &client,
            &client.database(db.name()),
            &request
        )
        .await
        .unwrap()
        .validate_for(&request));
        let commands = trace.lock().unwrap().clone();
        assert!(commands.iter().all(|(name, _)| matches!(
            name.as_str(),
            "listIndexes" | "find" | "getMore" | "abortTransaction" | "killCursors"
        )));
        let find = &commands.iter().find(|(name, _)| name == "find").unwrap().1;
        assert!(command_uses_snapshot_read(find));
        assert!(find.get_str("hint").is_ok());
        db.drop().await.unwrap();
    }
}
