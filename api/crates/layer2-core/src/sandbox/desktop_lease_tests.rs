use super::*;
use crate::sandbox::{has_open_desktop_session, register_open_desktop_session};

fn historical_connection(sandbox_id: SandboxId) -> DesktopConnection {
    register_open_desktop_session(&sandbox_id);
    DesktopConnection {
        connection: None,
        sandbox_id,
        session_id: "retained-session".into(),
        runtime_id: "retained-runtime".into(),
        _lease: Some(Arc::new(Mutex::new(())).try_lock_owned().unwrap()),
        migration_guard: None,
        owned: None,
    }
}

#[tokio::test]
async fn retained_connection_drop_releases_local_inactivity_marker_and_lease() {
    let id = SandboxId("retained-desktop-drop".into());
    let connection = historical_connection(id.clone());
    assert!(has_open_desktop_session(&id));
    let lease = tokio::sync::OwnedMutexGuard::mutex(connection._lease.as_ref().unwrap()).clone();
    assert!(lease.try_lock().is_err());
    drop(connection);
    assert!(!has_open_desktop_session(&id));
    assert!(lease.try_lock().is_ok());
}
