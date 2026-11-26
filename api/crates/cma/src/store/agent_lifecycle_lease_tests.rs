use super::*;
use mongodb::{bson::Document, Client};

struct Fixture {
    first: MongoAgentLifecycleStore,
    second: MongoAgentLifecycleStore,
    db: Database,
    admin: Database,
    app: String,
    id: String,
}
impl Fixture {
    async fn new() -> Self {
        let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
        let app = format!("lifecycle-lease-{}", uuid::Uuid::new_v4().simple());
        let mut options = mongodb::options::ClientOptions::parse(&uri).await.unwrap();
        options.app_name = Some(app.clone());
        let first_client = Client::with_options(options).unwrap();
        let second_client = Client::with_uri_str(&uri).await.unwrap();
        let db = second_client.database(&format!(
            "lifecycle_lease_{}",
            uuid::Uuid::new_v4().simple()
        ));
        let first = MongoAgentLifecycleStore::open(&first_client.database(db.name()))
            .await
            .unwrap();
        let second = MongoAgentLifecycleStore::open(&db).await.unwrap();
        let input = LifecycleInput {
            behavior: LifecycleBehavior::PromptRepository,
            managed_agent_instructions: None,
            setup_message: None,
            request: serde_json::from_value(serde_json::json!({"name":"Lease fixture"})).unwrap(),
            resolved_image: None,
            capacity: None,
            sandbox_availability: Default::default(),
            capabilities: vec![],
            actor: None,
        };
        let CreateResult::Created(id) = first
            .create(&SubjectId::new("owner"), "key", input)
            .await
            .unwrap()
        else {
            panic!("new fixture must admit durable work")
        };
        Self {
            first,
            second,
            db,
            admin: second_client.database("admin"),
            app,
            id,
        }
    }
    fn rows(&self) -> Collection<Document> {
        self.db.collection(COLLECTION)
    }
    async fn now(&self) -> DateTime {
        self.admin
            .run_command(doc! {"hello":1})
            .await
            .unwrap()
            .get_datetime("localTime")
            .copied()
            .unwrap()
    }
    async fn set(&self, update: Document) {
        self.rows()
            .update_one(doc! {"_id":&self.id}, doc! {"$set":update})
            .await
            .unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn independent_lifecycle_claimants_share_server_authority_and_retry_deadline() {
    let f = Fixture::new().await;
    let before = f.now().await.timestamp_millis();
    let (a, b) = tokio::join!(f.first.claim(&f.id), f.second.claim(&f.id));
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_ne!(a.is_some(), b.is_some());
    let claim = a.or(b).unwrap();
    let after = f.now().await.timestamp_millis();
    let expiry = claim.lease_expires_at.unwrap().timestamp_millis();
    assert!((before + 90_000..=after + 90_000).contains(&expiry));
    let lease = claim.lease().unwrap();
    f.first.mark_sandbox(&lease, "sandbox").await.unwrap();
    f.first.renew(&lease).await.unwrap();
    let before = f.now().await.timestamp_millis();
    f.first.retry(&lease, 1).await.unwrap();
    let after = f.now().await.timestamp_millis();
    let queued = f
        .second
        .read_for_owner(&SubjectId::new("owner"), &f.id)
        .await
        .unwrap()
        .unwrap();
    assert!((before + 5_000..=after + 5_000).contains(&queued.retry_at.unwrap().timestamp_millis()));
    assert_eq!(queued.sandbox_id.as_deref(), Some("sandbox"));
    assert!(f.second.claim(&f.id).await.unwrap().is_none());
    f.set(doc! {"retry_at":DateTime::from_millis(0)}).await;
    let next = f.second.claim(&f.id).await.unwrap().unwrap();
    assert_ne!(next.lease().unwrap().token, lease.token);
    assert_eq!(next.sandbox_id.as_deref(), Some("sandbox"));
    assert_eq!(next.attempts, 2);
    assert!(f.first.succeed(&lease, "stale").await.is_err());
    f.second
        .succeed(&next.lease().unwrap(), "agent")
        .await
        .unwrap();
    assert!(f.first.claim(&f.id).await.unwrap().is_none());
    f.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn expired_lifecycle_token_refuses_every_worker_write_before_and_after_takeover() {
    let f = Fixture::new().await;
    let lease = f
        .first
        .claim(&f.id)
        .await
        .unwrap()
        .unwrap()
        .lease()
        .unwrap();
    f.set(doc! {"lease_expires_at":DateTime::from_millis(0)})
        .await;
    for successor in [false, true] {
        if successor {
            assert!(f.second.claim(&f.id).await.unwrap().is_some());
        }
        let before = f
            .rows()
            .find_one(doc! {"_id":&f.id})
            .await
            .unwrap()
            .unwrap();
        assert!(f.first.renew(&lease).await.is_err());
        assert!(f.first.retry(&lease, 1).await.is_err());
        assert!(f.first.mark_sandbox(&lease, "stale").await.is_err());
        assert!(f.first.mark_agent(&lease, "stale").await.is_err());
        assert!(f
            .first
            .record_progress(&lease, "test", "ready", "stale")
            .await
            .is_err());
        assert!(f
            .first
            .append_event(&lease, 1, "test", "ready", "stale")
            .await
            .is_err());
        assert!(f.first.succeed(&lease, "stale").await.is_err());
        assert!(f.first.fail(&lease, "stale").await.is_err());
        assert!(f
            .first
            .wait_for_connection(&lease, vec![], serde_json::json!({}), None)
            .await
            .is_err());
        assert_eq!(
            f.rows()
                .find_one(doc! {"_id":&f.id})
                .await
                .unwrap()
                .unwrap(),
            before
        );
    }
    f.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI disposable replica set"]
async fn delayed_lifecycle_renewal_cannot_revive_lease_after_server_expiry() {
    let f = Fixture::new().await;
    let lease = f
        .first
        .claim(&f.id)
        .await
        .unwrap()
        .unwrap()
        .lease()
        .unwrap();
    f.rows()
        .update_one(
            doc! {"_id":&f.id},
            vec![doc! {"$set":{
                "lease_expires_at":{"$add":["$$NOW",1500_i64]}
            }}],
        )
        .await
        .unwrap();
    f.admin.run_command(doc! {"configureFailPoint":"failCommand","mode":{"times":1},
        "data":{"failCommands":["update"],"appName":&f.app,"blockConnection":true,"blockTimeMS":2500}
    }).await.unwrap();
    // Freeze a serialized renewal in Mongo while its lease naturally expires;
    // no peer writes the original row or changes its token during this wait.
    let outcome = tokio::time::timeout(Duration::from_secs(8), f.first.renew(&lease)).await;
    f.admin
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .unwrap();
    assert!(matches!(
        outcome.unwrap(),
        Err(StoreError::RevisionConflict { .. })
    ));
    let next = f
        .second
        .claim(&f.id)
        .await
        .unwrap()
        .unwrap()
        .lease()
        .unwrap();
    assert_ne!(next.token, lease.token);
    f.second.mark_agent(&next, "successor").await.unwrap();
    f.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn lifecycle_renewal_restores_server_duration_and_refuses_malformed_dates() {
    let f = Fixture::new().await;
    let lease = f
        .first
        .claim(&f.id)
        .await
        .unwrap()
        .unwrap()
        .lease()
        .unwrap();
    let later = DateTime::from_millis(f.now().await.timestamp_millis() + 120_000);
    f.set(doc! {"lease_expires_at":later}).await;
    let before = f.now().await.timestamp_millis();
    f.first.renew(&lease).await.unwrap();
    let after = f.now().await.timestamp_millis();
    let renewed = *f
        .rows()
        .find_one(doc! {"_id":&f.id})
        .await
        .unwrap()
        .unwrap()
        .get_datetime("lease_expires_at")
        .unwrap();
    assert!((before + 90_000..=after + 90_000).contains(&renewed.timestamp_millis()));
    assert!(
        renewed < later,
        "retain the existing finite-duration renewal contract"
    );
    // Expression comparisons must keep BSON date type checks; otherwise type
    // ordering can make a malformed envelope appear expired or authoritative.
    for bad in [
        Bson::String("invalid".into()),
        Bson::Int64(0),
        Bson::Boolean(true),
        Bson::Array(vec![Bson::DateTime(DateTime::from_millis(0))]),
        Bson::Array(vec![Bson::Null]),
        Bson::Document(doc! {}),
    ] {
        f.set(doc! {"status":"running","lease_expires_at":bad.clone()})
            .await;
        assert!(f.first.renew(&lease).await.is_err());
        assert!(f.second.claim(&f.id).await.unwrap().is_none());
        f.set(doc! {"status":"queued","retry_at":bad}).await;
        assert!(f.second.claim(&f.id).await.unwrap().is_none());
    }
    f.db.drop().await.unwrap();
}
