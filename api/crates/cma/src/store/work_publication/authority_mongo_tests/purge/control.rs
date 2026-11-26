use super::*;
use std::future::Future;

fn independent<F, Fut>(task: F)
where
    F: FnOnce(Client) -> Fut + Send + 'static,
    Fut: Future<Output = ()>,
{
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async move {
                tokio::time::timeout(Duration::from_secs(8), async move {
                    let client = Client::with_uri_str(uri).await.unwrap();
                    task(client.clone()).await;
                    client.shutdown().await;
                })
                .await
                .expect("independent test barrier must remain bounded");
            });
    })
    .join()
    .unwrap();
}

pub(super) async fn fail(
    f: &Fixture,
    command: &str,
    times: i32,
    code: i32,
    labels: &[&str],
    committed: bool,
) {
    let mut data = doc! {"appName":&f.app,"failCommands":[command]};
    if committed {
        data.insert(
            "writeConcernError",
            doc! {"code":50_i32,"codeName":"MaxTimeMSExpired",
            "errmsg":"synthetic commit acknowledgment unavailable"},
        );
    } else {
        data.insert("errorCode", code);
        data.insert("errorLabels", labels.to_vec());
        if code == 11000 {
            // Mongo requires DuplicateKey's extra info to emit code 11000.
            // Without it the injected command returns parser error 40671.
            data.insert(
                "errorExtraInfo",
                doc! {"keyPattern":{"_id":1_i32},
                "keyValue":{"_id":"synthetic-duplicate"}},
            );
            data.insert("keyPattern", doc! {"_id":1_i32});
            data.insert("keyValue", doc! {"_id":"synthetic-duplicate"});
        }
    }
    f.admin
        .database("admin")
        .run_command(doc! {
            "configureFailPoint":"failCommand", "mode":{"times":times}, "data":data,
        })
        .await
        .unwrap();
}

pub(super) fn late_abort(p: &PurgeFixture, persistent: bool) {
    let app = p.f.app.clone();
    let collection = p.f.store.suggestions.name().to_owned();
    let mut fired = false;
    *p.f.observer.lock().unwrap() = Some(Box::new(move |event| {
        if !persistent && fired {
            return;
        }
        if !matches!(event, CommandEvent::Started(event)
            if event.command.get_str("delete").ok() == Some(collection.as_str()))
        {
            return;
        }
        fired = true;
        let app = app.clone();
        independent(move |client| async move {
            client
                .database("admin")
                .run_command(doc! {
                    "configureFailPoint":"failCommand", "mode":{"times":1},
                    "data":{"appName":app,"failCommands":["delete"],"errorCode":112_i32,
                        "errorLabels":["TransientTransactionError"]},
                })
                .await
                .unwrap();
        });
    }));
}

pub(super) fn latch(p: &PurgeFixture, transient: bool) {
    let app = p.f.app.clone();
    let mut fired = false;
    *p.f.observer.lock().unwrap() = Some(Box::new(move |event| {
        if fired
            || !matches!(event, CommandEvent::Failed(event)
            if event.command_name == "commitTransaction")
        {
            return;
        }
        fired = true;
        let app = app.clone();
        independent(move |client| async move {
            client.database("admin").run_command(doc! {
                "configureFailPoint":"failCommand", "mode":{"times":1},
                "data":{"appName":app,"failCommands":["commitTransaction"],
                    "errorCode":if transient {112_i32} else {1_i32},
                    "errorLabels":if transient {vec!["TransientTransactionError"]} else {vec![]}},
            }).await.unwrap();
        });
    }));
}

pub(super) fn successor(p: &PurgeFixture, expected: Arc<Mutex<Option<State>>>) {
    let database = p.f.name.clone();
    let root = p.claimed.id.clone();
    let mut fired = false;
    *p.f.observer.lock().unwrap() = Some(Box::new(move |event| {
        if fired
            || !matches!(event, CommandEvent::Failed(event)
            if event.command_name == "update")
        {
            return;
        }
        fired = true;
        let database = database.clone();
        let root = root.clone();
        let expected = expected.clone();
        independent(move |client| async move {
            let store = MongoSandboxStore::open(&client, &database)
                .await
                .unwrap()
                .with_renewable_work(true);
            let mut parent = store.get_sandbox(&root).await.unwrap().unwrap();
            let revision = parent.revision;
            parent.work.lease = None;
            parent.revision += 1;
            store.save_sandbox(revision, parent).await.unwrap();
            store
                .claim_sandbox_work(&root, "successor", now(), work_lease_deadline())
                .await
                .unwrap()
                .unwrap();
            *expected.lock().unwrap() = Some(fixture::state(&store).await);
        });
    }));
}

/// The committed-unknown case must first prove majority visibility. A write
/// concern error alone does not establish that the durable state is readable.
pub(super) fn committed_visibility(p: &PurgeFixture) {
    let database = p.f.name.clone();
    let root = p.claimed.id.to_string();
    let mut replies = 0;
    *p.f.observer.lock().unwrap() =
        Some(Box::new(move |event| {
            let CommandEvent::Succeeded(event) = event else {
                return;
            };
            if event.command_name != "commitTransaction" {
                return;
            }
            replies += 1;
            if replies != 5 {
                return;
            }
            let at = event
                .reply
                .get_timestamp("operationTime")
                .expect("commit reply operation time");
            let database = database.clone();
            let root = root.clone();
            independent(move |client| async move {
                let reply = client.database(&database).run_command(doc! {
                "find":"sandboxes","filter":{"_id":root},"singleBatch":true,
                "readConcern":{"level":"majority","afterClusterTime":at},"maxTimeMS":5000_i32,
            }).await.unwrap();
                assert!(
                    reply
                        .get_document("cursor")
                        .unwrap()
                        .get_array("firstBatch")
                        .unwrap()
                        .is_empty(),
                    "committed-unknown fixture must prove the deletion is majority-visible"
                );
            });
        }));
}
