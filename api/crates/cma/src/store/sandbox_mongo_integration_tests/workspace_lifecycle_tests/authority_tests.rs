use super::*;
use std::time::{Duration, SystemTime};

mod renewal_tests;
mod snapshot_tests;
mod snapshot_unknown_tests;

#[derive(Clone, Copy)]
enum Kind {
    Sandbox,
    Workspace,
}

impl Kind {
    fn id(self) -> &'static str {
        match self {
            Self::Sandbox => "sbx_server_claim",
            Self::Workspace => "wks_server_claim",
        }
    }

    fn rows(self, store: &MongoSandboxStore) -> &mongodb::Collection<Document> {
        match self {
            Self::Sandbox => &store.sandboxes,
            Self::Workspace => &store.workspaces,
        }
    }

    async fn claim(
        self,
        store: &MongoSandboxStore,
        holder: &str,
        now: String,
        until: String,
    ) -> Result<Option<u64>, SandboxStoreError> {
        match self {
            Self::Sandbox => store
                .claim_sandbox_work(&SandboxId(self.id().into()), holder, now, until)
                .await
                .map(|row| row.map(|row| row.work.lease.unwrap().fence)),
            Self::Workspace => store
                .claim_workspace_work(&WorkspaceId(self.id().into()), holder, now, until)
                .await
                .map(|row| row.map(|row| row.work.lease.unwrap().fence)),
        }
    }

    async fn row(self, store: &MongoSandboxStore) -> Document {
        self.rows(store)
            .find_one(doc! {"_id":self.id()})
            .await
            .unwrap()
            .unwrap()
    }

    async fn set(self, store: &MongoSandboxStore, fields: Document) {
        self.rows(store)
            .update_one(doc! {"_id":self.id()}, doc! {"$set":fields})
            .await
            .unwrap();
    }
}

fn at(offset: i64) -> String {
    let delta = Duration::from_secs(offset.unsigned_abs());
    layer2_core::rfc3339_at(if offset < 0 {
        SystemTime::now() - delta
    } else {
        SystemTime::now() + delta
    })
}

async fn seed(store: &MongoSandboxStore) {
    let parent = sandbox(Kind::Sandbox.id());
    store.create_sandbox(parent.clone()).await.unwrap();
    store
        .create_workspace(workspace(&parent, Kind::Workspace.id(), "server-claim"))
        .await
        .unwrap();
}

async fn peer(name: &str) -> MongoSandboxStore {
    let client = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    MongoSandboxStore::open(&client, name).await.unwrap()
}

fn refused(result: Result<Option<u64>, SandboxStoreError>) {
    assert!(
        matches!(result, Ok(None) | Err(SandboxStoreError::Conflict)),
        "{result:?}"
    );
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn independent_workers_cannot_steal_live_or_future_sandbox_workspace_work() {
    let (client, name) = database("work_server_authority").await;
    let first = MongoSandboxStore::open(&client, &name).await.unwrap();
    let second = peer(&name).await;
    seed(&first).await;
    for kind in [Kind::Sandbox, Kind::Workspace] {
        let before = kind.row(&first).await;
        refused(kind.claim(&first, "delayed", at(-60), at(-30)).await);
        assert_eq!(kind.row(&second).await, before);

        let (left, right) = tokio::join!(
            kind.claim(&first, "first", at(0), at(60)),
            kind.claim(&second, "second", at(0), at(60)),
        );
        let claims = [left.unwrap(), right.unwrap()];
        assert_eq!(claims.into_iter().flatten().collect::<Vec<_>>(), vec![1]);
        let held = kind.row(&first).await;
        refused(kind.claim(&second, "ahead", at(120), at(180)).await);
        assert_eq!(kind.row(&first).await, held);

        let expired = at(-1);
        kind.set(
            &first,
            doc! {"work_lease_expires_at":&expired,
            "record.work.lease.expires_at":&expired},
        )
        .await;
        assert_eq!(
            kind.claim(&second, "successor", at(0), at(60))
                .await
                .unwrap(),
            Some(2)
        );

        let future = at(120);
        kind.set(
            &first,
            doc! {"work_ready_at":&future,"record.work.ready_at":&future,
            "work_lease_expires_at":Bson::Null,"record.work.lease":Bson::Null},
        )
        .await;
        let scheduled = kind.row(&first).await;
        refused(kind.claim(&first, "ahead", at(180), at(240)).await);
        assert_eq!(kind.row(&second).await, scheduled);
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn delayed_acquisition_write_cannot_install_an_expired_requested_lease() {
    let (admin, name) = database("work_delayed_authority").await;
    let app = format!("work-claim-{}", uuid::Uuid::new_v4());
    let mut options =
        ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap())
            .await
            .unwrap();
    options.app_name = Some(app.clone());
    let client = Client::with_options(options).unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    seed(&store).await;
    for kind in [Kind::Sandbox, Kind::Workspace] {
        let before = kind.row(&store).await;
        admin
            .database("admin")
            .run_command(doc! {
                "configureFailPoint":"failCommand","mode":{"times":1_i32},
                "data":{"failCommands":["update"],"appName":&app,
                    "blockConnection":true,"blockTimeMS":2200_i32}
            })
            .await
            .unwrap();
        let result = kind.claim(&store, "delayed", at(0), at(1)).await;
        admin
            .database("admin")
            .run_command(doc! {
                "configureFailPoint":"failCommand","mode":"off"
            })
            .await
            .unwrap();
        refused(result);
        assert_eq!(kind.row(&store).await, before);
        assert_eq!(
            kind.claim(&store, "fresh", at(0), at(60)).await.unwrap(),
            Some(1)
        );
    }
    admin.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn malformed_work_due_times_refuse_without_advancing_authority() {
    let (client, name) = database("work_corrupt_authority").await;
    let first = MongoSandboxStore::open(&client, &name).await.unwrap();
    let second = peer(&name).await;
    seed(&first).await;
    for kind in [Kind::Sandbox, Kind::Workspace] {
        for bad in [
            Bson::String("2000-99-01T00:00:00Z".into()),
            Bson::String(String::new()),
            Bson::Int64(0),
            Bson::Array(vec![Bson::String(at(-1))]),
        ] {
            kind.set(
                &first,
                doc! {"work_ready_at":bad.clone(),"record.work.ready_at":bad},
            )
            .await;
            let before = kind.row(&first).await;
            assert!(!matches!(
                kind.claim(&second, "invalid", at(0), at(60)).await,
                Ok(Some(_))
            ));
            assert_eq!(kind.row(&first).await, before);
        }
        let valid = at(-60);
        kind.set(
            &first,
            doc! {"work_ready_at":&valid,"record.work.ready_at":&valid},
        )
        .await;
        assert_eq!(
            kind.claim(&second, "fresh", at(0), at(60)).await.unwrap(),
            Some(1)
        );
        for bad in [
            Bson::String("2000-99-01T00:00:00Z".into()),
            Bson::String(String::new()),
            Bson::Int64(0),
            Bson::Array(vec![Bson::String(at(-1))]),
            Bson::Array(vec![Bson::String(at(60))]),
        ] {
            kind.set(
                &first,
                doc! {"work_lease_expires_at":bad.clone(),
                "record.work.lease.expires_at":bad},
            )
            .await;
            let before = kind.row(&first).await;
            assert!(!matches!(
                kind.claim(&second, "invalid", at(120), at(180)).await,
                Ok(Some(_))
            ));
            assert_eq!(kind.row(&first).await, before);
        }
        kind.set(
            &first,
            doc! {"work_lease_expires_at":&valid,
            "record.work.lease.expires_at":&valid},
        )
        .await;
        assert_eq!(
            kind.claim(&second, "successor", at(0), at(60))
                .await
                .unwrap(),
            Some(2)
        );
    }
    client.database(&name).drop().await.unwrap();
}
