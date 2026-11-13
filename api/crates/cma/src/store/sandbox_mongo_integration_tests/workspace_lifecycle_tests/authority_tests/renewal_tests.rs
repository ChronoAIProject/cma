use super::*;

#[derive(Clone)]
enum Claim {
    Sandbox(Box<Sandbox>),
    Workspace(Box<Workspace>),
}

impl Claim {
    async fn read(store: &MongoSandboxStore, kind: Kind) -> Self {
        match kind {
            Kind::Sandbox => Self::Sandbox(Box::new(
                store
                    .get_sandbox(&SandboxId(kind.id().into()))
                    .await
                    .unwrap()
                    .unwrap(),
            )),
            Kind::Workspace => Self::Workspace(Box::new(
                store
                    .get_workspace(&WorkspaceId(kind.id().into()))
                    .await
                    .unwrap()
                    .unwrap(),
            )),
        }
    }

    async fn renew(&self, store: &MongoSandboxStore, until: &str) -> Option<String> {
        // Deliberately wrong caller clock: durable authority is server-based.
        match self {
            Self::Sandbox(row) => store.renew_sandbox_work(row, &at(3600), until).await,
            Self::Workspace(row) => store.renew_workspace_work(row, &at(3600), until).await,
        }
        .unwrap()
    }

    fn wrong_holder(&self) -> Self {
        let mut wrong = self.clone();
        match &mut wrong {
            Self::Sandbox(row) => row.work.lease.as_mut().unwrap().holder.push_str("-wrong"),
            Self::Workspace(row) => row.work.lease.as_mut().unwrap().holder.push_str("-wrong"),
        }
        wrong
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn independent_renewals_preserve_identity_revision_and_monotonic_expiry() {
    let (client, name) = database("work_renewal_authority").await;
    let first = MongoSandboxStore::open(&client, &name).await.unwrap();
    let second = peer(&name).await;
    seed(&first).await;
    for kind in [Kind::Sandbox, Kind::Workspace] {
        assert_eq!(
            kind.claim(&first, "original", at(0), at(5)).await.unwrap(),
            Some(1)
        );
        let original = Claim::read(&first, kind).await;
        let before = kind.row(&first).await;
        assert!(original
            .wrong_holder()
            .renew(&second, &at(15))
            .await
            .is_none());
        assert_eq!(kind.row(&first).await, before);
        let shorter = at(10);
        let longer = at(12);
        assert!(original.renew(&first, &shorter).await.is_some());
        assert!(original.renew(&second, &longer).await.is_some());
        let renewed = kind.row(&first).await;
        assert_eq!(renewed.get_str("work_lease_expires_at").unwrap(), longer);
        assert_eq!(
            renewed.get_i64("revision").unwrap(),
            before.get_i64("revision").unwrap()
        );
        assert_eq!(
            original.renew(&second, &shorter).await.as_deref(),
            Some(longer.as_str())
        );
        let claim = Claim::read(&second, kind).await;
        let lease = match claim {
            Claim::Sandbox(row) => row.work.lease.unwrap().expires_at,
            Claim::Workspace(row) => row.work.lease.unwrap().expires_at,
        };
        assert_eq!(lease, longer);
        assert!(original.renew(&first, &at(-1)).await.is_none());
        assert_eq!(kind.row(&first).await, renewed);
        // Never-activated work retains the existing expiry-only renewal API.
        let requested = at(3600);
        assert_eq!(original.renew(&first, &requested).await, Some(requested));
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn workspace_renewal_refuses_a_changed_parent_owner() {
    let (client, name) = database("work_renewal_parent").await;
    let first = MongoSandboxStore::open(&client, &name).await.unwrap();
    let second = peer(&name).await;
    seed(&first).await;
    let kind = Kind::Workspace;
    kind.claim(&first, "original", at(0), at(10))
        .await
        .unwrap()
        .unwrap();
    let original = Claim::read(&first, kind).await;
    let before = kind.row(&first).await;
    first
        .sandboxes
        .update_one(
            doc! {"_id":Kind::Sandbox.id()},
            doc! {
                "$set":{"owner":"replacement-owner","record.owner":"replacement-owner"},
                "$inc":{"revision":1_i64,"record.revision":1_i64}
            },
        )
        .await
        .unwrap();
    assert!(original.renew(&second, &at(15)).await.is_none());
    assert_eq!(kind.row(&first).await, before);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn expired_original_cannot_renew_before_or_after_takeover() {
    let (client, name) = database("work_expired_renewal").await;
    let first = MongoSandboxStore::open(&client, &name).await.unwrap();
    let second = peer(&name).await;
    seed(&first).await;
    let mut claims = Vec::new();
    for kind in [Kind::Sandbox, Kind::Workspace] {
        kind.claim(&first, "original", at(0), at(3))
            .await
            .unwrap()
            .unwrap();
        claims.push((kind, Claim::read(&first, kind).await));
    }
    tokio::time::sleep(Duration::from_millis(3200)).await;
    for (kind, old) in claims {
        let expired = kind.row(&first).await;
        assert!(old.renew(&first, &at(15)).await.is_none());
        assert_eq!(kind.row(&first).await, expired);
        assert_eq!(
            kind.claim(&second, "successor", at(0), at(10))
                .await
                .unwrap(),
            Some(2)
        );
        let successor = Claim::read(&second, kind).await;
        let held = kind.row(&second).await;
        assert!(old.renew(&first, &at(15)).await.is_none());
        assert_eq!(kind.row(&second).await, held);
        assert!(successor.renew(&second, &at(15)).await.is_some());
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a replica set"]
async fn delayed_renewal_cannot_extend_past_its_requested_deadline() {
    let (admin, name) = database("work_delayed_renewal").await;
    let app = format!("work-renew-{}", uuid::Uuid::new_v4());
    let mut options =
        ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap())
            .await
            .unwrap();
    options.app_name = Some(app.clone());
    let client = Client::with_options(options).unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    seed(&store).await;
    for kind in [Kind::Sandbox, Kind::Workspace] {
        kind.claim(&store, "original", at(0), at(10))
            .await
            .unwrap()
            .unwrap();
        let original = Claim::read(&store, kind).await;
        let before = kind.row(&store).await;
        admin
            .database("admin")
            .run_command(doc! {
                "configureFailPoint":"failCommand","mode":{"times":1_i32},
                "data":{"failCommands":["findAndModify"],"appName":&app,
                "blockConnection":true,"blockTimeMS":2200_i32}
            })
            .await
            .unwrap();
        assert!(original.renew(&store, &at(1)).await.is_none());
        admin
            .database("admin")
            .run_command(doc! {
                "configureFailPoint":"failCommand","mode":"off"
            })
            .await
            .unwrap();
        assert_eq!(kind.row(&store).await, before);
        assert!(original.renew(&store, &at(15)).await.is_some());
    }
    admin.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn store_clock_receipt_bounds_local_authority_for_both_worker_kinds() {
    let (client, name) = database("work_renewal_receipt").await;
    let first = MongoSandboxStore::open(&client, &name).await.unwrap();
    let second = peer(&name).await;
    seed(&first).await;
    for kind in [Kind::Sandbox, Kind::Workspace] {
        kind.claim(&first, "original", at(0), at(10))
            .await
            .unwrap()
            .unwrap();
        let claim = Claim::read(&first, kind).await;
        let started = std::time::Instant::now();
        let grant = match &claim {
            Claim::Sandbox(row) => {
                second
                    .renew_sandbox_authority(row, &at(3600), &at(3600))
                    .await
            }
            Claim::Workspace(row) => {
                second
                    .renew_workspace_authority(row, &at(3600), &at(3600))
                    .await
            }
        }
        .unwrap()
        .unwrap();
        assert!(grant.valid_for > Duration::from_secs(13));
        assert!(grant.valid_for <= Duration::from_secs(15));
        assert!(started.elapsed() < grant.valid_for);
        assert_eq!(
            kind.row(&first)
                .await
                .get_str("work_lease_expires_at")
                .unwrap(),
            grant.expires_at
        );
        let refused = match claim.wrong_holder() {
            Claim::Sandbox(row) => {
                second
                    .renew_sandbox_authority(&row, &at(-3600), &at(15))
                    .await
            }
            Claim::Workspace(row) => {
                second
                    .renew_workspace_authority(&row, &at(-3600), &at(15))
                    .await
            }
        }
        .unwrap();
        assert!(refused.is_none());
    }
    client.database(&name).drop().await.unwrap();
}
