use serde_json::{json, Value};

use super::super::tests::{claim, request as home_request, row, serve, status, template};
use super::*;
use crate::HomeVolumeAttachment;
use crate::HomeVolumeStore;

fn request() -> HomeSnapshotRequest {
    let home = home_request();
    let receipt = super::super::identity::validate_claim(
        "cma-test",
        "deployment",
        &home,
        &claim("1", "disk-one"),
    )
    .unwrap();
    HomeSnapshotRequest {
        capture_id: "capture-one".into(),
        epoch: 1,
        home: HomeVolumeAttachment {
            request: home,
            receipt,
        },
        snapshot_class: "cma-home-v1".into(),
    }
}

fn snapshot(uid: &str) -> DynamicObject {
    let mut snapshot = identity::desired("cma-test", "deployment", &request()).unwrap();
    snapshot.metadata.uid = Some(uid.into());
    snapshot.metadata.resource_version = Some("42".into());
    snapshot
}

fn snapshot_row(snapshot: DynamicObject) -> (u16, Value) {
    (200, serde_json::to_value(snapshot).unwrap())
}

fn receipt() -> HomeSnapshotReceipt {
    identity::receipt(
        "cma-test",
        "deployment",
        &request(),
        &snapshot("snapshot-one"),
    )
    .unwrap()
}

fn pending() -> HomeSnapshotObservation {
    HomeSnapshotObservation {
        receipt: receipt(),
        captured_at: None,
        ready: false,
    }
}

#[tokio::test]
async fn home_retirement_removes_only_captures_of_the_exact_original_disk() {
    for original in [false, true] {
        let mut expected = request().home;
        if !original {
            expected.receipt.storage_generation = "replacement-home-uid".into();
        }
        let mut replies = vec![
            (
                200,
                json!({"apiVersion":"v1","kind":"PodList","metadata":{},"items":[]}),
            ),
            (
                200,
                json!({"apiVersion":"v1","kind":"PersistentVolumeClaimList","metadata":{},"items":[]}),
            ),
            (
                200,
                json!({"apiVersion":"snapshot.storage.k8s.io/v1","kind":"VolumeSnapshotList","metadata":{},"items":[snapshot("snapshot-one")]}),
            ),
        ];
        if original {
            replies.extend([
                snapshot_row(snapshot("snapshot-one")),
                status(200),
                status(404),
            ]);
        }
        let fixture = serve(replies).await;
        fixture.store.remove_captures(&expected).await.unwrap();
        let requests = fixture.finish().await;
        assert!(requests[0].contains("/pods?"));
        assert!(requests[1].contains("/persistentvolumeclaims?"));
        assert_eq!(
            requests
                .iter()
                .filter(|request| request.starts_with("DELETE "))
                .count(),
            usize::from(original)
        );
        if original {
            assert!(requests[4].contains("snapshot-one"));
            assert!(requests[4].contains("resourceVersion"));
        }
    }
}

#[tokio::test]
async fn allocation_returns_pending_and_retries_without_recapturing() {
    let pending = snapshot("snapshot-one");
    let fixture = serve(vec![
        template(),
        row(claim("1", "disk-one")),
        status(404),
        snapshot_row(pending.clone()),
        template(),
        row(claim("2", "disk-one")),
        template(),
        row(claim("3", "disk-one")),
        snapshot_row(pending),
        template(),
        row(claim("4", "disk-one")),
    ])
    .await;
    let first = fixture.store.prepare_snapshot(&request()).await.unwrap();
    assert!(!first.ready);
    assert!(first.captured_at.is_none());
    assert_eq!(first.receipt, receipt());
    assert_eq!(
        fixture.store.prepare_snapshot(&request()).await.unwrap(),
        first
    );
    let requests = fixture.finish().await;
    let creates: Vec<_> = requests
        .iter()
        .filter(|request| request.starts_with("POST "))
        .collect();
    assert_eq!(creates.len(), 1);
    let created: Value =
        serde_json::from_str(creates[0].split_once("\r\n\r\n").unwrap().1).unwrap();
    assert_eq!(
        created["spec"]["source"]["persistentVolumeClaimName"],
        request().home.receipt.storage_reference
    );
    assert_eq!(created["spec"]["volumeSnapshotClassName"], "cma-home-v1");
    assert!(requests
        .iter()
        .all(|request| !request.contains("/pods") && !request.contains("/exec")));
}

#[tokio::test]
async fn ambiguous_creation_recovers_only_the_same_snapshot() {
    for code in [409, 500] {
        let fixture = serve(vec![
            template(),
            row(claim("1", "disk-one")),
            status(404),
            status(code),
            snapshot_row(snapshot("snapshot-one")),
            template(),
            row(claim("2", "disk-one")),
        ])
        .await;
        assert_eq!(
            fixture
                .store
                .prepare_snapshot(&request())
                .await
                .unwrap()
                .receipt,
            receipt()
        );
        assert_eq!(fixture.finish().await.len(), 7);
    }
    let mut foreign = snapshot("snapshot-two");
    foreign.data["spec"]["source"]["persistentVolumeClaimName"] = json!("another-home");
    let fixture = serve(vec![
        template(),
        row(claim("1", "disk-one")),
        status(404),
        status(409),
        snapshot_row(foreign),
    ])
    .await;
    assert!(fixture.store.prepare_snapshot(&request()).await.is_err());
    assert_eq!(fixture.finish().await.len(), 5);
}

#[tokio::test]
async fn source_replacement_during_allocation_cannot_be_accepted() {
    let fixture = serve(vec![
        template(),
        row(claim("1", "disk-one")),
        status(404),
        snapshot_row(snapshot("snapshot-one")),
        template(),
        row(claim("2", "disk-two")),
    ])
    .await;
    assert!(fixture.store.prepare_snapshot(&request()).await.is_err());
    assert_eq!(fixture.finish().await.len(), 6);
}

#[tokio::test]
async fn capture_boundary_precedes_readiness_and_observation_needs_no_live_source() {
    let mut captured = snapshot("snapshot-one");
    captured.data["status"] = json!({
        "readyToUse":false, "creationTime":"2026-09-10T00:00:00Z",
        "boundVolumeSnapshotContentName":"content-one",
    });
    let mut ready = captured.clone();
    ready.data["status"]["readyToUse"] = json!(true);
    let fixture = serve(vec![
        snapshot_row(captured),
        template(),
        row(claim("2", "disk-one")),
        snapshot_row(ready),
    ])
    .await;
    let capture = fixture
        .store
        .observe_snapshot(&request(), &pending())
        .await
        .unwrap();
    assert!(!capture.ready);
    assert_eq!(capture.captured_at.as_deref(), Some("2026-09-10T00:00:00Z"));
    let ready = fixture
        .store
        .observe_snapshot(&request(), &capture)
        .await
        .unwrap();
    assert!(ready.ready);
    assert_eq!(ready.captured_at, capture.captured_at);
    let requests = fixture.finish().await;
    assert_eq!(requests.len(), 4);
    assert!(requests[3].starts_with("GET /apis/snapshot.storage.k8s.io/"));
}

#[tokio::test]
async fn recorded_snapshot_loss_or_replacement_never_recaptures() {
    for reply in [status(404), snapshot_row(snapshot("replacement"))] {
        let fixture = serve(vec![reply]).await;
        assert!(fixture
            .store
            .observe_snapshot(&request(), &pending())
            .await
            .is_err());
        assert!(fixture.finish().await[0].starts_with("GET "));
    }
}

#[tokio::test]
async fn asynchronous_capture_still_requires_the_original_source_disk() {
    for capture_status in [
        json!({}),
        json!({
            "readyToUse":false, "creationTime":"2026-09-10T00:00:00Z",
            "boundVolumeSnapshotContentName":"content-one",
        }),
    ] {
        let mut captured = snapshot("snapshot-one");
        captured.data["status"] = capture_status;
        let fixture = serve(vec![
            snapshot_row(captured),
            template(),
            row(claim("2", "disk-two")),
        ])
        .await;
        assert!(fixture
            .store
            .observe_snapshot(&request(), &pending())
            .await
            .is_err());
        assert_eq!(fixture.finish().await.len(), 3);
    }
}

#[tokio::test]
async fn established_capture_time_and_readiness_cannot_regress() {
    let previous = HomeSnapshotObservation {
        receipt: receipt(),
        captured_at: Some("2026-09-10T00:00:00Z".into()),
        ready: true,
    };
    for capture_status in [
        json!({"readyToUse":false,"creationTime":"2026-09-10T00:00:00Z","boundVolumeSnapshotContentName":"content-one"}),
        json!({"readyToUse":true,"creationTime":"2026-09-10T00:01:00Z","boundVolumeSnapshotContentName":"content-one"}),
        json!({}),
    ] {
        let mut captured = snapshot("snapshot-one");
        captured.data["status"] = capture_status;
        let fixture = serve(vec![snapshot_row(captured)]).await;
        assert!(fixture
            .store
            .observe_snapshot(&request(), &previous)
            .await
            .is_err());
        assert_eq!(fixture.finish().await.len(), 1);
    }
}

#[test]
fn malformed_status_and_foreign_authority_are_refused() {
    for status in [
        json!({"readyToUse":true}),
        json!({"readyToUse":"true"}),
        json!({"creationTime":"yesterday"}),
        json!({"error":{"message":"private provider diagnostic"}}),
    ] {
        let mut snapshot = snapshot("snapshot-one");
        snapshot.data["status"] = status;
        let error = identity::observe(receipt(), &snapshot).unwrap_err();
        assert!(!error.to_string().contains("private provider diagnostic"));
    }
    let original = serde_json::to_value(snapshot("snapshot-one")).unwrap();
    for (pointer, value) in [
        ("/metadata/namespace", json!("another")),
        (
            "/metadata/labels/cma~1home-snapshot-managed-by",
            json!("other"),
        ),
        (
            "/metadata/annotations/cma~1home-snapshot-request-sha256",
            json!("other"),
        ),
        ("/spec/source/persistentVolumeClaimName", json!("other")),
        ("/spec/volumeSnapshotClassName", json!("other")),
    ] {
        let mut changed = original.clone();
        *changed.pointer_mut(pointer).unwrap() = value;
        let snapshot = serde_json::from_value(changed).unwrap();
        assert!(
            identity::receipt("cma-test", "deployment", &request(), &snapshot).is_err(),
            "{pointer}"
        );
    }
    let mut changed = request();
    changed.epoch += 1;
    assert!(receipt().validate_for(&changed).is_err());
}

#[tokio::test]
async fn cleanup_waits_for_finalizers_and_never_deletes_a_replacement() {
    let mut deleting = snapshot("snapshot-one");
    deleting.metadata.deletion_timestamp =
        Some(serde_json::from_value(json!("2026-09-10T00:00:00Z")).unwrap());
    let fixture = serve(vec![
        snapshot_row(snapshot("snapshot-one")),
        status(200),
        snapshot_row(deleting.clone()),
        snapshot_row(deleting),
        status(404),
    ])
    .await;
    assert!(fixture
        .store
        .remove_snapshot(&request(), Some(&receipt()))
        .await
        .is_err());
    fixture
        .store
        .remove_snapshot(&request(), Some(&receipt()))
        .await
        .unwrap();
    let requests = fixture.finish().await;
    let deletes: Vec<_> = requests
        .iter()
        .filter(|request| request.starts_with("DELETE "))
        .collect();
    assert_eq!(deletes.len(), 1);
    let deleted: Value =
        serde_json::from_str(deletes[0].split_once("\r\n\r\n").unwrap().1).unwrap();
    assert_eq!(
        deleted["preconditions"],
        json!({"uid":"snapshot-one","resourceVersion":"42"})
    );
    let fixture = serve(vec![snapshot_row(snapshot("replacement"))]).await;
    assert!(fixture
        .store
        .remove_snapshot(&request(), Some(&receipt()))
        .await
        .is_err());
    assert!(fixture.finish().await[0].starts_with("GET "));
}

#[tokio::test]
async fn inventory_includes_pending_and_failed_captures_for_durable_cleanup() {
    let page = |items: Vec<DynamicObject>, cursor: &str| {
        (
            200,
            json!({
                "apiVersion":"snapshot.storage.k8s.io/v1", "kind":"VolumeSnapshotList",
                "metadata":{"continue":cursor}, "items":items,
            }),
        )
    };
    let mut failed = snapshot("snapshot-one");
    failed.data["status"] = json!({"error":{"message":"fixture"}});
    let fixture = serve(vec![page(vec![failed], "next"), page(vec![], "")]).await;
    let entries = fixture.store.snapshot_inventory().await.unwrap();
    assert_eq!(
        entries,
        vec![HomeSnapshotInventoryEntry {
            request: request(),
            receipt: receipt()
        }]
    );
    let requests = fixture.finish().await;
    assert!(requests
        .iter()
        .all(|request| request.contains("labelSelector=")));
    assert!(requests[1].contains("continue=next"));
    let fixture = serve(vec![
        page(vec![snapshot("snapshot-one")], "next"),
        page(vec![snapshot("snapshot-one")], ""),
    ])
    .await;
    assert!(fixture.store.snapshot_inventory().await.is_err());
    assert_eq!(fixture.finish().await.len(), 2);
}
