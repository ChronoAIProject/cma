//! Exact replay remains available only under current resource authority.
use super::*;
use layer2_core::admission::AdmissionControlStore;
use layer2_core::budget::{
    BudgetError, CloudObservationInput, ObservationError, PlatformAdmissionState,
    PlatformSpendLedger, PlatformSpendState,
};

async fn revive(
    f: &Fixture,
    key: &str,
    revision: u64,
) -> Result<SandboxControlRecord, SandboxControlServiceError> {
    f.service
        .admit(
            &context("recipient"),
            "fixture",
            SandboxControlSubmission {
                sandbox_id: f.parent.id.clone(),
                command: SandboxControl::Revive,
                expected_revision: revision,
                idempotency_key: key.into(),
            },
        )
        .await
}

async fn stop(client: &Client, name: &str) {
    crate::store::admission_control::MongoAdmissionControlStore::open(client, name)
        .await
        .unwrap()
        .change(
            0,
            &SubjectId::new("operator"),
            "contain new work",
            true,
            100,
        )
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn operator_stop_replays_exact_receipt_but_refuses_new_and_changed_keys() {
    let (client, name) = database("operator_control_replay").await;
    let f = Fixture::new(&client, &name).await;
    f.service.attach_platform_spend(f.store.clone());
    let admitted = revive(&f, "retained", f.parent.revision).await.unwrap();
    stop(&client, &name).await;
    let before = f.documents().await;
    assert_eq!(
        revive(&f, "retained", f.parent.revision).await.unwrap(),
        admitted
    );
    assert!(matches!(
        revive(&f, "fresh", f.parent.revision).await,
        Err(SandboxControlServiceError::AdmissionPaused)
    ));
    assert!(matches!(
        revive(&f, "retained", f.parent.revision + 1).await,
        Err(SandboxControlServiceError::Store(
            SandboxControlError::IdempotencyConflict
        ))
    ));
    // Policy-fence counters may advance, but receipts, lifecycle and history do not.
    let after = f.documents().await;
    assert_eq!(before.1, after.1);
    assert_eq!(before.2, after.2);
    assert_eq!(before.3, after.3);
    assert_eq!(
        f.store.get_sandbox(&f.parent.id).await.unwrap().unwrap(),
        f.parent
    );
    client.database(&name).drop().await.unwrap();
}

struct RevokeDuringRead {
    store: Arc<MongoSandboxStore>,
    access: Arc<ResourceAccessService>,
    revoke: std::sync::Mutex<Option<ShareCommand>>,
}

#[async_trait::async_trait]
impl PlatformSpendLedger for RevokeDuringRead {
    async fn admission_state(&self, now: u64) -> Result<PlatformAdmissionState, BudgetError> {
        let revoke = self.revoke.lock().unwrap().take().unwrap();
        self.access
            .change_share(&context("owner"), Some("fixture"), revoke)
            .await
            .unwrap();
        self.store.admission_state(now).await
    }
    async fn spend_state(&self, _: u64) -> Result<PlatformSpendState, BudgetError> {
        unreachable!("receipt recovery reads only admission state")
    }
    async fn record_cloud_observation(
        &self,
        _: CloudObservationInput,
        _: u64,
    ) -> Result<PlatformSpendState, ObservationError> {
        unreachable!("receipt recovery records no observation")
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn operator_stop_replay_rechecks_revoked_share_after_admission_read() {
    let (client, name) = database("operator_control_revoke").await;
    let f = Fixture::new(&client, &name).await;
    revive(&f, "retained", f.parent.revision).await.unwrap();
    stop(&client, &name).await;
    let before = f.documents().await;
    f.service.attach_platform_spend(Arc::new(RevokeDuringRead {
        store: f.store.clone(),
        access: f.access.clone(),
        revoke: std::sync::Mutex::new(Some(ShareCommand {
            selector: f.share.share.selector.clone(),
            change: ShareChange::Revoke {
                share_id: f.share.share.id.clone(),
            },
            expected_share_revision: Some(f.share.share.revision),
            idempotency_key: "revoke-during-admission".into(),
        })),
    }));
    assert!(matches!(
        revive(&f, "retained", f.parent.revision).await,
        Err(SandboxControlServiceError::Store(
            SandboxControlError::Conflict
        ))
    ));
    let after = f.documents().await;
    assert_eq!(
        after.0.get_i64("resource_share_guard").unwrap(),
        before.0.get_i64("resource_share_guard").unwrap() + 1,
        "only the share revocation may fence the Sandbox"
    );
    assert_eq!(after.1, before.1);
    assert_eq!(after.2, before.2);
    assert_eq!(after.3, before.3);
    assert_eq!(
        f.store.get_sandbox(&f.parent.id).await.unwrap().unwrap(),
        f.parent
    );
    client.database(&name).drop().await.unwrap();
}
