//! Actual managed Mongo store + retirement reconciler, with a modeled provider.
//! No provider finality or runnable rollback qualification is implied.
use super::*;
use crate::{
    release_authority::*, runtime_release::Session, store::native_warm_pools::MongoNativeWarmPools,
};
use mongodb::{
    bson::{doc, DateTime, Document},
    options::{ValidationAction, ValidationLevel},
    Client, Database,
};

struct Fixture {
    db: Database,
    store: MongoNativeWarmPools,
    session: Arc<Session>,
    provider: Provider,
    image: String,
    next: PoolTarget,
}

async fn fixture(retiring: bool) -> Fixture {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").expect("owned local replica set required");
    assert!(uri.starts_with("mongodb://127.0.0.1:") || uri.starts_with("mongodb://localhost:"));
    let client = Client::with_uri_str(uri).await.unwrap();
    let db = client.database(&format!(
        "cma_managed_retirement_{}",
        uuid::Uuid::new_v4().simple()
    ));
    for name in COLLECTIONS {
        db.create_collection(*name).await.unwrap();
    }
    // Install the exact reader floor explicitly in this disposable fixture.
    // Production open_managed remains read-only and verifies this validator.
    db.create_collection("native_warm_pool_targets").validator(doc! {
        "$or": [{"document_schema_version":3_i64,"machine":false}, {"document_schema_version":4_i64,"machine":true}],
        "$and": [
            {"document_schema_version":{"$type":["int","long"]}},
            {"document_schema_version":{"$not":{"$type":"array"}}},
            {"machine":{"$type":"bool"}}, {"machine":{"$not":{"$type":"array"}}}
        ]
    }).validation_level(ValidationLevel::Strict).validation_action(ValidationAction::Error).await.unwrap();
    let writer = WriterIdentity {
        deployment: "test".into(),
        instance_id: "test-instance".into(),
        config_sha256: "a".repeat(64),
        epoch: 1,
        fence: 1,
    };
    let image = format!("registry.example/runtime@sha256:{}", "b".repeat(64));
    let pair = ReleasePair {
        agent: ReleaseTarget {
            image: image.clone(),
            plan_sha256: "b".repeat(64),
            target_id: format!("wp_{}", "b".repeat(64)),
            target_revision: 1,
            pool_name: "selected".into(),
            pool_uid: "selected-uid".into(),
            template_name: "selected-template".into(),
            template_uid: "selected-template-uid".into(),
            qualification_sha256: "b".repeat(64),
        },
        machine: None,
    };
    let head = ReleaseHead {
        deployment: writer.deployment.clone(),
        schema_version: HEAD_SCHEMA_VERSION,
        epoch: 1,
        control_revision: 1,
        admission_sequence: 0,
        active_effects: 0,
        phase: Phase::Active,
        pair: pair.clone(),
        writer: Some(writer.clone()),
        writer_fence: 1,
        operation_id: None,
        protected_pairs: vec![pair],
        committed_at: DateTime::from_millis(1),
    };
    head.validate().unwrap();
    db.collection::<ReleaseHead>(HEADS)
        .insert_one(head)
        .await
        .unwrap();
    let session = Arc::new(Session {
        store: Arc::new(AuthorityStore::open(client, db.clone()).await.unwrap()),
        writer,
    });
    let mut old = target();
    old.id = format!("wp_{}", "e".repeat(64));
    old.document_schema_version = 3;
    old.lease = None;
    old.lease_until = DateTime::from_millis(0);
    old.activated_at = Some(DateTime::from_millis(1));
    old.retiring = retiring;
    if retiring {
        old.target_replicas = 0;
    }
    let provider = Provider {
        supports_target_fencing: true,
        calls: Arc::default(),
        original: old.plan.clone(),
        refuse_replaced_uid: AtomicBool::new(false),
        retired: AtomicBool::new(false),
    };
    old.binding = Some(provider.binding());
    let mut next = old.clone();
    next.id = format!("wp_{}", "f".repeat(64));
    next.plan.config_sha256 = "f".repeat(64);
    next.binding = None;
    db.collection::<PoolTarget>("native_warm_pool_targets")
        .insert_one(old)
        .await
        .unwrap();
    let store = MongoNativeWarmPools::open_managed(&db, "test", session.clone())
        .await
        .unwrap();
    Fixture {
        db,
        store,
        session,
        provider,
        image,
        next,
    }
}

impl Fixture {
    async fn next_poll(&self) {
        self.db
            .collection::<PoolTarget>("native_warm_pool_targets")
            .insert_one(self.next.clone())
            .await
            .unwrap();
        assert_eq!(self.store.claim().await.unwrap().unwrap().id, self.next.id);
    }
    async fn tip(&self) -> NativePollReceipt {
        self.session
            .store
            .native_poll_tip(&self.session.writer, NativePollLane::Background)
            .await
            .unwrap()
            .unwrap()
    }
    async fn assert_original_receipt(&self, original: &NativePollReceipt) {
        let record = self
            .db
            .collection::<Document>(STORE_COMMANDS)
            .find_one(doc! {"_id":original.key()})
            .await
            .unwrap()
            .unwrap();
        assert_eq!(record.get_str("receipt_sha256").unwrap(), original.sha256());
    }
}

#[tokio::test]
#[ignore = "requires owned CHRONO_TEST_MONGODB_URI replica set"]
async fn managed_retirement_revision_finish_allows_next_background_poll() {
    let f = fixture(false).await;
    let claimed = f.store.claim().await.unwrap().unwrap();
    let receipt = f.tip().await;
    super::super::retirement::retire_claimed(&f.store, &f.provider, &f.image, claimed.clone())
        .await
        .unwrap();
    let updated = f.store.find(&claimed.id).await.unwrap().unwrap();
    assert!(updated.retiring && !updated.retired);
    assert_eq!(updated.revision, claimed.revision + 1);
    assert_eq!(updated.applied_revision, updated.revision);
    assert!(updated.lease.is_none());
    f.next_poll().await;
    f.assert_original_receipt(&receipt).await;
    let head = f.session.current().await.unwrap();
    assert_eq!(
        head.active_effects, 2,
        "retire and finish permits remain unresolved"
    );
    assert!(f
        .provider
        .calls
        .lock()
        .unwrap()
        .contains(&"resize:0".into()));
    f.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires owned CHRONO_TEST_MONGODB_URI replica set"]
async fn managed_tombstone_allows_next_background_poll_with_or_without_revision_change() {
    for already_retiring in [false, true] {
        let f = fixture(already_retiring).await;
        f.provider.retired.store(true, Ordering::SeqCst);
        let claimed = f.store.claim().await.unwrap().unwrap();
        let receipt = f.tip().await;
        super::super::retirement::retire_claimed(&f.store, &f.provider, &f.image, claimed.clone())
            .await
            .unwrap();
        let tombstone = f.store.find(&claimed.id).await.unwrap().unwrap();
        assert!(tombstone.retired && tombstone.lease.is_none());
        assert_eq!(
            tombstone.revision,
            claimed.revision + i64::from(!already_retiring)
        );
        f.next_poll().await;
        f.assert_original_receipt(&receipt).await;
        assert_eq!(
            f.session.current().await.unwrap().active_effects,
            if already_retiring { 1 } else { 2 }
        );
        f.db.drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires owned CHRONO_TEST_MONGODB_URI replica set"]
async fn managed_retirement_rejected_cas_does_not_release_or_advance_the_lane() {
    let f = fixture(false).await;
    let claimed = f.store.claim().await.unwrap().unwrap();
    let receipt = f.tip().await;
    for changed_revision in [false, true] {
        let mut foreign = claimed.clone();
        if changed_revision {
            foreign.revision += 1;
        } else {
            foreign.lease = Some("foreign".into());
        }
        assert!(matches!(
            f.store.begin_retirement(&foreign).await,
            Err(WarmPoolError::Conflict)
        ));
        assert!(f.store.claim().await.is_err());
    }
    let updated = f.store.begin_retirement(&claimed).await.unwrap();
    assert!(matches!(
        f.store.finish(&claimed, true).await,
        Err(WarmPoolError::Conflict)
    ));
    let mut foreign = updated.clone();
    foreign.lease = Some("foreign".into());
    assert!(matches!(
        f.store.remove_retired(&foreign).await,
        Err(WarmPoolError::Conflict)
    ));
    assert!(f.store.claim().await.is_err());
    // A durable write without an observed store ACK is not terminal evidence.
    f.db.collection::<Document>("native_warm_pool_targets")
        .update_one(
            doc! {"_id":&updated.id},
            doc! {"$set":{"retired":true,"lease":mongodb::bson::Bson::Null}},
        )
        .await
        .unwrap();
    assert!(f.store.claim().await.is_err());
    assert_eq!(f.tip().await.sha256(), receipt.sha256());
    f.db.drop().await.unwrap();
}
