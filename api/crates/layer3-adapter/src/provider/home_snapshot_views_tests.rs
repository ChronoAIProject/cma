use serde_json::{json, Value};

use super::super::tests::{claim as home_claim, request as home_request, row, serve, status};
use super::*;
use crate::{
    HomeSnapshotObservation, HomeSnapshotReceipt, HomeSnapshotRequest, HomeVolumeAttachment,
};

pub(crate) fn request() -> HomeSnapshotExportRequest {
    let home = home_request();
    let receipt = super::super::identity::validate_claim(
        "cma-test",
        "deployment",
        &home,
        &home_claim("1", "disk-one"),
    )
    .unwrap();
    let snapshot = HomeSnapshotRequest {
        capture_id: "view-fixture".into(),
        epoch: 1,
        home: HomeVolumeAttachment {
            request: home,
            receipt,
        },
        snapshot_class: "cma-home-v1".into(),
    };
    let observation = HomeSnapshotObservation {
        receipt: HomeSnapshotReceipt {
            capture_id: snapshot.capture_id.clone(),
            request_sha256: snapshot.computed_sha256(),
            snapshot_reference: format!("cma-save-{}", &snapshot.computed_sha256()[..52]),
            snapshot_generation: "snapshot-one".into(),
        },
        captured_at: Some("2026-09-10T00:00:00Z".into()),
        ready: true,
    };
    HomeSnapshotExportRequest {
        snapshot,
        observation,
        runtime_generation: 1,
        conversations: vec![],
    }
}

pub(crate) fn snapshot() -> (u16, Value) {
    let request = request();
    (
        200,
        json!({
            "apiVersion":"snapshot.storage.k8s.io/v1", "kind":"VolumeSnapshot",
            "metadata":{
                "name":request.observation.receipt.snapshot_reference,
                "namespace":"cma-test", "uid":"snapshot-one", "resourceVersion":"1",
                "labels":{"cma/home-snapshot-managed-by":"deployment"},
                "annotations":{
                    "cma/home-snapshot-request":serde_json::to_string(&request.snapshot).unwrap(),
                    "cma/home-snapshot-request-sha256":request.snapshot.computed_sha256()
                }
            },
            "spec":{"volumeSnapshotClassName":"cma-home-v1","source":{"persistentVolumeClaimName":request.snapshot.home.receipt.storage_reference}},
            "status":{"readyToUse":true,"creationTime":request.observation.captured_at,"boundVolumeSnapshotContentName":"snapshot-content"}
        }),
    )
}

pub(crate) fn claim(uid: &str, bound: bool) -> PersistentVolumeClaim {
    let mut claim = identity::desired("cma-test", "deployment", &request()).unwrap();
    claim.metadata.uid = Some(uid.into());
    claim.metadata.resource_version = Some("12".into());
    if bound {
        claim.spec.as_mut().unwrap().volume_name = Some("captured-volume".into());
        claim.status = Some(serde_json::from_value(json!({"phase":"Bound"})).unwrap());
    }
    claim
}

fn observation(bound: bool) -> HomeSnapshotViewObservation {
    let claim = claim("view-one", bound);
    identity::observe(
        identity::receipt("cma-test", "deployment", &request(), &claim).unwrap(),
        &claim,
    )
    .unwrap()
}

#[tokio::test]
async fn allocation_returns_pending_and_replays_one_distinct_snapshot_derived_volume() {
    let fixture = serve(vec![
        snapshot(),
        status(404),
        row(claim("view-one", false)),
        snapshot(),
        snapshot(),
        row(claim("view-one", false)),
        snapshot(),
    ])
    .await;
    let observed = fixture.store.prepare_view(&request()).await.unwrap();
    assert_eq!(observed, observation(false));
    assert_eq!(
        fixture.store.prepare_view(&request()).await.unwrap(),
        observed
    );
    let requests = fixture.finish().await;
    let creates = requests
        .iter()
        .filter(|request| request.starts_with("POST "))
        .collect::<Vec<_>>();
    assert_eq!(creates.len(), 1);
    let created: Value =
        serde_json::from_str(creates[0].split_once("\r\n\r\n").unwrap().1).unwrap();
    assert_eq!(
        created["spec"]["dataSource"]["name"],
        request().observation.receipt.snapshot_reference
    );
    assert_ne!(
        created["metadata"]["name"],
        request().snapshot.home.receipt.storage_reference
    );
    assert_eq!(created["spec"]["accessModes"], json!(["ReadWriteOncePod"]));
    assert!(created["metadata"].get("ownerReferences").is_none());
    assert!(requests
        .iter()
        .all(|value| !value.contains("/pods") && !value.contains("/exec")));
}

#[tokio::test]
async fn ambiguous_allocation_adopts_only_exact_export_authority() {
    for code in [409, 500] {
        let fixture = serve(vec![
            snapshot(),
            status(404),
            status(code),
            row(claim("view-one", false)),
            snapshot(),
        ])
        .await;
        assert_eq!(
            fixture.store.prepare_view(&request()).await.unwrap(),
            observation(false)
        );
        assert_eq!(fixture.finish().await.len(), 5);
    }
    let mut foreign = claim("view-one", false);
    foreign
        .spec
        .as_mut()
        .unwrap()
        .data_source
        .as_mut()
        .unwrap()
        .name = "another-snapshot".into();
    let fixture = serve(vec![snapshot(), status(404), status(409), row(foreign)]).await;
    assert!(fixture.store.prepare_view(&request()).await.is_err());
    assert_eq!(fixture.finish().await.len(), 4);
}

#[tokio::test]
async fn snapshot_uid_must_survive_allocation_and_first_observed_binding() {
    let mut replaced = snapshot();
    replaced.1["metadata"]["uid"] = json!("replacement-snapshot");
    let fixture = serve(vec![
        snapshot(),
        status(404),
        row(claim("view-one", false)),
        replaced.clone(),
    ])
    .await;
    assert!(fixture.store.prepare_view(&request()).await.is_err());
    assert_eq!(fixture.finish().await.len(), 4);
    for bound in [false, true] {
        let fixture = serve(vec![row(claim("view-one", bound)), replaced.clone()]).await;
        assert!(fixture
            .store
            .observe_view(&request(), &observation(false))
            .await
            .is_err());
        assert_eq!(fixture.finish().await.len(), 2);
    }
}

#[tokio::test]
async fn verified_binding_survives_snapshot_removal_but_never_changes_disk() {
    let fixture = serve(vec![
        row(claim("view-one", true)),
        snapshot(),
        row(claim("view-one", true)),
    ])
    .await;
    let bound = fixture
        .store
        .observe_view(&request(), &observation(false))
        .await
        .unwrap();
    assert_eq!(bound, observation(true));
    assert_eq!(
        fixture
            .store
            .observe_view(&request(), &bound)
            .await
            .unwrap(),
        bound
    );
    let requests = fixture.finish().await;
    assert!(requests[2].starts_with("GET /api/v1/namespaces/cma-test/persistentvolumeclaims/"));
    let mut other_disk = claim("view-one", true);
    other_disk.spec.as_mut().unwrap().volume_name = Some("another-volume".into());
    for response in [
        status(404),
        row(claim("view-two", true)),
        row(claim("view-one", false)),
        row(other_disk),
    ] {
        let fixture = serve(vec![response]).await;
        assert!(fixture
            .store
            .observe_view(&request(), &bound)
            .await
            .is_err());
        assert!(fixture.finish().await[0].starts_with("GET "));
    }
}

#[test]
fn view_identity_rejects_cross_owner_source_and_policy_changes() {
    let original = serde_json::to_value(claim("view-one", false)).unwrap();
    for (pointer, value) in [
        ("/metadata/namespace", json!("other")),
        ("/metadata/labels/cma~1home-view-managed-by", json!("other")),
        (
            "/metadata/annotations/cma~1home-view-request-sha256",
            json!("other"),
        ),
        ("/spec/dataSource/name", json!("live-home")),
        ("/spec/dataSource/kind", json!("PersistentVolumeClaim")),
        ("/spec/storageClassName", json!("other")),
        ("/spec/resources/requests/storage", json!("1Gi")),
        ("/spec/accessModes", json!(["ReadWriteMany"])),
    ] {
        let mut changed = original.clone();
        *changed.pointer_mut(pointer).unwrap() = value;
        assert!(
            identity::receipt(
                "cma-test",
                "deployment",
                &request(),
                &serde_json::from_value(changed).unwrap()
            )
            .is_err(),
            "{pointer}"
        );
    }
    let mut lost = claim("view-one", true);
    lost.status.as_mut().unwrap().phase = Some("Lost".into());
    assert!(identity::observe(observation(true).receipt, &lost).is_err());
    let mut pending = request();
    pending.observation.ready = false;
    assert!(identity::desired("cma-test", "deployment", &pending).is_err());
}

#[tokio::test]
async fn cleanup_needs_no_source_and_waits_for_exact_view_finalizers() {
    let fixture = serve(vec![
        row(claim("view-one", true)),
        status(200),
        row(claim("view-one", true)),
    ])
    .await;
    assert!(matches!(
        fixture
            .store
            .remove_view(&request(), Some(&observation(true).receipt))
            .await,
        Err(AdapterError::Unavailable(_))
    ));
    let requests = fixture.finish().await;
    let deleted: Value =
        serde_json::from_str(requests[1].split_once("\r\n\r\n").unwrap().1).unwrap();
    assert_eq!(deleted["preconditions"]["uid"], "view-one");
    assert_eq!(deleted["preconditions"]["resourceVersion"], "12");
    let fixture = serve(vec![row(claim("view-two", true))]).await;
    assert!(fixture
        .store
        .remove_view(&request(), Some(&observation(true).receipt))
        .await
        .is_err());
    assert_eq!(fixture.finish().await.len(), 1);
    for remaining in [status(404), row(claim("view-two", true))] {
        let fixture = serve(vec![row(claim("view-one", true)), status(200), remaining]).await;
        fixture.store.remove_view(&request(), None).await.unwrap();
        assert_eq!(fixture.finish().await.len(), 3);
    }
}

#[tokio::test]
async fn inventory_is_scoped_paginated_and_includes_failed_and_deleting_views() {
    let page = |items: Vec<PersistentVolumeClaim>, cursor: &str| {
        (
            200,
            json!({"apiVersion":"v1","kind":"PersistentVolumeClaimList","metadata":{"continue":cursor},"items":items}),
        )
    };
    let mut failed = claim("view-one", false);
    failed.status = Some(serde_json::from_value(json!({"phase":"Lost"})).unwrap());
    failed.metadata.deletion_timestamp =
        Some(serde_json::from_value(json!("2026-09-10T00:01:00Z")).unwrap());
    let fixture = serve(vec![page(vec![failed.clone()], "next"), page(vec![], "")]).await;
    let entries = fixture.store.view_inventory().await.unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].receipt, observation(false).receipt);
    let requests = fixture.finish().await;
    assert!(requests
        .iter()
        .all(|request| request.contains("labelSelector=cma%2Fhome-view-managed-by%3Ddeployment")));
    assert!(requests[1].contains("continue=next"));
    for last in [
        page(vec![failed.clone()], ""),
        page(vec![], "next"),
        status(403),
    ] {
        let fixture = serve(vec![page(vec![failed.clone()], "next"), last]).await;
        assert!(fixture.store.view_inventory().await.is_err());
        assert_eq!(fixture.finish().await.len(), 2);
    }
}
