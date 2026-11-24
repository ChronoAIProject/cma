//! Independent Mongo clients exercise shared authority, including maintenance.
use super::*;
use crate::store::sandbox_maintenance::mongo_integration_tests::active_sandbox;
use layer2_core::sandbox::store::{
    InactivityPauseCause, SandboxInactivityPauseRequest, SandboxMaintenanceOutcome,
    SandboxMaintenanceStore, SandboxSessionStore, SandboxStore,
};
use mongodb::{bson::doc, Client};

pub(super) struct Fixture {
    pub(super) left: MongoSandboxStore,
    pub(super) right: MongoSandboxStore,
    pub(super) db: mongodb::Database,
    pub(super) request: DesktopSessionRequest,
}

impl Fixture {
    pub(super) async fn new() -> Self {
        let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
        let left = Client::with_uri_str(&uri).await.unwrap();
        let right = Client::with_uri_str(&uri).await.unwrap();
        Self::with_clients(left, right).await
    }

    async fn with_clients(left: Client, right: Client) -> Self {
        let name = format!("cma_desktop_replicas_{}", uuid::Uuid::new_v4().simple());
        let db = left.database(&name);
        let left = MongoSandboxStore::open(&left, &name).await.unwrap();
        let right = MongoSandboxStore::open(&right, &name).await.unwrap();
        let mut sandbox = active_sandbox(&format!("sbx_desktop_{}", uuid::Uuid::new_v4().simple()));
        sandbox.profile.content.environment_variables.push(
            layer2_core::profiles::EnvironmentVariable {
                name: "CMA_MACHINE_INCLUDED".into(),
                value: "1".into(),
            },
        );
        sandbox.profile.digest =
            layer2_core::profiles::content_digest(&sandbox.profile.content).unwrap();
        left.create_sandbox(sandbox.clone()).await.unwrap();
        Self {
            left,
            right,
            db,
            request: DesktopSessionRequest {
                sandbox_id: sandbox.id,
                session_id: "desktop-first".into(),
                runtime: sandbox.runtime.unwrap(),
            },
        }
    }

    fn peer_request(&self) -> DesktopSessionRequest {
        DesktopSessionRequest {
            session_id: "desktop-second".into(),
            ..self.request.clone()
        }
    }

    fn pause(&self) -> SandboxInactivityPauseRequest {
        SandboxInactivityPauseRequest {
            sandbox_id: self.request.sandbox_id.clone(),
            requested_at: layer2_core::now_rfc3339(),
            pause_after_seconds: 0,
            auto_pause_enabled: true,
            expected_policy_revision: None,
            cause: InactivityPauseCause::PlatformSpendStop,
        }
    }
}

mod command_tests;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn independent_clients_admit_one_sandbox_desktop_and_preserve_epoch_after_release() {
    let f = Fixture::new().await;
    let peer = f.peer_request();
    let (left, right) = tokio::join!(
        f.left.acquire_exclusive_sandbox_session(&f.request),
        f.right.acquire_exclusive_sandbox_session(&peer),
    );
    assert_eq!(usize::from(left.is_ok()) + usize::from(right.is_ok()), 1);
    let (first, loser) = match (left, right) {
        (Ok(authority), Err(error)) | (Err(error), Ok(authority)) => (authority, error),
        _ => panic!("exactly one shared claim must win"),
    };
    assert!(matches!(loser, SandboxStoreError::Conflict));
    assert!(f
        .right
        .has_open_sandbox_session(&f.request.sandbox_id, &stamp(first.issued_at_ms))
        .await
        .unwrap());
    assert!(matches!(
        f.right.schedule_inactivity_pause(f.pause()).await.unwrap(),
        SandboxMaintenanceOutcome::NotEligible
    ));
    f.right
        .release_exclusive_sandbox_session(&first)
        .await
        .unwrap();
    // Ordinary aggregate updates must preserve the permanent epoch sidecar.
    // Otherwise maintenance/metadata changes could resurrect an old receiver token.
    let mut sandbox = f
        .left
        .get_sandbox(&f.request.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    let revision = sandbox.revision;
    sandbox.revision += 1;
    sandbox.name = "Desktop after metadata update".into();
    f.right.save_sandbox(revision, sandbox).await.unwrap();
    let next = f
        .left
        .acquire_exclusive_sandbox_session(&DesktopSessionRequest {
            session_id: "desktop-third".into(),
            ..peer
        })
        .await
        .unwrap();
    assert!(next.epoch > first.epoch);
    assert_ne!(next.session_id, first.session_id);
    // Release is token-exact even when delayed until after a successor opens.
    f.right
        .release_exclusive_sandbox_session(&first)
        .await
        .unwrap();
    assert!(matches!(
        f.left.acquire_exclusive_sandbox_session(&f.request).await,
        Err(SandboxStoreError::Conflict)
    ));
    f.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn expired_claim_cannot_renew_or_release_a_successor_on_another_client() {
    let f = Fixture::new().await;
    let first = f
        .left
        .acquire_exclusive_sandbox_session(&f.request)
        .await
        .unwrap();
    f.left
        .sandbox_sessions
        .update_one(
            exact(&first),
            doc! { "$set": {
                "lease_expires_at": "2000-01-01T00:00:00Z", "expires_at_ms": 0_i64,
            } },
        )
        .await
        .unwrap();
    assert!(matches!(
        f.left
            .renew_exclusive_sandbox_session(&f.request, &first)
            .await,
        Err(SandboxStoreError::Conflict)
    ));
    let peer = f.peer_request();
    let next = f
        .right
        .acquire_exclusive_sandbox_session(&peer)
        .await
        .unwrap();
    assert!(next.epoch > first.epoch);
    f.left
        .release_exclusive_sandbox_session(&first)
        .await
        .unwrap();
    let renewed = f
        .right
        .renew_exclusive_sandbox_session(&peer, &next)
        .await
        .unwrap();
    assert_eq!(renewed.epoch, next.epoch);
    assert!(renewed.expires_at_ms >= next.expires_at_ms);
    assert!(matches!(
        f.left
            .renew_exclusive_sandbox_session(&f.request, &first)
            .await,
        Err(SandboxStoreError::Conflict)
    ));
    f.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn competing_maintenance_and_open_cannot_both_commit_first_batch() {
    race_maintenance_and_open().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn competing_maintenance_and_open_cannot_both_commit_second_batch() {
    race_maintenance_and_open().await;
}

// Each batch runs six races in fresh databases within the per-test budget.
async fn race_maintenance_and_open() {
    for _ in 0..6 {
        let f = Fixture::new().await;
        let (opened, paused) = tokio::join!(
            f.left.acquire_exclusive_sandbox_session(&f.request),
            f.right.schedule_inactivity_pause(f.pause()),
        );
        match (opened, paused.unwrap()) {
            (Ok(_), SandboxMaintenanceOutcome::NotEligible) => {}
            (Err(SandboxStoreError::Conflict), SandboxMaintenanceOutcome::Scheduled) => {}
            _ => panic!("desktop and maintenance must serialize"),
        }
        f.db.drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn runtime_change_and_store_failure_cannot_mint_desktop_authority() {
    let f = Fixture::new().await;
    let mut foreign = f.request.clone();
    foreign.runtime.runtime_id = "replacement-runtime".into();
    assert!(matches!(
        f.left.acquire_exclusive_sandbox_session(&foreign).await,
        Err(SandboxStoreError::Conflict)
    ));
    assert_eq!(
        f.left
            .sandbox_sessions
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    f.db.run_command(doc! { "collMod": "sandbox_sessions",
        "validator": { "$expr": { "$eq": [1, 0] } }, "validationLevel": "strict",
    })
    .await
    .unwrap();
    assert!(matches!(
        f.right.acquire_exclusive_sandbox_session(&f.request).await,
        Err(SandboxStoreError::Unavailable(_))
    ));
    assert_eq!(
        f.left
            .sandbox_sessions
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    f.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn strict_inactivity_and_exclusive_desktop_cannot_both_commit() {
    use layer2_core::PlatformSettingsStore;

    for _ in 0..6 {
        let f = Fixture::new().await;
        let settings =
            crate::store::settings::MongoPlatformSettingsStore::open(f.db.client(), f.db.name())
                .await
                .unwrap();
        let current = settings.read().await.unwrap();
        let mut enabled = current.settings;
        enabled.inactivity.auto_pause_enabled = true;
        enabled.inactivity.grace_started_at = Some("2026-09-05T00:00:00Z".into());
        let policy = settings.replace(current.revision, &enabled).await.unwrap();
        let strict = f
            .right
            .clone()
            .with_strict_inactivity_session_evidence(true);
        let request = SandboxInactivityPauseRequest {
            cause: InactivityPauseCause::Inactivity,
            expected_policy_revision: Some(policy.revision),
            pause_after_seconds: policy.settings.inactivity.pause_after_seconds,
            ..f.pause()
        };
        assert_eq!(
            strict
                .preview_inactivity_pause(request.clone())
                .await
                .unwrap(),
            SandboxMaintenanceOutcome::Scheduled
        );
        let (opened, paused) = tokio::join!(
            f.left.acquire_exclusive_sandbox_session(&f.request),
            strict.schedule_inactivity_pause(request),
        );
        match (opened, paused.unwrap()) {
            (Ok(_), SandboxMaintenanceOutcome::NotEligible) => {}
            (Err(SandboxStoreError::Conflict), SandboxMaintenanceOutcome::Scheduled) => {}
            result => panic!(
                "desktop claim and strict inactivity did not serialize: {:?}",
                (result.0.is_ok(), result.1)
            ),
        }
        f.db.drop().await.unwrap();
    }
}
