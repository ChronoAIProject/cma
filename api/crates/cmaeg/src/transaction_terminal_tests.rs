//! A gateway transaction whose commit or abort is cancelled mid-send must not
//! keep its row locks (#3299). Failpoints match this fixture's appName, so the
//! contracts run serially on a disposable replica set with test commands.
use super::{kill_session, GuardedTerminal as _};
use crate::transaction_start::and_run_write;
use futures_util::FutureExt as _;
use mongodb::{
    bson::{doc, Document},
    options::{ClientOptions, Credential},
    Client, ClientSession, Collection,
};
use std::time::{Duration, Instant};

fn fixture_uri() -> String {
    std::env::var("CMA_AUTHENTICATED_TEST_MONGODB_URI").expect("disposable Mongo fixture required")
}

struct Fixture {
    admin: Client,
    client: Client,
    name: String,
}

impl Fixture {
    async fn new() -> Self {
        let uri = fixture_uri();
        let admin = Client::with_uri_str(&uri).await.unwrap();
        let name = format!("cmaeg_terminal_{}", uuid::Uuid::new_v4().simple());
        let mut options = ClientOptions::parse(&uri).await.unwrap();
        options.app_name = Some(name.clone());
        let client = Client::with_options(options).unwrap();
        let fixture = Self {
            admin,
            client,
            name,
        };
        fixture
            .rows(&fixture.admin)
            .insert_one(doc! {"_id": "row", "value": 0_i32})
            .await
            .unwrap();
        fixture
    }

    fn rows(&self, client: &Client) -> Collection<Document> {
        client.database(&self.name).collection("rows")
    }

    /// A transaction holding the row's write lock.
    async fn locked_transaction(&self) -> ClientSession {
        let mut session = self.client.start_session().await.unwrap();
        session.start_transaction().await.unwrap();
        self.rows(&self.client)
            .update_one(doc! {"_id": "row"}, doc! {"$inc": {"value": 1_i32}})
            .session(&mut session)
            .await
            .unwrap();
        session
    }

    async fn next_command(&self, command: &str, data: Document) -> i64 {
        let mut data = data;
        data.insert("appName", self.name.clone());
        data.insert("failCommands", vec![command]);
        let set = self
            .admin
            .database("admin")
            .run_command(doc! {
                "configureFailPoint": "failCommand", "mode": {"times": 1_i32}, "data": data,
            })
            .await
            .unwrap();
        set.get_i64("count")
            .or_else(|_| set.get_i32("count").map(i64::from))
            .unwrap()
    }

    async fn wait_for_command(&self, previous_count: i64) {
        self.admin
            .database("admin")
            .run_command(doc! {
                "waitForFailPoint": "failCommand", "timesEntered": previous_count + 1,
                "maxTimeMS": 5_000,
            })
            .await
            .unwrap();
    }

    async fn cancel_mid_send<F: std::future::Future>(&self, command: &str, terminal: F) {
        let count = self
            .next_command(
                command,
                doc! {
                    "blockConnection": true, "blockTimeMS": 400_i32, "errorCode": 2_i32,
                },
            )
            .await;
        let mut terminal = Box::pin(terminal);
        tokio::select! {
            _ = &mut terminal => panic!("the stalled {command} returned before cancellation"),
            () = self.wait_for_command(count) => {},
        }
        drop(terminal);
        tokio::time::sleep(Duration::from_millis(600)).await;
        self.admin
            .database("admin")
            .run_command(doc! {"configureFailPoint": "failCommand", "mode": "off"})
            .await
            .unwrap();
    }

    /// Another client cannot reuse the cancelled server session, so only a
    /// lock left by its transaction could keep this writer conflicting.
    async fn row_is_free_promptly(&self) {
        let started = Instant::now();
        let mut conflicts = 0;
        loop {
            let mut session = self.admin.start_session().await.unwrap();
            session.start_transaction().await.unwrap();
            let written = self
                .rows(&self.admin)
                .update_one(
                    doc! {"_id": "row"},
                    doc! {"$set": {"writer": "independent"}},
                )
                .session(&mut session)
                .await;
            match written {
                Ok(_) => {
                    session.commit_transaction().await.unwrap();
                    break;
                }
                Err(error) if error.contains_label("TransientTransactionError") => {
                    let _ = session.abort_transaction().await;
                    conflicts += 1;
                }
                Err(error) => panic!("unexpected independent write error: {error}"),
            }
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "a cancelled transaction stranded the row lock ({conflicts} conflicts)"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    async fn value(&self) -> i32 {
        self.rows(&self.admin)
            .find_one(doc! {"_id": "row"})
            .await
            .unwrap()
            .unwrap()
            .get_i32("value")
            .unwrap()
    }

    async fn cleanup(self) {
        self.admin.database(&self.name).drop().await.unwrap();
    }
}

async fn assert_cancelled_start_releases_row(commands: &[&str]) {
    let f = Fixture::new().await;
    let admin = f.admin.database("admin");
    let failpoint = admin
        .run_command(doc! {
            "configureFailPoint": "failCommand",
            "mode": {"times": 1_i32},
            "data": {
                "appName": &f.name,
                "failCommands": commands,
                "blockConnection": true,
                "blockTimeMS": 2_000_i32,
            },
        })
        .await
        .unwrap();
    let count = failpoint
        .get_i64("count")
        .or_else(|_| failpoint.get_i32("count").map(i64::from))
        .unwrap();
    let mut writer = Box::pin(async {
        let mut session = f.client.start_session().await.unwrap();
        crate::transaction_start::start_write(&f.client.database(&f.name), &mut session, None)
            .await
            .unwrap();
        f.rows(&f.client)
            .update_one(doc! {"_id": "row"}, doc! {"$inc": {"value": 1_i32}})
            .session(&mut session)
            .await
            .unwrap();
    });
    tokio::select! {
        _ = &mut writer => panic!("the stalled command returned before cancellation"),
        result = admin.run_command(doc! {
            "waitForFailPoint": "failCommand",
            "timesEntered": count + 1,
            "maxTimeMS": 5_000,
        }) => { result.unwrap(); }
    }
    drop(writer);
    // Let the Mongo command finish before checking an independent writer.
    tokio::time::sleep(Duration::from_millis(2_100)).await;
    f.row_is_free_promptly().await;
    assert_eq!(f.value().await, 0);
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set with failpoints"]
async fn cancelled_first_write_strands_no_row_lock() {
    assert_cancelled_start_releases_row(&["aggregate", "update"]).await;
    assert_cancelled_start_releases_row(&["update"]).await;
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set with failpoints"]
async fn cancelled_abort_mid_send_strands_no_row_lock() {
    let f = Fixture::new().await;
    {
        let mut session = f.locked_transaction().await;
        f.cancel_mid_send("abortTransaction", session.abort_guarded())
            .await;
    }
    f.row_is_free_promptly().await;
    assert_eq!(
        f.value().await,
        0,
        "the cancelled transaction never commits"
    );
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set with failpoints"]
async fn cancelled_commit_mid_send_resolves_without_stranding_the_row_lock() {
    let f = Fixture::new().await;
    {
        let mut session = f.locked_transaction().await;
        f.cancel_mid_send("commitTransaction", session.commit_guarded())
            .await;
    }
    f.row_is_free_promptly().await;
    // The server refused the stalled commit, so ending the session aborted it.
    assert_eq!(f.value().await, 0);
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set with failpoints"]
async fn completed_terminal_commands_leave_the_session_reusable() {
    let f = Fixture::new().await;
    let mut session = f.locked_transaction().await;
    session.commit_guarded().await.unwrap();
    assert_eq!(f.value().await, 1);
    session.start_transaction().await.unwrap();
    f.rows(&f.client)
        .update_one(doc! {"_id": "row"}, doc! {"$inc": {"value": 1_i32}})
        .session(&mut session)
        .await
        .unwrap();
    session.abort_guarded().await.unwrap();
    // A completed command disarms its guard: the same session keeps working.
    session.start_transaction().await.unwrap();
    f.rows(&f.client)
        .update_one(doc! {"_id": "row"}, doc! {"$inc": {"value": 1_i32}})
        .session(&mut session)
        .await
        .unwrap();
    session.commit_guarded().await.unwrap();
    assert_eq!(f.value().await, 2);
    drop(session);
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set with failpoints"]
async fn driver_runner_cancels_opening_read_and_both_terminal_commands() {
    for (command, commits) in [
        ("aggregate", true),
        ("commitTransaction", true),
        ("abortTransaction", false),
    ] {
        let f = Fixture::new().await;
        let db = f.client.database(&f.name);
        let body_count = std::sync::atomic::AtomicUsize::new(0);
        let operation = and_run_write(
            &db,
            (f.rows(&f.client), &body_count, commits),
            |session, (rows, count, commits)| {
                async move {
                    count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    rows.update_one(doc! {"_id":"row"}, doc! {"$inc":{"value":1_i32}})
                        .session(session)
                        .await?;
                    if *commits {
                        Ok(())
                    } else {
                        Err(mongodb::error::Error::custom("refuse"))
                    }
                }
                .boxed()
            },
        );
        f.cancel_mid_send(command, operation).await;
        f.row_is_free_promptly().await;
        assert_eq!(f.value().await, 0);
        assert_eq!(
            body_count.load(std::sync::atomic::Ordering::Relaxed),
            usize::from(command != "aggregate")
        );
        f.cleanup().await;
    }
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set with failpoints"]
async fn driver_runner_keeps_abandoned_session_checked_out_during_cleanup() {
    let f = Fixture::new().await;
    let db = f.client.database(&f.name);
    let (write_done, done) = tokio::sync::oneshot::channel();
    let mut operation = Box::pin(and_run_write(
        &db,
        (f.rows(&f.client), Some(write_done)),
        |session, (rows, write_done)| {
            async move {
                rows.update_one(doc! {"_id":"row"}, doc! {"$inc":{"value":1_i32}})
                    .session(&mut *session)
                    .await?;
                write_done
                    .take()
                    .unwrap()
                    .send(session.id().clone())
                    .unwrap();
                std::future::pending::<mongodb::error::Result<()>>().await
            }
            .boxed()
        },
    ));
    let cancelled_id = tokio::select! {
        id = done => id.unwrap(),
        _ = &mut operation => panic!("the fixture body returned before cancellation"),
    };
    let count = f
        .next_command(
            "killSessions",
            doc! {"blockConnection":true, "blockTimeMS":400_i32},
        )
        .await;
    drop(operation);
    f.wait_for_command(count).await;
    let mut next = f.client.start_session().await.unwrap();
    assert_ne!(next.id(), &cancelled_id);
    assert!(f
        .rows(&f.client)
        .find_one(doc! {"_id":"row"})
        .session(&mut next)
        .await
        .unwrap()
        .is_some());
    f.row_is_free_promptly().await;
    assert_eq!(f.value().await, 0);
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set with failpoints"]
async fn driver_runner_retries_transient_work_and_reuses_an_unknown_commit() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let f = Fixture::new().await;
    let db = f.client.database(&f.name);
    for (command, code, label, expected_bodies) in [
        ("update", 112_i32, "TransientTransactionError", 2_usize),
        (
            "commitTransaction",
            6_i32,
            "UnknownTransactionCommitResult",
            1_usize,
        ),
    ] {
        let count = AtomicUsize::new(0);
        f.next_command(command, doc! {"errorCode":code, "errorLabels":[label]})
            .await;
        and_run_write(
            &db,
            (f.rows(&f.client), &count),
            |session, (rows, count)| {
                async move {
                    count.fetch_add(1, Ordering::Relaxed);
                    rows.update_one(doc! {"_id":"row"}, doc! {"$inc":{"value":1_i32}})
                        .session(session)
                        .await?;
                    Ok(())
                }
                .boxed()
            },
        )
        .await
        .unwrap();
        assert_eq!(count.load(Ordering::Relaxed), expected_bodies);
    }
    assert_eq!(f.value().await, 2);
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set with failpoints"]
async fn cancelled_terminal_command_can_end_its_own_session_under_the_exact_runtime_grant() {
    let uri = fixture_uri();
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let name = format!("cmaeg_terminal_grant_{}", uuid::Uuid::new_v4().simple());
    let db = admin.database(&name);
    db.run_command(crate::records::mongo::role(&name, false))
        .await
        .unwrap();
    let password = uuid::Uuid::new_v4().to_string();
    db.run_command(doc! {"createUser": "terminal-runtime", "pwd": &password,
    "roles": [{"role": "cma_eg_runtime", "db": &name}]})
        .await
        .unwrap();
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.credential = Some(
        Credential::builder()
            .username("terminal-runtime".to_owned())
            .password(password)
            .source(name.clone())
            .build(),
    );
    let runtime = Client::with_options(options).unwrap();
    let records = runtime
        .database(&name)
        .collection::<Document>(crate::records::mongo::RECORDS);
    let mut session = runtime.start_session().await.unwrap();
    crate::transaction_start::start_write(&runtime.database(&name), &mut session, None)
        .await
        .expect("the exact runtime grant can start a lock-free write transaction");
    records
        .find_one(doc! {})
        .session(&mut session)
        .await
        .expect("the runtime grant reads gateway records");
    // Control: the credential really is confined to its grant.
    let foreign = runtime
        .database(&format!("{name}_foreign"))
        .collection::<Document>(crate::records::mongo::RECORDS)
        .find_one(doc! {})
        .await
        .unwrap_err();
    assert!(
        matches!(foreign.kind.as_ref(), mongodb::error::ErrorKind::Command(c) if c.code == 13),
        "the runtime credential must be refused outside its grant"
    );
    // The guard's recovery command needs no privilege beyond owning the session.
    kill_session(&runtime, session.id())
        .await
        .expect("a gateway runtime credential may end its own session");
    assert!(
        records
            .find_one(doc! {})
            .session(&mut session)
            .await
            .is_err(),
        "ending the session aborts its open transaction"
    );
    drop(session);
    db.run_command(doc! {"dropUser": "terminal-runtime"})
        .await
        .unwrap();
    db.drop().await.unwrap();
}
