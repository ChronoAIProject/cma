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
        self.requests.lock().unwrap().clone()
    }
    async fn original(&self) -> RuntimeProvisionReceipt {
        let observation = self
            .reclaimer
            .begin_creation_observation(OPERATION)
            .await
            .unwrap();
        self.reclaimer
            .capture_created(
                &RuntimeHandle::new(HANDLE),
                OPERATION,
                &"a".repeat(64),
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
fn added() -> (u16, String) {
    row(json!({"type":"ADDED","object":object("original-uid","11")}))
}
fn success() -> (u16, String) {
    row(json!({"apiVersion":"v1","kind":"Status","status":"Success","code":200}))
}

#[tokio::test]
async fn creation_watch_pins_first_uid_and_rejects_replacement_with_copied_labels() {
    let stream = [
        json!({"type":"ADDED","object":object("original-uid","11")}),
        json!({"type":"DELETED","object":object("original-uid","12")}),
        json!({"type":"ADDED","object":object("replacement-uid","13")}),
    ]
    .map(|value| value.to_string())
    .join("\n");
    let fixture = serve(vec![
        baseline(),
        (200, stream),
        row(object("replacement-uid", "13")),
    ])
    .await;
    let original = fixture.original().await;
    assert_eq!(original.incarnation.uid, "original-uid");
    assert_eq!(original.incarnation.creation_resource_version, "11");
    assert_eq!(original.creation_observation_version, "10");
    assert!(fixture.reclaimer.require_original(&original).await.is_err());
    let requests = fixture.finish().await;
    let query = requests[1].lines().next().unwrap();
    assert!(query.contains("watch=true"));
    assert!(query.contains("resourceVersion=10"));
    assert!(query.contains("fieldSelector=metadata.name%3Druntime-finality"));
    assert!(!query.contains("labelSelector"));
}

#[tokio::test]
async fn creation_watch_rejects_missing_original_event_and_compacted_history() {
    for event in [
        json!({"type":"MODIFIED","object":object("copied-uid","12")}),
        json!({"type":"DELETED","object":object("lost-uid","12")}),
        json!({"type":"ERROR","object":{"apiVersion":"v1","kind":"Status","status":"Failure","message":"too old resource version","reason":"Expired","code":410}}),
    ] {
        let fixture = serve(vec![baseline(), row(event)]).await;
        let observation = fixture
            .reclaimer
            .begin_creation_observation(OPERATION)
            .await
            .unwrap();
        assert!(fixture
            .reclaimer
            .capture_created(
                &RuntimeHandle::new(HANDLE),
                OPERATION,
                &"a".repeat(64),
                observation
            )
            .await
            .is_err());
        assert_eq!(fixture.finish().await.len(), 2);
    }
    let fixture = serve(vec![baseline(), (200, String::new())]).await;
    let observation = fixture
        .reclaimer
        .begin_creation_observation(OPERATION)
        .await
        .unwrap();
    assert!(fixture
        .reclaimer
        .capture_created(
            &RuntimeHandle::new(HANDLE),
            OPERATION,
            &"a".repeat(64),
            observation
        )
        .await
        .is_err());
    fixture.finish().await;
}

#[tokio::test]
async fn creation_observation_rejects_existing_dispatch_or_incomplete_baseline() {
    for initial in [
        list("BatchSandboxList", "10", vec![object("old-uid", "9")], ""),
        list("BatchSandboxList", "10", vec![], "more"),
    ] {
        let fixture = serve(vec![initial]).await;
        assert!(fixture
            .reclaimer
            .begin_creation_observation(OPERATION)
            .await
            .is_err());
        assert_eq!(fixture.finish().await.len(), 1);
    }
}

#[tokio::test]
async fn finality_requires_acknowledged_uid_delete_and_complete_absence() {
    let fixture = serve(vec![
        baseline(),
        added(),
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
        .finalize_original(&original, "delete-1")
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
        added(),
        row(object("original-uid", "15")),
        (404, missing.to_string()),
    ])
    .await;
    let original = fixture.original().await;
    assert!(fixture
        .reclaimer
        .finalize_original(&original, "delete-1")
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
            added(),
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
            .finalize_original(&original, "delete-1")
            .await
            .is_err());
        assert_eq!(fixture.finish().await.len(), 7);
    }
}
