use super::*;
use kube::api::DynamicObject;
use serde_json::json;

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
