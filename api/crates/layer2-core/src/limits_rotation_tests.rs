//! A runtime replacement never asks for another live-session slot.

use std::num::NonZeroU32;
use std::sync::Arc;

use crate::store::SessionStore;
use crate::test_support::{
    await_settled, service_on_version, service_on_version_with_settings, spec, user, FakeAdapter,
    FakeStore,
};
use crate::{PlatformSettings, SessionStatus};

#[tokio::test]
async fn rotation_at_the_limit_replaces_one_runtime_without_raising_the_count() {
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let owner = user("owner");
    let old = service_on_version(Arc::clone(&store), Arc::clone(&adapter), "runtime:1");
    let created = old
        .create_session(&owner, spec())
        .await
        .expect("session creates before the limit is set");
    await_settled(&old, &owner, &created.id).await;
    drop(old);

    let settings = PlatformSettings {
        live_sessions: NonZeroU32::new(1),
        ..PlatformSettings::default()
    };
    let replacement = service_on_version_with_settings(
        Arc::clone(&store),
        Arc::clone(&adapter),
        "runtime:2",
        settings,
    );
    assert_eq!(store.count_live_for(&owner.subject).await.unwrap(), 1);

    let reconciled = replacement.reconcile().await;
    assert_eq!(reconciled.rotated_sessions, 1);
    for _ in 0..400 {
        let status = store
            .get(&created.id)
            .await
            .unwrap()
            .unwrap()
            .session
            .status;
        if status == SessionStatus::Ready {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }

    let record = store.get(&created.id).await.unwrap().unwrap();
    assert_eq!(record.session.status, SessionStatus::Ready);
    assert_eq!(record.session.rotations, 1);
    assert_eq!(store.count_live_for(&owner.subject).await.unwrap(), 1);
    assert_eq!(adapter.running_count().await, 1);
}
