use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::AsyncWriteExt;

use super::reclaimer::BatchSandboxReclaimer;
use super::tests::read_request;
use super::ProviderError;
use crate::RuntimeHandle;

const HANDLE: &str = "runtime-test";
const NAMESPACE: &str = "cma-test";

struct Fixture {
    reclaimer: BatchSandboxReclaimer,
    requests: Arc<Mutex<Vec<String>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Fixture {
    async fn finish(mut self) -> Vec<String> {
        tokio::time::timeout(Duration::from_secs(2), &mut self.task)
            .await
            .expect("Kubernetes fixture received every request")
            .expect("Kubernetes fixture did not panic");
        self.requests.lock().expect("request lock").clone()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn serve(replies: Vec<(u16, Value)>) -> Fixture {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("fixture binds");
    let address = listener.local_addr().expect("fixture address");
    let requests = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&requests);
    let task = tokio::spawn(async move {
        for (status, body) in replies {
            let (mut stream, _) = listener.accept().await.expect("fixture accepts");
            let request = read_request(&mut stream).await;
            observed.lock().expect("request lock").push(request);
            let reason = match status {
                200 => "OK",
                403 => "Forbidden",
                404 => "Not Found",
                409 => "Conflict",
                _ => panic!("unsupported fixture status {status}"),
            };
            let body = body.to_string();
            let response = format!(
                "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len(),
            );
            stream
                .write_all(response.as_bytes())
                .await
                .expect("fixture writes");
            stream.shutdown().await.expect("fixture closes");
        }
    });
    let config = kube::Config::new(format!("http://{address}").parse().expect("fixture URI"));
    let client = kube::Client::try_from(config).expect("fixture Kubernetes client");
    Fixture {
        reclaimer: BatchSandboxReclaimer::namespaced(client, NAMESPACE).for_creator("cma-test"),
        requests,
        task,
    }
}

fn object(resource_version: &str) -> Value {
    json!({
        "apiVersion": "sandbox.opensandbox.io/v1alpha1",
        "kind": "BatchSandbox",
        "metadata": {
            "name": HANDLE,
            "namespace": NAMESPACE,
            "uid": "captured-uid",
            "resourceVersion": resource_version,
            "generation": 1,
            "labels": {
                "opensandbox.io/id": HANDLE,
                "cma/control-plane": "cma-test",
                "cma/sandbox": "sbx-test",
                "cma/runtime-generation": "3",
                "cma/spec-sha256": "spec-test",
            },
            "ownerReferences": [{
                "apiVersion": "v1",
                "kind": "ConfigMap",
                "name": "owner-test",
                "uid": "owner-uid",
            }],
        },
        "spec": { "replicas": 1, "podTemplate": { "image": "runtime:test" } },
        "status": { "phase": "Running" },
    })
}

fn status(code: u16) -> (u16, Value) {
    let (state, reason) = match code {
        200 => ("Success", "Success"),
        403 => ("Failure", "Forbidden"),
        404 => ("Failure", "NotFound"),
        409 => ("Failure", "Conflict"),
        _ => panic!("unsupported fixture status {code}"),
    };
    (
        code,
        json!({
            "apiVersion": "v1", "kind": "Status", "status": state,
            "reason": reason, "message": reason, "code": code,
        }),
    )
}

fn deletion_preconditions(requests: &[String]) -> Vec<Value> {
    for request in requests {
        assert!(request.lines().next().expect("request line").contains(
            "/apis/sandbox.opensandbox.io/v1alpha1/namespaces/cma-test/batchsandboxes/runtime-test"
        ));
    }
    requests
        .iter()
        .filter(|request| request.starts_with("DELETE "))
        .map(|request| {
            let (_, body) = request.split_once("\r\n\r\n").expect("HTTP body");
            let body: Value = serde_json::from_str(body).expect("DELETE JSON");
            assert_eq!(body["gracePeriodSeconds"], 0);
            assert_eq!(body["propagationPolicy"], "Foreground");
            body["preconditions"].clone()
        })
        .collect()
}

#[tokio::test]
async fn delete_refreshes_only_resource_version_after_status_conflict() {
    let mut changed = object("12");
    changed["status"] = json!({ "phase": "Succeed", "conditions": [{ "type": "Ready" }] });
    let fixture = serve(vec![
        (200, object("11")),
        status(409),
        (200, changed),
        status(200),
        status(404),
    ])
    .await;
    let captured = fixture
        .reclaimer
        .capture(&RuntimeHandle::new(HANDLE))
        .await
        .expect("capture succeeds")
        .expect("sandbox exists");
    fixture
        .reclaimer
        .delete(captured)
        .await
        .expect("status-only resourceVersion conflict is retried with the original identity");
    assert_eq!(
        deletion_preconditions(&fixture.finish().await),
        vec![
            json!({ "uid": "captured-uid", "resourceVersion": "11" }),
            json!({ "uid": "captured-uid", "resourceVersion": "12" }),
        ],
    );
}

#[tokio::test]
async fn delete_never_refreshes_changed_identity_ownership_generation_or_spec() {
    for (path, replacement) in [
        ("/metadata/name", json!("another-runtime")),
        ("/metadata/namespace", json!("another-namespace")),
        ("/metadata/uid", json!("another-uid")),
        ("/metadata/generation", json!(2)),
        (
            "/metadata/labels/opensandbox.io~1id",
            json!("another-runtime"),
        ),
        (
            "/metadata/labels/cma~1control-plane",
            json!("another-owner"),
        ),
        ("/metadata/labels/cma~1sandbox", json!("another-sandbox")),
        ("/metadata/labels/cma~1runtime-generation", json!("4")),
        ("/metadata/labels/cma~1spec-sha256", json!("another-spec")),
        (
            "/metadata/ownerReferences/0/uid",
            json!("another-owner-uid"),
        ),
        ("/spec/podTemplate/image", json!("runtime:changed")),
    ] {
        let mut changed = object("12");
        *changed.pointer_mut(path).expect("fixture field") = replacement;
        let fixture = serve(vec![(200, object("11")), status(409), (200, changed)]).await;
        let captured = fixture
            .reclaimer
            .capture(&RuntimeHandle::new(HANDLE))
            .await
            .expect("capture succeeds")
            .expect("sandbox exists");
        let error = fixture.reclaimer.delete(captured).await.unwrap_err();
        assert!(
            matches!(error, ProviderError::IdentityMismatch(_)),
            "changed {path} must refuse deletion: {error:?}",
        );
        assert_eq!(
            deletion_preconditions(&fixture.finish().await),
            vec![json!({ "uid": "captured-uid", "resourceVersion": "11" })],
            "changed {path} must never receive another DELETE",
        );
    }
}

#[tokio::test]
async fn delete_refuses_missing_identity_evidence_on_capture_or_conflict() {
    for path in [
        "/metadata/uid",
        "/metadata/resourceVersion",
        "/metadata/generation",
        "/metadata/labels",
        "/spec",
    ] {
        for during_conflict in [false, true] {
            let mut malformed = object("12");
            *malformed.pointer_mut(path).expect("fixture field") = Value::Null;
            let replies = if during_conflict {
                vec![(200, object("11")), status(409), (200, malformed)]
            } else {
                vec![(200, malformed)]
            };
            let fixture = serve(replies).await;
            let captured = fixture.reclaimer.capture(&RuntimeHandle::new(HANDLE)).await;
            let result = if during_conflict {
                fixture
                    .reclaimer
                    .delete(
                        captured
                            .expect("first capture succeeds")
                            .expect("sandbox exists"),
                    )
                    .await
            } else {
                captured.map(|_| ())
            };
            assert!(
                matches!(
                    result,
                    Err(ProviderError::Malformed(_) | ProviderError::IdentityMismatch(_))
                ),
                "missing {path} must refuse deletion",
            );
            let requests = fixture.finish().await;
            assert_eq!(
                deletion_preconditions(&requests).len(),
                usize::from(during_conflict),
                "missing {path} must not receive another DELETE",
            );
        }
    }
}

#[tokio::test]
async fn delete_never_retries_permission_errors_or_unbounded_conflicts() {
    for code in [403, 409] {
        let replies = if code == 409 {
            vec![
                (200, object("11")),
                status(409),
                (200, object("12")),
                status(409),
                (200, object("13")),
                status(409),
            ]
        } else {
            vec![(200, object("11")), status(code)]
        };
        let fixture = serve(replies).await;
        let captured = fixture
            .reclaimer
            .capture(&RuntimeHandle::new(HANDLE))
            .await
            .expect("capture succeeds")
            .expect("sandbox exists");
        let error = fixture.reclaimer.delete(captured).await.unwrap_err();
        assert!(matches!(error, ProviderError::Rejected { status, .. } if status == code));
        let conditions = deletion_preconditions(&fixture.finish().await);
        let versions = if code == 409 {
            vec!["11", "12", "13"]
        } else {
            vec!["11"]
        };
        assert_eq!(
            conditions,
            versions
                .into_iter()
                .map(|version| json!({ "uid": "captured-uid", "resourceVersion": version }))
                .collect::<Vec<_>>(),
        );
    }
}

#[tokio::test]
async fn delete_accepts_only_kubernetes_confirmed_absence() {
    for replies in [
        vec![(200, object("11")), status(200), status(404)],
        vec![(200, object("11")), status(404)],
        vec![(200, object("11")), status(409), status(404)],
    ] {
        let fixture = serve(replies).await;
        let captured = fixture
            .reclaimer
            .capture(&RuntimeHandle::new(HANDLE))
            .await
            .expect("capture succeeds")
            .expect("sandbox exists");
        fixture
            .reclaimer
            .delete(captured)
            .await
            .expect("runtime absent");
        assert_eq!(
            deletion_preconditions(&fixture.finish().await),
            vec![json!({ "uid": "captured-uid", "resourceVersion": "11" })],
        );
    }
}

#[tokio::test]
async fn delete_confirms_original_resource_disappeared_without_touching_replacement() {
    let mut replacement = object("21");
    replacement["metadata"]["uid"] = json!("replacement-uid");
    let fixture = serve(vec![(200, object("11")), status(200), (200, replacement)]).await;
    let captured = fixture
        .reclaimer
        .capture(&RuntimeHandle::new(HANDLE))
        .await
        .expect("capture succeeds")
        .expect("sandbox exists");
    fixture
        .reclaimer
        .delete(captured)
        .await
        .expect("original absent");
    assert_eq!(
        deletion_preconditions(&fixture.finish().await),
        vec![json!({ "uid": "captured-uid", "resourceVersion": "11" })],
    );
}

#[tokio::test]
async fn delete_never_treats_unreadable_or_missing_confirmation_identity_as_absence() {
    let mut malformed = object("12");
    malformed["metadata"]["uid"] = Value::Null;
    for reply in [status(403), (200, malformed)] {
        let fixture = serve(vec![(200, object("11")), status(200), reply]).await;
        let captured = fixture
            .reclaimer
            .capture(&RuntimeHandle::new(HANDLE))
            .await
            .expect("capture succeeds")
            .expect("sandbox exists");
        assert!(fixture.reclaimer.delete(captured).await.is_err());
        assert_eq!(
            deletion_preconditions(&fixture.finish().await),
            vec![json!({ "uid": "captured-uid", "resourceVersion": "11" })],
        );
    }
}

#[tokio::test]
async fn fenced_client_never_falls_back_to_provider_delete_after_identity_change() {
    let mut replacement = object("21");
    replacement["metadata"]["uid"] = json!("replacement-uid");
    let fixture = serve(vec![(200, object("11")), status(409), (200, replacement)]).await;
    let client = super::SandboxClient::new("http://127.0.0.1:1", "fixture-no-credential")
        .with_reclaimer(fixture.reclaimer.clone());
    let result = client.delete(&RuntimeHandle::new(HANDLE)).await;
    assert!(matches!(result, Err(ProviderError::IdentityMismatch(_))));
    assert_eq!(
        deletion_preconditions(&fixture.finish().await),
        vec![json!({ "uid": "captured-uid", "resourceVersion": "11" })],
    );
}
