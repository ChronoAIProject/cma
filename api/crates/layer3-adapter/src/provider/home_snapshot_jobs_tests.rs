use serde_json::{json, Value};

use super::super::snapshot_views::tests::{
    claim as view_claim, request as export_request, snapshot,
};
use super::super::tests::{row, serve, status};
use super::*;
use crate::HomeSnapshotViewReceipt;

pub(super) fn request() -> HomeSnapshotExportJobRequest {
    let export = export_request();
    let digest = export.computed_sha256().unwrap();
    HomeSnapshotExportJobRequest {
        export,
        view: HomeSnapshotViewReceipt {
            volume_reference: format!("cma-view-{}", &digest[..52]),
            request_sha256: digest,
            volume_generation: "view-one".into(),
        },
        attempt: 1,
        image: format!("example.invalid/runtime@sha256:{}", "a".repeat(64)),
    }
}

pub(super) fn pod(uid: &str, phase: Phase) -> Pod {
    let mut pod = spec::desired("cma-test", "deployment", &request()).unwrap();
    pod.metadata.uid = Some(uid.into());
    pod.metadata.resource_version = Some("52".into());
    if phase != Phase::Pending {
        let init_state = match phase {
            Phase::Ready => json!({"terminated":{"exitCode":0}}),
            Phase::Failed => json!({"terminated":{"exitCode":2}}),
            _ => json!({"running":{}}),
        };
        pod.status = Some(serde_json::from_value(json!({
            "phase":"Running",
            "initContainerStatuses":[{"name":"export","image":request().image,"imageID":"sha256:fixture","ready":true,"restartCount":0,"state":init_state}],
            "containerStatuses":[{"name":"transfer","image":request().image,"imageID":"sha256:fixture","ready":true,"restartCount":0,"state":{"running":{}}}]
        })).unwrap());
    }
    pod
}

pub(super) fn pod_row(pod: Pod) -> (u16, Value) {
    (200, serde_json::to_value(pod).unwrap())
}

pub(super) fn ready() -> HomeSnapshotExportJobObservation {
    HomeSnapshotExportJobObservation {
        receipt: identity::receipt(
            "cma-test",
            "deployment",
            &request(),
            &pod("export-one", Phase::Ready),
        )
        .unwrap(),
        view: HomeSnapshotViewObservation {
            receipt: request().view,
            bound_volume: Some("captured-volume".into()),
        },
        phase: Phase::Ready,
    }
}

#[test]
fn pod_mounts_only_snapshot_input_and_has_no_user_or_cluster_credentials() {
    let value = serde_json::to_value(pod("export-one", Phase::Pending)).unwrap();
    let spec = &value["spec"];
    assert_eq!(spec["automountServiceAccountToken"], false);
    assert_eq!(spec["runtimeClassName"], "gvisor");
    assert_eq!(
        spec["volumes"][0]["persistentVolumeClaim"]["claimName"],
        request().view.volume_reference
    );
    assert_ne!(
        spec["volumes"][0]["persistentVolumeClaim"]["claimName"],
        request().export.snapshot.home.receipt.storage_reference
    );
    assert_eq!(
        spec["volumes"][0]["persistentVolumeClaim"]["readOnly"],
        true
    );
    let exporter = &spec["initContainers"][0];
    assert_eq!(exporter["args"][0], "--export-home-snapshot");
    assert!(exporter.get("env").is_none());
    let transfer = &spec["containers"][0];
    assert_eq!(transfer["command"], json!(["/bin/sleep"]));
    assert_eq!(transfer["volumeMounts"].as_array().unwrap().len(), 1);
    assert_eq!(transfer["volumeMounts"][0]["readOnly"], true);
    assert!(value["metadata"]["ownerReferences"].is_null());
}

#[tokio::test]
async fn schedule_replays_one_pending_export_and_observes_ready_without_repeating_work() {
    let fixture = serve(vec![
        row(view_claim("view-one", false)),
        snapshot(),
        status(404),
        pod_row(pod("export-one", Phase::Pending)),
        row(view_claim("view-one", false)),
        snapshot(),
        row(view_claim("view-one", false)),
        snapshot(),
        pod_row(pod("export-one", Phase::Pending)),
        row(view_claim("view-one", false)),
        snapshot(),
        pod_row(pod("export-one", Phase::Ready)),
        row(view_claim("view-one", true)),
        snapshot(),
        pod_row(pod("export-one", Phase::Ready)),
        row(view_claim("view-one", true)),
    ])
    .await;
    let pending = fixture.store.prepare_export_job(&request()).await.unwrap();
    assert_eq!(pending.phase, Phase::Pending);
    assert_eq!(
        fixture.store.prepare_export_job(&request()).await.unwrap(),
        pending
    );
    let bound = fixture
        .store
        .observe_export_job(&request(), &pending)
        .await
        .unwrap();
    assert_eq!(bound, ready());
    assert_eq!(
        fixture
            .store
            .observe_export_job(&request(), &bound)
            .await
            .unwrap(),
        bound
    );
    let requests = fixture.finish().await;
    assert_eq!(
        requests
            .iter()
            .filter(|value| value.starts_with("POST "))
            .count(),
        1
    );
    assert!(requests.iter().all(|value| !value.contains("/exec")));
}

#[tokio::test]
async fn ambiguous_job_creation_recovers_only_its_recorded_attempt() {
    for code in [409, 500] {
        let fixture = serve(vec![
            row(view_claim("view-one", true)),
            snapshot(),
            status(404),
            status(code),
            pod_row(pod("export-one", Phase::Pending)),
            row(view_claim("view-one", true)),
        ])
        .await;
        assert_eq!(
            fixture
                .store
                .prepare_export_job(&request())
                .await
                .unwrap()
                .receipt,
            ready().receipt
        );
        assert_eq!(fixture.finish().await.len(), 6);
    }
    let mut different = pod("export-one", Phase::Pending);
    different.spec.as_mut().unwrap().containers[0].command = Some(vec!["another-program".into()]);
    let fixture = serve(vec![
        row(view_claim("view-one", true)),
        snapshot(),
        status(404),
        status(409),
        pod_row(different),
    ])
    .await;
    assert!(fixture.store.prepare_export_job(&request()).await.is_err());
    assert_eq!(fixture.finish().await.len(), 5);
}

#[tokio::test]
async fn recorded_job_loss_replacement_or_regression_never_reschedules() {
    for reply in [status(404), pod_row(pod("replacement", Phase::Ready))] {
        let fixture = serve(vec![reply]).await;
        assert!(fixture
            .store
            .observe_export_job(&request(), &ready())
            .await
            .is_err());
        assert_eq!(fixture.finish().await.len(), 1);
    }
    let fixture = serve(vec![
        pod_row(pod("export-one", Phase::Pending)),
        row(view_claim("view-one", true)),
    ])
    .await;
    assert!(fixture
        .store
        .observe_export_job(&request(), &ready())
        .await
        .is_err());
    assert_eq!(fixture.finish().await.len(), 2);
    let fixture = serve(vec![
        pod_row(pod("export-one", Phase::Failed)),
        row(view_claim("view-one", true)),
    ])
    .await;
    assert_eq!(
        fixture
            .store
            .observe_export_job(&request(), &ready())
            .await
            .unwrap()
            .phase,
        Phase::Failed
    );
    fixture.finish().await;
}

#[test]
fn foreign_mounts_execution_and_credentials_are_rejected() {
    let original = serde_json::to_value(pod("export-one", Phase::Pending)).unwrap();
    for (pointer, value) in [
        ("/metadata/namespace", json!("other")),
        ("/metadata/labels/app.kubernetes.io~1name", json!("other")),
        ("/spec/automountServiceAccountToken", json!(true)),
        ("/spec/securityContext/runAsUser", json!(0)),
        ("/spec/runtimeClassName", json!("other")),
        ("/spec/initContainers/0/image", json!("other:latest")),
        ("/spec/containers/0/volumeMounts/0/readOnly", json!(false)),
        (
            "/spec/volumes/0/persistentVolumeClaim/claimName",
            json!("live-home"),
        ),
        (
            "/spec/volumes/0/persistentVolumeClaim/readOnly",
            json!(false),
        ),
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
    let mut other = request();
    other.attempt += 1;
    assert!(ready().receipt.validate_for(&other).is_err());
    other = request();
    other.image = "example.invalid/runtime:latest".into();
    assert!(other.validate().is_err());
}

#[tokio::test]
async fn job_cleanup_preserves_replacements_and_waits_for_finalizers_without_source_reads() {
    let fixture = serve(vec![
        pod_row(pod("export-one", Phase::Failed)),
        status(200),
        pod_row(pod("export-one", Phase::Failed)),
    ])
    .await;
    assert!(matches!(
        fixture
            .store
            .remove_export_job(&request(), Some(&ready().receipt))
            .await,
        Err(crate::AdapterError::Unavailable(_))
    ));
    let requests = fixture.finish().await;
    let deletion: Value =
        serde_json::from_str(requests[1].split_once("\r\n\r\n").unwrap().1).unwrap();
    assert_eq!(
        deletion["preconditions"],
        json!({"uid":"export-one","resourceVersion":"52"})
    );
    for after in [status(404), pod_row(pod("replacement", Phase::Pending))] {
        let fixture = serve(vec![
            pod_row(pod("export-one", Phase::Failed)),
            status(200),
            after,
        ])
        .await;
        fixture
            .store
            .remove_export_job(&request(), None)
            .await
            .unwrap();
        assert_eq!(fixture.finish().await.len(), 3);
    }
}

#[tokio::test]
async fn complete_job_inventory_includes_failed_and_deleting_attempts() {
    let page = |items: Vec<Pod>, cursor: &str| {
        (
            200,
            json!({"apiVersion":"v1","kind":"PodList","metadata":{"continue":cursor},"items":items}),
        )
    };
    let mut failed = pod("export-one", Phase::Failed);
    failed.metadata.deletion_timestamp =
        Some(serde_json::from_value(json!("2026-09-10T00:00:00Z")).unwrap());
    let fixture = serve(vec![page(vec![failed.clone()], "next"), page(vec![], "")]).await;
    assert_eq!(
        fixture.store.export_job_inventory().await.unwrap()[0].receipt,
        ready().receipt
    );
    let requests = fixture.finish().await;
    assert!(requests[0].contains("labelSelector="));
    assert!(requests[1].contains("continue=next"));
    let fixture = serve(vec![
        page(vec![failed.clone()], "next"),
        page(vec![failed], ""),
    ])
    .await;
    assert!(fixture.store.export_job_inventory().await.is_err());
    assert_eq!(fixture.finish().await.len(), 2);
}
