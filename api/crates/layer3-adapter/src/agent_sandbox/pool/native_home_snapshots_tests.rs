use super::super::fixture::Fixture;
use super::*;
use kube::api::DynamicObject;
use serde_json::json;

#[path = "native_home_snapshot_reobserve_tests.rs"]
mod reobserve_contracts;

fn request() -> NativeHomeSnapshotRequest {
    NativeHomeSnapshotRequest {
        backup_id: "backup-1".into(),
        backup_of: "sandbox-hash".into(),
        epoch: 7,
        snapshot_class: "retain".into(),
        source: NativeHomeSnapshotSource {
            namespace: "cma-test".into(),
            pvc_name: "home".into(),
            pvc_uid: "pvc-uid-1".into(),
        },
    }
}

fn observation(ready: bool) -> NativeHomeSnapshotObservation {
    NativeHomeSnapshotObservation {
        request: request(),
        snapshot_name: request().deterministic_name(),
        snapshot_uid: "snapshot-uid".into(),
        snapshot_content_name: ready.then(|| "content".into()),
        snapshot_content_uid: ready.then(|| "content-uid".into()),
        underlying_copy_handle: ready.then(|| "handle".into()),
        creation_time: ready.then(|| "2026-11-17T00:00:00Z".into()),
        ready,
    }
}

fn pending_with_cut() -> NativeHomeSnapshotObservation {
    let mut value = observation(false);
    value.creation_time = Some("2026-11-17T00:00:00Z".into());
    value
}

#[test]
fn desired_object_pins_complete_request_and_exact_labels() {
    let request = request();
    let object = desired_snapshot("cma-test", "deployment", &request).unwrap();
    let request_annotation = request_json(&request).unwrap();
    assert_eq!(
        object.labels().get(BACKUP_OF_LABEL),
        Some(&request.backup_of)
    );
    assert_eq!(
        object.labels().get(BACKUP_ID_LABEL),
        Some(&request.backup_id)
    );
    assert_eq!(
        object.annotations().get(REQUEST_ANNOTATION),
        Some(&request_annotation)
    );
    assert_eq!(object.data["spec"], desired_spec(&request));
}

#[test]
fn observations_reject_ready_evidence_replacement_and_regression() {
    let previous = observation(true);
    for next in [
        {
            let mut next = previous.clone();
            next.ready = false;
            next
        },
        {
            let mut next = previous.clone();
            next.creation_time = Some("2026-11-17T00:01:00Z".into());
            next
        },
        {
            let mut next = previous.clone();
            next.snapshot_content_uid = Some("other".into());
            next
        },
        {
            let mut next = previous.clone();
            next.underlying_copy_handle = Some("other".into());
            next
        },
    ] {
        assert!(validate_observation_transition(&previous, &next).is_err());
    }
}

#[test]
fn pending_observation_can_gain_only_new_evidence() {
    let previous = observation(false);
    let mut next = previous.clone();
    next.ready = true;
    next.creation_time = Some("2026-11-17T00:00:00Z".into());
    next.snapshot_content_name = Some("content".into());
    next.snapshot_content_uid = Some("content-uid".into());
    next.underlying_copy_handle = Some("handle".into());
    assert!(validate_observation_transition(&previous, &next).is_ok());
}

#[test]
fn creation_time_establishes_cut_before_ready() {
    let request = request();
    let mut object = desired_snapshot("cma-test", "deployment", &request).unwrap();
    object.metadata.uid = Some("snapshot-uid".into());
    object.data["status"] = json!({
        "readyToUse": false,
        "creationTime": "2026-11-17T00:00:00Z"
    });
    assert!(object_reports_cut(&object).unwrap());
}

#[test]
fn source_guard_ends_after_strict_creation_cut_for_pending_and_ready() {
    assert!(source_required_for_observation(&observation(false)));
    assert!(!source_required_for_observation(&pending_with_cut()));
    assert!(!source_required_for_observation(&observation(true)));
}

fn snapshot_path(request: &NativeHomeSnapshotRequest) -> String {
    format!(
        "/apis/snapshot.storage.k8s.io/v1/namespaces/cma-test/volumesnapshots/{}",
        request.deterministic_name()
    )
}

#[tokio::test]
async fn source_missing_before_cut_fails_without_creating_snapshot() {
    let fixture = Fixture::new().await;
    assert!(fixture
        .client
        .observe_native_home_snapshot(&observation(false))
        .await
        .is_err());
    let calls = fixture.state.lock().unwrap().calls.clone();
    assert!(calls.iter().all(|(method, _)| method.starts_with("GET ")));
    assert!(!calls.iter().any(|(method, _)| method.starts_with("POST ")));
}

#[tokio::test]
async fn source_missing_after_pending_cut_still_observes() {
    let fixture = Fixture::new().await;
    let request = request();
    let mut snapshot = desired_snapshot("cma-test", "cma-test", &request).unwrap();
    snapshot.metadata.uid = Some("snapshot-uid".into());
    snapshot.data["status"] = json!({
        "readyToUse": false,
        "creationTime": "2026-11-17T00:00:00Z"
    });
    fixture.state.lock().unwrap().rows.insert(
        snapshot_path(&request),
        serde_json::to_value(snapshot).unwrap(),
    );
    let observed = fixture
        .client
        .observe_native_home_snapshot(&pending_with_cut())
        .await
        .unwrap();
    assert!(!observed.ready);
}

#[tokio::test]
async fn source_missing_after_ready_cut_still_returns_handle() {
    let fixture = Fixture::new().await;
    let request = request();
    let mut snapshot = desired_snapshot("cma-test", "cma-test", &request).unwrap();
    snapshot.metadata.uid = Some("snapshot-uid".into());
    snapshot.data["status"] = json!({
        "readyToUse": true,
        "creationTime": "2026-11-17T00:00:00Z",
        "boundVolumeSnapshotContentName": "content"
    });
    let content: DynamicObject = serde_json::from_value(json!({
        "apiVersion":"snapshot.storage.k8s.io/v1", "kind":"VolumeSnapshotContent",
        "metadata":{"name":"content","uid":"content-uid"},
        "spec":{"volumeSnapshotRef":{"name":request.deterministic_name(),"namespace":"cma-test","uid":"snapshot-uid"}},
        "status":{"snapshotHandle":"handle"}
    }))
    .unwrap();
    {
        let mut state = fixture.state.lock().unwrap();
        state.rows.insert(
            snapshot_path(&request),
            serde_json::to_value(snapshot).unwrap(),
        );
        state.rows.insert(
            "/apis/snapshot.storage.k8s.io/v1/volumesnapshotcontents/content".into(),
            serde_json::to_value(content).unwrap(),
        );
    }
    let observed = fixture
        .client
        .observe_native_home_snapshot(&observation(true))
        .await
        .unwrap();
    assert_eq!(observed.underlying_copy_handle.as_deref(), Some("handle"));
}

#[tokio::test]
async fn missing_snapshot_is_error_and_never_recreated() {
    let fixture = Fixture::new().await;
    fixture.state.lock().unwrap().rows.insert(
        "/api/v1/namespaces/cma-test/persistentvolumeclaims/home".into(),
        json!({
            "apiVersion":"v1", "kind":"PersistentVolumeClaim",
            "metadata":{"name":"home","namespace":"cma-test","uid":"pvc-uid-1"}
        }),
    );
    assert!(fixture
        .client
        .observe_native_home_snapshot(&observation(false))
        .await
        .is_err());
    let calls = fixture.state.lock().unwrap().calls.clone();
    assert!(calls.iter().all(|(method, _)| method.starts_with("GET ")));
    assert!(!calls.iter().any(|(method, _)| method.starts_with("POST ")));
}

#[test]
fn snapshot_rejects_tampered_request_spec_status_and_deletion() {
    let request = request();
    let mut object = desired_snapshot("cma-test", "deployment", &request).unwrap();
    object.metadata.uid = Some("snapshot-uid".into());
    object.data["spec"]["source"]["persistentVolumeClaimName"] = json!("foreign");
    assert!(validate_snapshot(&object, "cma-test", "deployment", &request).is_err());

    let mut object = desired_snapshot("cma-test", "deployment", &request).unwrap();
    object.metadata.uid = Some("snapshot-uid".into());
    object.data["status"] = json!({"readyToUse": "true"});
    assert!(validate_snapshot(&object, "cma-test", "deployment", &request).is_err());

    let mut object = desired_snapshot("cma-test", "deployment", &request).unwrap();
    object.metadata.uid = Some("snapshot-uid".into());
    object.data["status"] = json!({"error": "provider failed"});
    assert!(validate_snapshot(&object, "cma-test", "deployment", &request).is_err());

    let mut object = desired_snapshot("cma-test", "deployment", &request).unwrap();
    object.metadata.uid = Some("snapshot-uid".into());
    object.metadata.deletion_timestamp =
        Some(serde_json::from_value(json!("2026-11-17T00:00:00Z")).unwrap());
    assert!(validate_snapshot(&object, "cma-test", "deployment", &request).is_err());
}

#[test]
fn content_requires_current_snapshot_uid_and_real_handle_type() {
    let mut content: DynamicObject = serde_json::from_value(json!({
        "apiVersion":"snapshot.storage.k8s.io/v1", "kind":"VolumeSnapshotContent",
        "metadata":{"name":"content","uid":"content-uid"},
        "spec":{"volumeSnapshotRef":{"name":"cma-home-backup-x","namespace":"cma-test","uid":"snapshot-uid"}},
        "status":{"snapshotHandle":"handle"}
    }))
    .unwrap();
    assert!(validate_content(&content, "cma-test", "snapshot-uid", "cma-home-backup-x").is_ok());
    content.data["spec"]["volumeSnapshotRef"]["uid"] = json!("foreign-snapshot-uid");
    assert!(validate_content(&content, "cma-test", "snapshot-uid", "cma-home-backup-x").is_err());
    content.data["spec"]["volumeSnapshotRef"]["uid"] = json!("snapshot-uid");
    content.metadata.uid = None;
    assert!(validate_content(&content, "cma-test", "snapshot-uid", "cma-home-backup-x").is_err());
    content.metadata.uid = Some("content-uid".into());
    content.data["status"]["snapshotHandle"] = json!(42);
    assert!(validate_content(&content, "cma-test", "snapshot-uid", "cma-home-backup-x").is_err());
}

#[test]
fn malformed_creation_time_and_request_are_rejected() {
    let mut pending = observation(false);
    pending.creation_time = Some("yesterday".into());
    assert!(pending.validate().is_err());
    let mut request = request();
    request.epoch = 0;
    assert!(request.validate().is_err());
}
