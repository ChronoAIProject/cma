use super::*;
#[path = "evidence_export_snapshot_tests.rs"]
mod evidence_export_snapshot_tests;
#[path = "evidence_export_tests.rs"]
mod evidence_export_tests;

#[path = "expiry_worker_tests.rs"]
mod expiry_worker;

#[path = "deletion_unknown_tests.rs"]
mod deletion_unknown;
#[path = "expiry_race_tests.rs"]
mod expiry_races;

#[path = "capture_tests.rs"]
mod capture;
#[path = "retirement_tests.rs"]
mod retirement;

#[path = "expiry_tests.rs"]
mod expiry;
#[path = "publication_tests.rs"]
mod publication;
use mongodb::options::{Acknowledgment, ReadConcernLevel};
use std::time::{Duration, SystemTime};

#[path = "commit_unknown_tests.rs"]
mod commit_unknown_tests;
#[path = "discovery_tests.rs"]
mod discovery_tests;
#[path = "erasure_tests.rs"]
mod erasure_tests;
#[path = "schema_tests.rs"]
mod schema_tests;
#[path = "startup_tests.rs"]
mod startup_tests;

fn schedule() -> HomeBackupSchedule {
    HomeBackupSchedule {
        id: "sandbox-fixture".into(),
        home: HomeBackupHome {
            mode: "full_home".into(),
            home_source_id: "source-fixture".into(),
            home_identity: "home-fixture".into(),
            source_reference: Some("volume-fixture".into()),
        },
        state: HomeBackupScheduleState::Active,
        next_due_at: layer2_core::rfc3339_at(SystemTime::now() - Duration::from_secs(60)),
        last_ready_at: None,
        last_ready_backup_id: None,
        lease: None,
        next_fence: 1,
        in_flight: None,
        revision: 1,
    }
}
fn backup(claim: &HomeBackupClaim) -> HomeBackup {
    let epoch = claim.schedule.lease.as_ref().unwrap().fence;
    HomeBackup {
        id: format!("hb-{}-{epoch}", claim.schedule.id),
        sandbox_id: claim.schedule.id.clone(),
        epoch,
        source: HomeBackupSource {
            home_source_id: claim.schedule.home.home_source_id.clone(),
            home_identity: claim.schedule.home.home_identity.clone(),
            runtime_generation: 1,
            runtime_version: "fixture-image".into(),
            boot_id: "fixture-boot".into(),
        },
        cut: HomeBackupCut {
            requested_at: layer2_core::now_rfc3339(),
            source_creation_time: None,
            ready_at: None,
        },
        storage: HomeBackupStorage {
            snapshot_name: format!("capture-{epoch}"),
            snapshot_class: "retain-fixture".into(),
            source_reference: None,
            restore_size_bytes: None,
        },
        custody: HomeBackupCustody {
            encryption_key_version_id: Some("key-version-fixture".into()),
            auth_key_ids: vec!["auth-version-fixture".into()],
        },
        retention: HomeBackupRetention {
            class: HomeBackupRetentionClass::Daily,
            expires_at: layer2_core::rfc3339_at(
                SystemTime::now() + Duration::from_secs(30 * 86400 - 60),
            ),
        },
        state: HomeBackupState::Requested,
        restore_pins: 0,
        lease: None,
        next_fence: 1,
        revision: 1,
        failure: None,
        erasure: None,
    }
}
fn standalone_backup(state: HomeBackupState) -> HomeBackup {
    let mut value = HomeBackup {
        id: "hb-sandbox-fixture-1".into(),
        sandbox_id: "sandbox-fixture".into(),
        epoch: 1,
        source: HomeBackupSource {
            home_source_id: "source-fixture".into(),
            home_identity: "home-fixture".into(),
            runtime_generation: 1,
            runtime_version: "fixture-image".into(),
            boot_id: "fixture-boot".into(),
        },
        cut: HomeBackupCut {
            requested_at: "2026-11-16T00:00:00Z".into(),
            source_creation_time: None,
            ready_at: None,
        },
        storage: HomeBackupStorage {
            snapshot_name: "capture-1".into(),
            snapshot_class: "retain-fixture".into(),
            source_reference: None,
            restore_size_bytes: None,
        },
        custody: HomeBackupCustody {
            encryption_key_version_id: Some("key-version-fixture".into()),
            auth_key_ids: vec!["auth-version-fixture".into()],
        },
        retention: HomeBackupRetention {
            class: HomeBackupRetentionClass::Daily,
            expires_at: "2026-11-17T00:00:00Z".into(),
        },
        state,
        restore_pins: 0,
        lease: None,
        next_fence: 1,
        revision: 1,
        failure: None,
        erasure: None,
    };
    match state {
        HomeBackupState::Failed => value.failure = Some(HomeBackupFailure::CopyTimedOut),
        HomeBackupState::Ready => {
            value.cut.source_creation_time = Some(value.cut.requested_at.clone());
            value.cut.ready_at = Some(value.cut.requested_at.clone());
            value.storage.source_reference = Some("handle-fixture".into());
        }
        HomeBackupState::Deleted => {
            value.erasure = Some(HomeBackupErasure {
                erased_at: "2026-11-16T00:00:01Z".into(),
                receipt: "receipt-fixture".into(),
            });
        }
        _ => {}
    }
    value
}
fn holder() -> String {
    format!("fixture-deployment:{}", uuid::Uuid::new_v4())
}
fn until() -> String {
    layer2_core::rfc3339_at(SystemTime::now() + Duration::from_secs(120))
}

#[test]
fn strict_bson_reader_preserves_provider_names_and_rejects_invalid_evidence() {
    let original = schedule();
    let row = schedule_document(&original).unwrap();
    assert_eq!(read_schedule(row.clone()).unwrap(), original);
    assert_eq!(
        row.get_document("home")
            .unwrap()
            .get_str("pvc_uid")
            .unwrap(),
        "source-fixture"
    );
    let mut unknown = row.clone();
    unknown.insert("owner", "unexpected");
    assert!(read_schedule(unknown).is_err());
    let mut missing = row.clone();
    missing.remove("revision");
    assert!(read_schedule(missing).is_err());
    let mut bad_time = row.clone();
    bad_time.insert("next_due_at", "2026-11-16T00:00:00+08:00");
    assert!(read_schedule(bad_time).is_err());
    let mut bad_counter = row;
    bad_counter.insert("next_fence", -1_i64);
    assert!(read_schedule(bad_counter).is_err());
    assert!(codec::signed(u64::MAX).is_err());

    for state in [
        HomeBackupState::Requested,
        HomeBackupState::Creating,
        HomeBackupState::Ready,
        HomeBackupState::Failed,
        HomeBackupState::Expiring,
        HomeBackupState::Deleted,
    ] {
        let value = standalone_backup(state);
        let row = backup_document(&value).unwrap();
        assert_eq!(read_backup(row).unwrap(), value, "state {state:?}");
    }
    let mut maximum = backup_document(&standalone_backup(HomeBackupState::Creating)).unwrap();
    maximum.insert("next_fence", i64::MAX);
    let decoded = read_backup(maximum).unwrap();
    assert_eq!(decoded.next_fence, i64::MAX as u64);
    let mut leased = standalone_backup(HomeBackupState::Creating);
    leased.next_fence = i64::MAX as u64;
    leased.lease = Some(HomeBackupLease {
        holder: holder(),
        token: uuid::Uuid::new_v4().to_string(),
        fence: i64::MAX as u64 - 1,
        expires_at: until(),
    });
    let row = backup_document(&leased).unwrap();
    assert_eq!(
        row.get_document("lease").unwrap().get_i64("fence").unwrap(),
        i64::MAX - 1
    );
    assert_eq!(read_backup(row.clone()).unwrap(), leased);
    for bad in [
        mongodb::bson::Bson::Int64(-1),
        mongodb::bson::Bson::Double(9.223_372_036_854_776e18),
    ] {
        let mut bad_row = row.clone();
        let mut bad_lease = bad_row.get_document("lease").unwrap().clone();
        bad_lease.insert("fence", bad);
        bad_row.insert("lease", bad_lease);
        assert!(read_backup(bad_row).is_err());
    }
    leased.lease.as_mut().unwrap().fence = i64::MAX as u64 + 1;
    assert!(backup_document(&leased).is_err());
    let mut negative = backup_document(&standalone_backup(HomeBackupState::Creating)).unwrap();
    negative.insert("next_fence", -1_i64);
    assert!(read_backup(negative).is_err());
    let mut floating = backup_document(&standalone_backup(HomeBackupState::Creating)).unwrap();
    floating.insert("next_fence", mongodb::bson::Bson::Double(9.22e18));
    assert!(read_backup(floating).is_err());
}

struct Fixture {
    client: Client,
    database: String,
    store: MongoHomeBackupStore,
}
impl Fixture {
    async fn new() -> Self {
        let uri = std::env::var("CHRONO_TEST_MONGODB_URI").expect("isolated Mongo replica-set URI");
        let client = Client::with_uri_str(&uri).await.unwrap();
        let database = format!("home_backup_contract_{}", uuid::Uuid::new_v4().simple());
        crate::store::migrations::home_backup_catalog::prepare_for_test(
            &client.database(&database),
        )
        .await;
        let store = MongoHomeBackupStore::open(&client, &database)
            .await
            .unwrap();
        Self {
            client,
            database,
            store,
        }
    }
    async fn cleanup(self) {
        self.client.database(&self.database).drop().await.unwrap();
    }
}

async fn complete_previous_ready(store: &MongoHomeBackupStore) -> HomeBackup {
    store.ensure_schedule(&schedule()).await.unwrap();
    let now = layer2_core::now_rfc3339();
    let claim = store
        .claim_due(&now, &holder(), &until())
        .await
        .unwrap()
        .unwrap();
    let planned = store
        .plan_capture(&claim, &backup(&claim), &now)
        .await
        .unwrap();
    let mut creating = planned.backup.clone().unwrap();
    creating.state = HomeBackupState::Creating;
    let mut progress = planned.schedule.clone();
    progress.in_flight.as_mut().unwrap().phase = HomeBackupPhase::Creating;
    let claim = store
        .save_capture(&planned, &progress, &creating, &now)
        .await
        .unwrap();
    let mut ready = claim.backup.clone().unwrap();
    ready.state = HomeBackupState::Ready;
    ready.cut.source_creation_time = Some(ready.cut.requested_at.clone());
    ready.cut.ready_at = Some(ready.cut.requested_at.clone());
    ready.storage.source_reference = Some("previous-handle-fixture".into());
    let mut completed = claim.schedule.clone();
    completed.in_flight = None;
    completed.last_ready_at = ready.cut.ready_at.clone();
    completed.last_ready_backup_id = Some(ready.id.clone());
    store
        .save_capture(&claim, &completed, &ready, &now)
        .await
        .unwrap();
    ready
}
