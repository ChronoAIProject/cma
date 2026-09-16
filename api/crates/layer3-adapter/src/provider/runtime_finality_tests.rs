use super::*;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tokio::io::AsyncWriteExt;

const HANDLE: &str = "runtime-finality";
const OPERATION: &str = "original-dispatch-1";

struct Fixture {
    reclaimer: BatchSandboxReclaimer,
    requests: Arc<Mutex<Vec<String>>>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Fixture {
    async fn finish(mut self) -> Vec<String> {
        tokio::time::timeout(Duration::from_secs(2), &mut self.task)
            .await
            .expect("every expected provider request was sent")
            .expect("fixture completed");
        let requests = self.requests.lock().unwrap().clone();
        assert!(requests.iter().all(|request| !request.contains("watch=")));
        requests
    }
    async fn original(&self) -> RuntimeProvisionReceipt {
        let observation = self
            .reclaimer
            .observe_before_dispatch(OPERATION)
            .await
            .unwrap();
        self.reclaimer
            .capture_for_binding(
                &RuntimeHandle::new(HANDLE),
                OPERATION,
                &"a".repeat(64),
                &"b".repeat(64),
                &request_metadata(),
                observation,
            )
            .await
            .unwrap()
    }
}

async fn serve(replies: Vec<(u16, String)>) -> Fixture {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let observed = requests.clone();
    let task = tokio::spawn(async move {
        for (status, body) in replies {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = crate::provider::tests::read_request(&mut stream).await;
            observed.lock().unwrap().push(request);
            let response = format!(
                "HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
            stream.shutdown().await.unwrap();
        }
    });
    let config = kube::Config::new(format!("http://{address}").parse().unwrap());
    Fixture {
        reclaimer: BatchSandboxReclaimer::namespaced(
            kube::Client::try_from(config).unwrap(),
            "cma-test",
        )
        .for_creator("cma-test")
        .with_finality_authority("https://original-cluster")
        .unwrap(),
        requests,
        task,
    }
}

fn object(uid: &str, version: &str) -> Value {
    json!({
        "apiVersion":"sandbox.opensandbox.io/v1alpha1", "kind":"BatchSandbox",
        "metadata":{
            "name":HANDLE, "namespace":"cma-test", "uid":uid,
            "resourceVersion":version, "generation":1,
            "labels":{
                "opensandbox.io/id":HANDLE, "cma/control-plane":"cma-test",
                "cma/sandbox":"sandbox-original", "cma/runtime-generation":"1",
                "cma/spec-sha256":"original-spec",
                DISPATCH_LABEL:dispatch_marker(OPERATION).unwrap()
            }
        },
        "spec":{"replicas":1}, "status":{"phase":"Running"}
    })
}
fn row(value: Value) -> (u16, String) {
    (200, value.to_string())
}
fn list(kind: &str, version: &str, rows: Vec<Value>, cursor: &str) -> (u16, String) {
    row(json!({
        "apiVersion":"v1", "kind":kind,
        "metadata":{"resourceVersion":version,"continue":cursor}, "items":rows
    }))
}
fn baseline() -> (u16, String) {
    list("BatchSandboxList", "10", vec![], "")
}
fn pinned() -> (u16, String) {
    row(object("original-uid", "11"))
}
fn request_metadata() -> BTreeMap<String, String> {
    BTreeMap::from([
        ("cma/control-plane".into(), "cma-test".into()),
        ("cma/sandbox".into(), "sandbox-original".into()),
        ("cma/runtime-generation".into(), "1".into()),
        ("cma/spec-sha256".into(), "original-spec".into()),
        (DISPATCH_LABEL.into(), dispatch_marker(OPERATION).unwrap()),
    ])
}
fn success() -> (u16, String) {
    row(json!({"apiVersion":"v1","kind":"Status","status":"Success","code":200}))
}

#[tokio::test]
async fn binding_get_pins_uid_and_rejects_later_replacement_with_copied_labels_without_watch() {
    let fixture = serve(vec![
        baseline(),
        pinned(),
        row(object("original-uid", "12")),
        row(object("replacement-uid", "13")),
    ])
    .await;
    let original = fixture.original().await;
    assert_eq!(original.incarnation.uid, "original-uid");
    assert_eq!(original.incarnation.binding_resource_version, "11");
    assert_eq!(original.pre_dispatch_list_version, "10");
    assert_eq!(original.provider_response_sha256, "b".repeat(64));
    fixture
        .reclaimer
        .require_bound_runtime(&original)
        .await
        .unwrap();
    assert!(fixture
        .reclaimer
        .require_bound_runtime(&original)
        .await
        .is_err());
    let requests = fixture.finish().await;
    assert!(requests[0].contains("labelSelector=cma%2Fprovider-dispatch%3D"));
    let query = requests[1].lines().next().unwrap();
    assert_eq!(query, "GET /apis/sandbox.opensandbox.io/v1alpha1/namespaces/cma-test/batchsandboxes/runtime-finality HTTP/1.1");
}

#[tokio::test]
async fn binding_get_requires_present_readable_object_and_exact_request_metadata() {
    let mut changed_metadata = object("candidate-uid", "11");
    changed_metadata["metadata"]["labels"]["cma/sandbox"] = json!("another-sandbox");
    let mut missing_metadata = object("candidate-uid", "11");
    missing_metadata["metadata"]["labels"]
        .as_object_mut()
        .unwrap()
        .remove("cma/spec-sha256");
    let mut copied_dispatch = object("candidate-uid", "11");
    copied_dispatch["metadata"]["labels"][DISPATCH_LABEL] = json!("another-dispatch");
    let missing = json!({"apiVersion":"v1","kind":"Status","status":"Failure","reason":"NotFound","message":"missing","code":404});
    let forbidden = json!({"apiVersion":"v1","kind":"Status","status":"Failure","reason":"Forbidden","message":"forbidden","code":403});
    for response in [
        (404, missing.to_string()),
        (403, forbidden.to_string()),
        (200, String::new()),
        row(changed_metadata),
        row(missing_metadata),
        row(copied_dispatch),
    ] {
        let fixture = serve(vec![baseline(), response]).await;
        let observation = fixture
            .reclaimer
            .observe_before_dispatch(OPERATION)
            .await
            .unwrap();
        assert!(fixture
            .reclaimer
            .capture_for_binding(
                &RuntimeHandle::new(HANDLE),
                OPERATION,
                &"a".repeat(64),
                &"b".repeat(64),
                &request_metadata(),
                observation,
            )
            .await
            .is_err());
        assert_eq!(fixture.finish().await.len(), 2);
    }
}

#[tokio::test]
async fn binding_get_compares_encoded_hash_labels_to_canonical_request_metadata() {
    let canonical = format!("-{}", "a".repeat(42));
    let mut candidate = object("original-uid", "11");
    candidate["metadata"]["labels"]["cma/spec-sha256"] = json!(format!("h.{canonical}"));
    let fixture = serve(vec![baseline(), row(candidate)]).await;
    let observation = fixture
        .reclaimer
        .observe_before_dispatch(OPERATION)
        .await
        .unwrap();
    let mut metadata = request_metadata();
    metadata.insert("cma/spec-sha256".into(), canonical);
    let receipt = fixture
        .reclaimer
        .capture_for_binding(
            &RuntimeHandle::new(HANDLE),
            OPERATION,
            &"a".repeat(64),
            &"b".repeat(64),
            &metadata,
            observation,
        )
        .await
        .unwrap();
    assert_eq!(receipt.incarnation.uid, "original-uid");
    fixture.finish().await;
}

#[tokio::test]
async fn pre_dispatch_list_rejects_existing_marker_or_incomplete_baseline() {
    for initial in [
        list("BatchSandboxList", "10", vec![object("old-uid", "9")], ""),
        list("BatchSandboxList", "10", vec![], "more"),
    ] {
        let fixture = serve(vec![initial]).await;
        assert!(fixture
            .reclaimer
            .observe_before_dispatch(OPERATION)
            .await
            .is_err());
        assert_eq!(fixture.finish().await.len(), 1);
    }
}

#[tokio::test]
async fn finality_requires_acknowledged_uid_delete_and_complete_absence() {
    let fixture = serve(vec![
        baseline(),
        pinned(),
        row(object("original-uid", "15")),
        success(),
        list("BatchSandboxList", "20", vec![], ""),
        list("PodList", "21", vec![], "next-page"),
        list("PodList", "21", vec![], ""),
    ])
    .await;
    let original = fixture.original().await;
    let proof = fixture
        .reclaimer
        .finalize_bound_runtime(&original, "delete-1")
        .await
        .unwrap();
    assert_eq!(proof.original, original);
    assert_eq!(proof.delete_resource_version, "15");
    assert_eq!(proof.absent_runtime_list_version, "20");
    assert_eq!(proof.absent_owned_pods_list_version, "21");
    let requests = fixture.finish().await;
    let delete: Value =
        serde_json::from_str(requests[3].split_once("\r\n\r\n").unwrap().1).unwrap();
    assert_eq!(
        delete["preconditions"],
        json!({"uid":"original-uid","resourceVersion":"15"})
    );
    assert_eq!(delete["propagationPolicy"], "Foreground");
    assert!(requests[6].contains("continue=next-page"));
}

#[tokio::test]
async fn missing_delete_response_cannot_be_replaced_by_absence() {
    let missing = json!({"apiVersion":"v1","kind":"Status","status":"Failure","reason":"NotFound","message":"missing","code":404});
    let fixture = serve(vec![
        baseline(),
        pinned(),
        row(object("original-uid", "15")),
        (404, missing.to_string()),
    ])
    .await;
    let original = fixture.original().await;
    assert!(fixture
        .reclaimer
        .finalize_bound_runtime(&original, "delete-1")
        .await
        .is_err());
    assert_eq!(fixture.finish().await.len(), 4);
}

#[tokio::test]
async fn incomplete_or_inconsistent_owned_pod_inventory_cannot_authorize_finality() {
    for second_page in [
        list("PodList", "21", vec![], "next-page"),
        list("PodList", "22", vec![], ""),
    ] {
        let fixture = serve(vec![
            baseline(),
            pinned(),
            row(object("original-uid", "15")),
            success(),
            list("BatchSandboxList", "20", vec![], ""),
            list("PodList", "21", vec![], "next-page"),
            second_page,
        ])
        .await;
        let original = fixture.original().await;
        assert!(fixture
            .reclaimer
            .finalize_bound_runtime(&original, "delete-1")
            .await
            .is_err());
        assert_eq!(fixture.finish().await.len(), 7);
    }
}
