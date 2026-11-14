use super::*;
use crate::sandbox::store::{SandboxSessionStore, SandboxStore};
use crate::sandbox::{runtime_desktop_activity_snapshot, try_acquire_runtime_migration_guard};
use std::sync::atomic::Ordering;

#[path = "desktop_migration_fixture.rs"]
mod fixture;
use fixture::Fixture;

#[tokio::test]
async fn held_native_open_blocks_migration_before_session_write_and_cancellation_advances_history()
{
    let fixture = Fixture::new().await;
    let baseline = runtime_desktop_activity_snapshot(&fixture.id).unwrap();
    let opening = fixture.start();
    fixture.entered().await;
    assert!(!fixture
        .store
        .has_open_sandbox_session(&fixture.id, &crate::now_rfc3339())
        .await
        .unwrap());
    assert!(try_acquire_runtime_migration_guard(&fixture.id).is_err());
    assert!(try_acquire_runtime_migration_guard(&SandboxId::generate()).is_ok());
    assert_eq!(
        runtime_desktop_activity_snapshot(&fixture.id)
            .unwrap()
            .opening_or_open,
        1
    );
    opening.abort();
    assert!(matches!(opening.await, Err(error) if error.is_cancelled()));
    let after = runtime_desktop_activity_snapshot(&fixture.id).unwrap();
    assert_eq!(after.opening_or_open, 0);
    assert_eq!(after.activity_epoch, baseline.activity_epoch + 2);
    assert_eq!(fixture.probe.sockets.load(Ordering::SeqCst), 0);
    assert!(try_acquire_runtime_migration_guard(&fixture.id).is_ok());
}

#[tokio::test]
async fn connection_close_and_drop_release_shared_guard_only_after_native_socket() {
    let fixture = Fixture::new().await;
    let baseline = runtime_desktop_activity_snapshot(&fixture.id).unwrap();
    let opening = fixture.start();
    fixture.entered().await;
    fixture.probe.release.notify_one();
    let mut connection = opening.await.unwrap().unwrap();
    assert_eq!(fixture.probe.sockets.load(Ordering::SeqCst), 1);
    assert!(try_acquire_runtime_migration_guard(&fixture.id).is_err());
    assert!(fixture
        .store
        .has_open_sandbox_session(&fixture.id, &crate::now_rfc3339())
        .await
        .unwrap());
    fixture.service.close_connection(&mut connection).await;
    assert_eq!(fixture.probe.sockets.load(Ordering::SeqCst), 0);
    assert_eq!(
        runtime_desktop_activity_snapshot(&fixture.id)
            .unwrap()
            .activity_epoch,
        baseline.activity_epoch + 3
    );
    assert!(!fixture
        .store
        .has_open_sandbox_session(&fixture.id, &crate::now_rfc3339())
        .await
        .unwrap());
    assert!(try_acquire_runtime_migration_guard(&fixture.id).is_ok());

    let opening = fixture.start();
    fixture.entered().await;
    fixture.probe.release.notify_one();
    let connection = opening.await.unwrap().unwrap();
    drop(connection);
    assert_eq!(fixture.probe.sockets.load(Ordering::SeqCst), 0);
    assert_eq!(
        runtime_desktop_activity_snapshot(&fixture.id)
            .unwrap()
            .opening_or_open,
        0
    );
    assert!(try_acquire_runtime_migration_guard(&fixture.id).is_ok());
    // Drop cannot synchronously close persistence: admission must still check
    // the durable session, in addition to this local desktop exclusion.
    assert!(fixture
        .store
        .has_open_sandbox_session(&fixture.id, &crate::now_rfc3339())
        .await
        .unwrap());
}

#[tokio::test]
async fn failed_native_open_is_not_erased_from_desktop_history() {
    let fixture = Fixture::new().await;
    let before = runtime_desktop_activity_snapshot(&fixture.id).unwrap();
    fixture.probe.fail.store(true, Ordering::SeqCst);
    let opening = fixture.start();
    fixture.entered().await;
    fixture.probe.release.notify_one();
    assert!(matches!(
        opening.await.unwrap(),
        Err(DesktopError::RuntimeUnavailable)
    ));
    let after = runtime_desktop_activity_snapshot(&fixture.id).unwrap();
    assert_eq!(after.activity_epoch, before.activity_epoch + 2);
    assert_eq!(after.opening_or_open, 0);
    assert!(!fixture
        .store
        .has_open_sandbox_session(&fixture.id, &crate::now_rfc3339())
        .await
        .unwrap());
    assert!(try_acquire_runtime_migration_guard(&fixture.id).is_ok());
}

#[tokio::test]
async fn migration_guard_blocks_new_native_opens_from_distinct_service_instances() {
    let fixture = Fixture::new().await;
    let second = DesktopService::new(fixture.service.reads.clone(), fixture.probe.clone());
    let before = runtime_desktop_activity_snapshot(&fixture.id).unwrap();
    let migration = try_acquire_runtime_migration_guard(&fixture.id).unwrap();
    for service in [&*fixture.service, &second] {
        assert!(matches!(
            service.open(&fixture.owner, &fixture.agent_id).await,
            Err(DesktopError::Busy)
        ));
    }
    assert_eq!(fixture.probe.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        runtime_desktop_activity_snapshot(&fixture.id).unwrap(),
        before
    );
    drop(migration);
}

#[tokio::test]
async fn desktop_revalidates_active_state_and_exact_runtime_after_earlier_authorization() {
    for paused in [false, true] {
        let fixture = Fixture::new().await;
        let (id, runtime) = fixture
            .service
            .authorize(&fixture.owner, &fixture.agent_id)
            .await
            .unwrap();
        let migration = try_acquire_runtime_migration_guard(&id).unwrap();
        let mut current = fixture.store.get_sandbox(&id).await.unwrap().unwrap();
        let revision = current.revision;
        if paused {
            current.status = SandboxStatus::Paused;
        } else {
            current.runtime.as_mut().unwrap().runtime_id = "successor-runtime".into();
        }
        current.revision += 1;
        fixture.store.save_sandbox(revision, current).await.unwrap();
        drop(migration);
        // Resume the real open path from its previously successful authorization.
        assert!(matches!(
            fixture
                .service
                .open_legacy_with_binding(&fixture.owner, &fixture.agent_id, id, runtime)
                .await,
            Err(DesktopError::Unavailable)
        ));
        assert_eq!(fixture.probe.calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            runtime_desktop_activity_snapshot(&fixture.id)
                .unwrap()
                .opening_or_open,
            0
        );
    }
}
