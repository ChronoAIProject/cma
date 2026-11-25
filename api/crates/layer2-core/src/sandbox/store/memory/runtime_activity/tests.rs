use super::*;
use crate::sandbox::lifecycle_test_support::NOW;
use crate::sandbox::{SandboxRotationIdentity, TypedFailure};

#[test]
fn parked_failed_update_can_earn_fresh_proof_but_pending_update_cannot() {
    let mut sandbox = crate::sandbox::runtime_update_test_support::native();
    sandbox
        .request_runtime_update("image-next".into(), false, NOW.into())
        .unwrap();
    let identity = SandboxRotationIdentity::capture(&sandbox, &[]).unwrap();
    sandbox
        .begin_image_rotation("image-next".into(), identity, NOW.into())
        .unwrap();
    let store = InMemorySandboxStore::default();
    let id = sandbox.id.clone();
    let at_ms = i64::try_from(crate::seconds_from_rfc3339(NOW).unwrap()).unwrap() * 1000;
    // Projection unit fixture: model transitions construct authority, without
    // using a generic persistence API to bypass store admission.
    store.lock().sandboxes.insert(id.clone(), sandbox.clone());
    assert!(store
        .runtime_activity_quiet_proof_fixture(&id, at_ms)
        .is_err());
    let fence = sandbox
        .work
        .claim("fixture".into(), NOW, "2026-09-05T01:00:00Z".into())
        .unwrap();
    sandbox
        .park_failed_runtime_update(
            TypedFailure {
                class: "runtime_update_failed".into(),
                detail: "fixture".into(),
            },
            "fixture",
            fence,
            NOW.into(),
        )
        .unwrap();
    store.lock().sandboxes.insert(id.clone(), sandbox.clone());
    let proof = store
        .runtime_activity_quiet_proof_fixture(&id, at_ms)
        .unwrap();
    validate_in_memory(&store.lock(), &id, &proof, at_ms).unwrap();
    sandbox
        .request_runtime_update("image-next".into(), false, NOW.into())
        .unwrap();
    store.lock().sandboxes.insert(id.clone(), sandbox);
    assert!(validate_in_memory(&store.lock(), &id, &proof, at_ms).is_err());
    assert!(store
        .runtime_activity_quiet_proof_fixture(&id, at_ms)
        .is_err());
}

#[test]
fn memory_snapshot_refuses_stale_revision_and_current_session() {
    let sandbox = crate::sandbox::runtime_update_test_support::native();
    let id = sandbox.id.clone();
    let store = InMemorySandboxStore::default();
    store.lock().sandboxes.insert(id.clone(), sandbox);
    let at_ms = i64::try_from(crate::seconds_from_rfc3339(NOW).unwrap()).unwrap() * 1000;
    let proof = store
        .runtime_activity_quiet_proof_fixture(&id, at_ms)
        .unwrap();
    validate_in_memory(&store.lock(), &id, &proof, at_ms).unwrap();
    store.lock().sandboxes.get_mut(&id).unwrap().revision += 1;
    assert!(validate_in_memory(&store.lock(), &id, &proof, at_ms).is_err());
    store.lock().sandbox_sessions.insert(
        (id.clone(), "fixture-session".into()),
        "2026-09-05T01:00:00Z".into(),
    );
    assert!(store
        .runtime_activity_quiet_proof_fixture(&id, at_ms)
        .is_err());
}
