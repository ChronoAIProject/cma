use super::*;
use crate::home_backup_expiry::{DeletionProvider, ExpiryWorker};
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;

struct Provider {
    calls: Mutex<Vec<(&'static str, String)>>,
    delete_started: Notify,
    release_delete: Notify,
}

#[async_trait::async_trait]
impl DeletionProvider for Provider {
    async fn delete_copy(&self, value: &HomeBackup) -> Result<(), HomeBackupError> {
        assert_eq!(value.state, HomeBackupState::Expiring);
        assert!(value.lease.is_some());
        self.calls
            .lock()
            .unwrap()
            .push(("delete", value.id.clone()));
        self.delete_started.notify_one();
        self.release_delete.notified().await;
        Ok(())
    }

    async fn observe_deletion(
        &self,
        value: &HomeBackup,
    ) -> Result<HomeBackupDeletionObservation, HomeBackupError> {
        self.calls
            .lock()
            .unwrap()
            .push(("observe", value.id.clone()));
        Ok(expiry::proof(value))
    }
}

#[tokio::test]
#[ignore = "requires a real isolated Mongo replica set"]
async fn closed_admission_recovers_only_expiring_and_drains_original_deletion_before_shutdown() {
    let fixture = Fixture::new().await;
    let ready = expiry::expired();
    let mut admitted = ready.clone();
    admitted.id = "hb-sandbox-fixture-2".into();
    admitted.epoch = 2;
    admitted.state = HomeBackupState::Expiring;
    fixture
        .store
        .backups
        .insert_many([
            backup_document(&ready).unwrap(),
            backup_document(&admitted).unwrap(),
        ])
        .await
        .unwrap();
    let provider = Arc::new(Provider {
        calls: Mutex::new(Vec::new()),
        delete_started: Notify::new(),
        release_delete: Notify::new(),
    });
    let worker = ExpiryWorker::new(
        false,
        Arc::new(fixture.store.clone()),
        Some(provider.clone()),
        None,
        "recovery-fixture",
    )
    .unwrap();
    let activity = crate::worker_supervision::Workers::new().get("home_backup_expiry");
    let (stop, signal) = tokio::sync::watch::channel(false);
    let mut task = worker.spawn_supervised(signal, activity.clone());
    let reached =
        tokio::time::timeout(Duration::from_secs(5), provider.delete_started.notified()).await;
    stop.send(true).unwrap();
    if reached.is_ok() {
        assert_eq!(
            fixture.store.get_backup(&ready.id).await.unwrap(),
            Some(ready.clone())
        );
        assert_eq!(
            fixture
                .store
                .get_backup(&admitted.id)
                .await
                .unwrap()
                .unwrap()
                .state,
            HomeBackupState::Expiring
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut task)
                .await
                .is_err(),
            "shutdown must drain the original in-flight deletion"
        );
    }
    provider.release_delete.notify_one();
    tokio::time::timeout(Duration::from_secs(5), &mut task)
        .await
        .unwrap()
        .unwrap();
    reached.expect("qualified fixture must recover the admitted deletion");
    assert_eq!(
        *provider.calls.lock().unwrap(),
        vec![
            ("delete", admitted.id.clone()),
            ("observe", admitted.id.clone())
        ]
    );
    let deleted = fixture
        .store
        .get_backup(&admitted.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(deleted.state, HomeBackupState::Deleted);
    assert_eq!(deleted.retention, admitted.retention);
    assert_eq!(
        deleted.erasure.unwrap().receipt,
        "independent-absence-fixture"
    );
    assert_eq!(
        fixture.store.get_backup(&ready.id).await.unwrap(),
        Some(ready)
    );
    assert_eq!(activity.snapshot().state, "stopped");
    fixture.cleanup().await;
}
