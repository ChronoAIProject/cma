//! A transaction whose commit or abort is cancelled mid-send must not keep its
//! row locks (#3288).
use super::GuardedTerminal as _;
use mongodb::{
    bson::{doc, Document},
    options::ClientOptions,
    Client, ClientSession, Collection,
};
use std::time::{Duration, Instant};

struct Fixture {
    admin: Client,
    client: Client,
    name: String,
}

impl Fixture {
    async fn new() -> Self {
        let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI")
            .expect("isolated failpoint-enabled replica set");
        let admin = Client::with_uri_str(&uri).await.unwrap();
        let name = format!("cma_terminal_{}", uuid::Uuid::new_v4().simple());
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

    /// Hold `command` on the server, then refuse it, so the server never runs
    /// the terminal command the cancelled caller sent.
    async fn stall(&self, command: &str) {
        self.admin
            .database("admin")
            .run_command(doc! {
                "configureFailPoint": "failCommand", "mode": {"times": 1_i32},
                "data": {
                    "appName": &self.name, "failCommands": [command],
                    "blockConnection": true, "blockTimeMS": 400_i32, "errorCode": 2_i32,
                },
            })
            .await
            .unwrap();
    }

    async fn cancel_mid_send<F: std::future::Future>(&self, command: &str, terminal: F) {
        self.stall(command).await;
        assert!(
            tokio::time::timeout(Duration::from_millis(100), terminal)
                .await
                .is_err(),
            "the stalled {command} must be in flight when the caller is cancelled"
        );
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
                "a cancelled terminal command stranded the row lock ({conflicts} conflicts)"
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

#[tokio::test]
#[ignore = "requires disposable replica set via CHRONO_TEST_MONGODB_FAILPOINT_URI"]
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
#[ignore = "requires disposable replica set via CHRONO_TEST_MONGODB_FAILPOINT_URI"]
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
#[ignore = "requires disposable replica set via CHRONO_TEST_MONGODB_FAILPOINT_URI"]
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
