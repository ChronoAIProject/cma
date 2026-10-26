use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::AsyncWriteExt;

use super::*;
use crate::provider::tests::read_request;

#[tokio::test]
async fn home_allocation_scope_tracks_configured_authority_namespace_and_controller() {
    let scope = |authority: &str, namespace: &str, controller: &str| {
        let client = kube::Client::try_from(kube::Config::new(authority.parse().unwrap())).unwrap();
        PersistentHomeVolumes::namespaced(client, namespace, controller, authority)
            .unwrap()
            .allocation_scope()
            .unwrap()
    };
    let original = scope("http://127.0.0.1:10001", "cma-test", "deployment");
    original.validate().unwrap();
    assert_eq!(
        original,
        scope("http://127.0.0.1:10001", "cma-test", "deployment")
    );
    for changed in [
        scope("http://127.0.0.1:10002", "cma-test", "deployment"),
        scope("http://127.0.0.1:10001", "other-namespace", "deployment"),
        scope("http://127.0.0.1:10001", "cma-test", "other-controller"),
    ] {
        assert_ne!(original, changed);
    }
    let encoded = serde_json::to_string(&original).unwrap();
    assert!(!encoded.contains("127.0.0.1") && !encoded.contains("cma-test"));
}

pub(super) fn request() -> HomeVolumeRequest {
    HomeVolumeRequest {
        scope: None,
        volume_id: "home-fixture".into(),
        sandbox_id: "sandbox-fixture".into(),
        policy: HomeVolumePolicy {
            mode: Default::default(),
            storage_class: "standard-rwo".into(),
            capacity_gib: 64,
            template_name: "empty-home".into(),
            template_generation: "template-generation".into(),
        },
    }
}

pub(super) fn template() -> (u16, Value) {
    (
        200,
        json!({
            "apiVersion":"snapshot.storage.k8s.io/v1", "kind":"VolumeSnapshot",
            "metadata":{"name":"empty-home","namespace":"cma-test","uid":"template-generation","annotations":{"cma/home-seed-format":"1"}},
            "status":{"readyToUse":true}
        }),
    )
}

pub(super) fn claim(version: &str, uid: &str) -> PersistentVolumeClaim {
    let mut claim = identity::desired_claim("cma-test", "deployment", &request()).unwrap();
    claim.metadata.uid = Some(uid.into());
    claim.metadata.resource_version = Some(version.into());
    claim
}

pub(super) fn row(claim: PersistentVolumeClaim) -> (u16, Value) {
    (200, serde_json::to_value(claim).unwrap())
}

pub(super) fn status(code: u16) -> (u16, Value) {
    (
        code,
        json!({"apiVersion":"v1","kind":"Status","status":if code < 400 {"Success"} else {"Failure"},"reason":if code==404 {"NotFound"} else {"Conflict"},"message":"fixture","code":code}),
    )
}

#[tokio::test]
async fn inventory_is_scoped_complete_and_rejects_invalid_later_pages() {
    let page = |items: Vec<PersistentVolumeClaim>, cursor: &str| {
        (
            200,
            json!({
                "apiVersion":"v1", "kind":"PersistentVolumeClaimList",
                "metadata":{"continue":cursor}, "items":items,
            }),
        )
    };
    let fixture = serve(vec![
        page(vec![claim("1", "disk-one")], "next-page"),
        page(vec![], ""),
    ])
    .await;
    let inventory = fixture.store.inventory().await.unwrap();
    assert_eq!(inventory.len(), 1);
    assert_eq!(inventory[0].request, request());
    let requests = fixture.finish().await;
    assert!(requests
        .iter()
        .all(|request| request.starts_with("GET ") && request.contains("labelSelector=")));
    assert!(requests[1].contains("continue=next-page"));
    for bad_page in [
        page(vec![claim("1", "disk-one")], ""),
        status(403),
        page(vec![], "next-page"),
    ] {
        let fixture = serve(vec![
            page(vec![claim("1", "disk-one")], "next-page"),
            bad_page,
        ])
        .await;
        assert!(fixture.store.inventory().await.is_err());
        assert_eq!(fixture.finish().await.len(), 2);
    }
}

pub(super) struct Fixture {
    pub(super) store: PersistentHomeVolumes,
    requests: Arc<Mutex<Vec<String>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Fixture {
    pub(super) async fn finish(mut self) -> Vec<String> {
        tokio::time::timeout(Duration::from_secs(2), &mut self.task)
            .await
            .unwrap()
            .unwrap();
        self.requests.lock().unwrap().clone()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub(super) async fn serve(replies: Vec<(u16, Value)>) -> Fixture {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let observed = requests.clone();
    let task = tokio::spawn(async move {
        for (status, body) in replies {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_request(&mut stream).await;
            observed.lock().unwrap().push(request);
            let body = body.to_string();
            let response = format!("HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            stream.write_all(response.as_bytes()).await.unwrap();
            stream.shutdown().await.unwrap();
        }
    });
    let config = kube::Config::new(format!("http://{address}").parse().unwrap());
    let client = kube::Client::try_from(config).unwrap();
    Fixture {
        store: PersistentHomeVolumes::namespaced(
            client,
            "cma-test",
            "deployment",
            &format!("http://{address}"),
        )
        .unwrap(),
        requests,
        task,
    }
}

#[tokio::test]
async fn allocation_replays_one_initialized_exclusive_claim() {
    let fixture = serve(vec![
        template(),
        status(404),
        row(claim("1", "disk-one")),
        template(),
        row(claim("2", "disk-one")),
    ])
    .await;
    let first = fixture.store.prepare(&request()).await.unwrap();
    assert_eq!(fixture.store.prepare(&request()).await.unwrap(), first);
    let requests = fixture.finish().await;
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.starts_with("POST "))
            .count(),
        1
    );
    let created: Value =
        serde_json::from_str(requests[2].split_once("\r\n\r\n").unwrap().1).unwrap();
    assert_eq!(created["spec"]["accessModes"], json!(["ReadWriteOncePod"]));
    assert_eq!(created["spec"]["dataSource"]["name"], "empty-home");
    assert_eq!(created["spec"]["resources"]["requests"]["storage"], "64Gi");
    assert!(created["metadata"].get("ownerReferences").is_none());
}

#[tokio::test]
async fn concurrent_or_ambiguous_creation_reconciles_only_the_same_request() {
    for code in [409, 500] {
        let fixture = serve(vec![
            template(),
            status(404),
            status(code),
            row(claim("2", "disk-one")),
        ])
        .await;
        assert_eq!(
            fixture
                .store
                .prepare(&request())
                .await
                .unwrap()
                .storage_generation,
            "disk-one"
        );
        assert_eq!(fixture.finish().await.len(), 4);
    }
    let mut foreign = claim("2", "disk-two");
    foreign
        .metadata
        .annotations
        .as_mut()
        .unwrap()
        .insert("cma/home-request-sha256".into(), "different".into());
    let fixture = serve(vec![template(), status(404), status(409), row(foreign)]).await;
    assert!(fixture.store.prepare(&request()).await.is_err());
    assert_eq!(fixture.finish().await.len(), 4);
}

#[tokio::test]
async fn changed_or_unready_template_never_allocates_a_volume() {
    for (path, value) in [
        ("/metadata/uid", json!("another-generation")),
        ("/status/readyToUse", json!(false)),
    ] {
        let mut response = template();
        *response.1.pointer_mut(path).unwrap() = value;
        let fixture = serve(vec![response]).await;
        assert!(fixture.store.prepare(&request()).await.is_err());
        assert_eq!(fixture.finish().await.len(), 1);
    }
}

#[test]
fn claim_validation_rejects_lineage_storage_and_ownership_changes() {
    let original = claim("1", "disk-one");
    let baseline = serde_json::to_value(original).unwrap();
    for (path, value) in [
        ("/metadata/namespace", json!("another")),
        (
            "/metadata/labels/cma~1home-managed-by",
            json!("other-deployment"),
        ),
        (
            "/metadata/annotations/cma~1home-request-sha256",
            json!("changed"),
        ),
        ("/spec/accessModes", json!(["ReadWriteMany"])),
        ("/spec/storageClassName", json!("another")),
        ("/spec/resources/requests/storage", json!("1Gi")),
        ("/spec/dataSource/name", json!("another")),
    ] {
        let mut value_row = baseline.clone();
        *value_row.pointer_mut(path).unwrap() = value;
        let changed = serde_json::from_value(value_row).unwrap();
        assert!(
            identity::validate_claim("cma-test", "deployment", &request(), &changed).is_err(),
            "{path}"
        );
    }
    let receipt = identity::validate_claim(
        "cma-test",
        "deployment",
        &request(),
        &claim("1", "disk-one"),
    )
    .unwrap();
    let mut changed_request = request();
    changed_request.sandbox_id = "another-owner-sandbox".into();
    assert!(receipt.validate_for(&changed_request).is_err());
}

#[tokio::test]
async fn verify_never_recreates_a_missing_or_replaced_recorded_home() {
    let receipt = identity::validate_claim(
        "cma-test",
        "deployment",
        &request(),
        &claim("1", "disk-one"),
    )
    .unwrap();
    for response in [status(404), row(claim("2", "disk-two"))] {
        let fixture = serve(vec![template(), response]).await;
        assert!(fixture
            .store
            .verify(&HomeVolumeAttachment {
                request: request(),
                receipt: receipt.clone()
            })
            .await
            .is_err());
        assert!(fixture
            .finish()
            .await
            .iter()
            .all(|request| request.starts_with("GET ")));
    }
}

#[tokio::test]
async fn removal_uses_both_preconditions_and_preserves_a_replacement() {
    let original = claim("7", "disk-one");
    let receipt =
        identity::validate_claim("cma-test", "deployment", &request(), &original).unwrap();
    let fixture = serve(vec![
        row(original),
        status(200),
        row(claim("8", "disk-two")),
    ])
    .await;
    fixture
        .store
        .remove(&request(), Some(&receipt))
        .await
        .unwrap();
    let requests = fixture.finish().await;
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.starts_with("DELETE "))
            .count(),
        1
    );
    let deletion: Value =
        serde_json::from_str(requests[1].split_once("\r\n\r\n").unwrap().1).unwrap();
    assert_eq!(
        deletion["preconditions"],
        json!({"uid":"disk-one","resourceVersion":"7"})
    );
    let fixture = serve(vec![row(claim("8", "disk-two"))]).await;
    assert!(fixture
        .store
        .remove(&request(), Some(&receipt))
        .await
        .is_err());
    assert_eq!(fixture.finish().await.len(), 1);
}

#[tokio::test]
async fn foreign_allocation_scope_is_rejected_before_any_kubernetes_request() {
    let fixture = serve(vec![]).await;
    let mut home = request();
    home.scope =
        Some(crate::HomeVolumeScope::from_authority("kubernetes-pvc-v1", &["foreign"]).unwrap());
    let attachment = crate::HomeVolumeAttachment {
        receipt: crate::HomeVolumeReceipt {
            native_authority: None,
            volume_id: home.volume_id.clone(),
            request_sha256: home.computed_sha256(),
            storage_reference: "original-pvc".into(),
            storage_generation: "original-uid".into(),
        },
        request: home.clone(),
    };
    assert!(matches!(
        fixture.store.prepare(&home).await,
        Err(crate::AdapterError::Rejected(_))
    ));
    assert!(matches!(
        fixture.store.prepare_receipted(&home, "original").await,
        Err(crate::AdapterError::Rejected(_))
    ));
    assert!(matches!(
        fixture.store.verify(&attachment).await,
        Err(crate::AdapterError::Rejected(_))
    ));
    assert!(matches!(
        fixture.store.remove_captures(&attachment).await,
        Err(crate::AdapterError::Rejected(_))
    ));
    assert!(matches!(
        fixture.store.remove(&home, None).await,
        Err(crate::AdapterError::Rejected(_))
    ));
    assert!(fixture.finish().await.is_empty());
}
