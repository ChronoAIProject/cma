//! Exercise real kube-rs HTTP requests, including response loss and UID fencing.
use super::*;
use crate::{HomeVolumePolicy, HomeVolumeRequest};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[path = "pod_read_tests.rs"]
mod pod_read_tests;

#[path = "observation_tests.rs"]
mod observation_tests;

#[path = "startup_provision_tests.rs"]
mod startup_provision_tests;

struct Fixture {
    client: AgentSandboxClient,
    calls: Arc<Mutex<Vec<(String, Value)>>>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Fixture {
    async fn start(replies: Vec<(u16, Value)>) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let observed = calls.clone();
        let task = tokio::spawn(async move {
            for (status, body) in replies {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut bytes = Vec::new();
                let (header_end, length) = loop {
                    let mut buf = [0; 8192];
                    let count = socket.read(&mut buf).await.unwrap();
                    assert_ne!(count, 0);
                    bytes.extend_from_slice(&buf[..count]);
                    if let Some(end) = bytes.windows(4).position(|v| v == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&bytes[..end]);
                        let length = headers
                            .lines()
                            .find_map(|s| {
                                s.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|v| v.trim().parse::<usize>().unwrap())
                            })
                            .unwrap_or(0);
                        break (end + 4, length);
                    }
                };
                while bytes.len() < header_end + length {
                    let mut buf = [0; 8192];
                    let count = socket.read(&mut buf).await.unwrap();
                    assert_ne!(count, 0);
                    bytes.extend_from_slice(&buf[..count]);
                }
                let line = String::from_utf8_lossy(&bytes[..header_end])
                    .lines()
                    .next()
                    .unwrap()
                    .to_owned();
                let request = if length == 0 {
                    Value::Null
                } else {
                    serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap()
                };
                observed.lock().unwrap().push((line, request));
                if status == 0 {
                    continue;
                } // successful create, lost response
                let body = body.to_string();
                socket.write_all(format!("HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
            }
        });
        let client = AgentSandboxClient::new(
            kube::Client::try_from(kube::Config::new(
                format!("http://{address}").parse().unwrap(),
            ))
            .unwrap(),
            super::tests::config(),
            "fixture",
        )
        .unwrap();
        Self {
            client,
            calls,
            task,
        }
    }
}
fn object() -> (crate::provider::ProviderSandboxRequest, Value) {
    let mut request = super::tests::request();
    request.volumes.clear();
    let config = super::tests::config();
    // Derive the same control token as the client without creating network IO.
    let client = AgentSandboxClient::new(
        kube::Client::try_from(kube::Config::new("http://127.0.0.1:1".parse().unwrap())).unwrap(),
        config.clone(),
        "fixture",
    )
    .unwrap();
    let name = model::name(&request).unwrap();
    let desired = model::desired(&config, &request, &client.control_token(&name)).unwrap();
    let mut object = serde_json::to_value(desired).unwrap();
    object["metadata"]["uid"] = json!("sandbox-uid");
    object["metadata"]["resourceVersion"] = json!("7");
    object["metadata"]["generation"] = json!(1);
    (request, object)
}
fn missing() -> Value {
    json!({"apiVersion":"v1","kind":"Status","status":"Failure","reason":"NotFound","code":404,"message":"absent"})
}
fn list(kind: &str) -> Value {
    json!({"apiVersion":"v1","kind":kind,"metadata":{"resourceVersion":"12"},"items":[]})
}

#[tokio::test]
async fn lost_create_response_recovers_same_uid_and_changed_spec_is_refused() {
    let (request, object) = object();
    let fixture = Fixture::start(vec![
        (404, missing()),
        (0, Value::Null),
        (200, object.clone()),
        (200, object),
    ])
    .await;
    let handle = fixture.client.create(&request).await.unwrap();
    assert!(handle.0.ends_with(":sandbox-uid"));
    let mut changed = request;
    changed.resource_limits.insert("cpu".into(), "1000m".into());
    assert!(fixture.client.create(&changed).await.is_err());
    let calls = fixture.calls.lock().unwrap();
    assert_eq!(
        calls
            .iter()
            .filter(|(line, _)| line.starts_with("POST "))
            .count(),
        1
    );
    assert!(calls[1]
        .0
        .contains("/apis/agents.x-k8s.io/v1beta1/namespaces/cma-test/sandboxes"));
}

#[tokio::test]
async fn suspend_and_delete_send_exact_uid_version_and_foreground_policy() {
    let (_, object) = object();
    let handle = RuntimeHandle(format!(
        "agent-sandbox:{}:sandbox-uid",
        object["metadata"]["name"].as_str().unwrap()
    ));
    let mut suspended = object.clone();
    suspended["spec"]["operatingMode"] = json!("Suspended");
    suspended["metadata"]["resourceVersion"] = json!("8");
    let fixture = Fixture::start(vec![
        (200, object),
        (200, suspended.clone()),
        (200, suspended.clone()),
        (200, suspended),
        (200, list("SandboxList")),
        (200, list("PodList")),
    ])
    .await;
    fixture.client.set_suspended(&handle, true).await.unwrap();
    fixture.client.delete(&handle).await.unwrap();
    let calls = fixture.calls.lock().unwrap();
    assert!(calls[1].0.starts_with("PATCH "));
    assert_eq!(
        calls[1].1["metadata"],
        json!({"uid":"sandbox-uid","resourceVersion":"7"})
    );
    assert_eq!(calls[1].1["spec"]["operatingMode"], "Suspended");
    assert!(calls[3].0.starts_with("DELETE "));
    assert_eq!(
        calls[3].1["preconditions"],
        json!({"uid":"sandbox-uid","resourceVersion":"8"})
    );
    assert_eq!(calls[3].1["propagationPolicy"], "Foreground");
    assert!(calls[4].0.contains("fieldSelector=metadata.name%3D"));
    assert!(!calls[4].0.contains("labelSelector="));
    assert!(calls[5].0.contains("/namespaces/cma-test/pods?"));
    assert!(!calls[5].0.contains("labelSelector="));
}

#[tokio::test]
async fn deletion_retry_waits_for_the_original_pod_even_after_the_crd_is_gone() {
    let (_, object) = object();
    let name = object["metadata"]["name"].as_str().unwrap();
    let handle = RuntimeHandle(format!("agent-sandbox:{name}:sandbox-uid"));
    let mut pods = list("PodList");
    pods["items"] = json!([{"apiVersion":"v1","kind":"Pod","metadata":{
        "name":"lingering-runtime","namespace":"cma-test","uid":"pod-uid",
        "ownerReferences":[{"apiVersion":"agents.x-k8s.io/v1beta1","kind":"Sandbox",
            "name":name,"uid":"sandbox-uid","controller":true}]}}]);
    let fixture = Fixture::start(vec![
        (404, missing()),
        (404, missing()),
        (200, list("SandboxList")),
        (200, pods),
        (200, list("SandboxList")),
        (200, list("PodList")),
    ])
    .await;
    fixture.client.delete(&handle).await.unwrap();
    let calls = fixture.calls.lock().unwrap();
    assert_eq!(
        calls.len(),
        6,
        "must observe the Pod disappearing before success"
    );
    assert!(calls.iter().all(|(line, _)| line.starts_with("GET ")));
    assert!(calls[3].0.contains("/pods?"));
    assert!(calls[5].0.contains("/pods?"));
    assert!(!calls[3].0.contains("labelSelector="));
    assert!(!calls[5].0.contains("labelSelector="));
}

#[tokio::test]
async fn absent_crd_cannot_complete_deletion_with_an_incomplete_pod_inventory() {
    let (_, object) = object();
    let handle = RuntimeHandle(format!(
        "agent-sandbox:{}:sandbox-uid",
        object["metadata"]["name"].as_str().unwrap()
    ));
    let mut pods = list("PodList");
    pods["metadata"]["continue"] = json!("next-page");
    let mut later = list("PodList");
    later["metadata"]["resourceVersion"] = json!("13");
    let fixture = Fixture::start(vec![
        (404, missing()),
        (404, missing()),
        (200, list("SandboxList")),
        (200, pods),
        (200, later),
    ])
    .await;
    assert!(matches!(
        fixture.client.delete(&handle).await,
        Err(AdapterError::Unavailable(_))
    ));
}

#[tokio::test]
async fn warm_member_pod_discovery_requires_tracking_label_and_exact_owner_without_runtime_label() {
    let (_, mut object) = object();
    object["status"]["conditions"] =
        json!([{"type":"Ready","status":"True","observedGeneration":1}]);
    let name = object["metadata"]["name"].as_str().unwrap();
    let handle = RuntimeHandle(format!("agent-sandbox:{name}:sandbox-uid"));
    for case in [
        "warm",
        "foreign_uid",
        "foreign_hash",
        "foreign_owner",
        "duplicate",
        "incomplete",
    ] {
        let mut pod = json!({"apiVersion":"v1","kind":"Pod", "metadata":{
            "name":name,"namespace":"cma-test","uid":"warm-pod",
            "labels":{OWNER_LABEL:"cma-test",pods::TRACKING_LABEL:pods::name_hash(name)},
            "ownerReferences":[{"apiVersion":"agents.x-k8s.io/v1beta1","kind":"Sandbox","name":name,"uid":"sandbox-uid","controller":true}]},
            "status":{"phase":"Running","podIP":"10.0.0.7","conditions":[{"type":"Ready","status":"True"}]}});
        match case {
            "foreign_uid" => pod["metadata"]["ownerReferences"][0]["uid"] = json!("replacement"),
            "foreign_hash" => pod["metadata"]["labels"][pods::TRACKING_LABEL] = json!("00000000"),
            "foreign_owner" => pod["metadata"]["labels"][OWNER_LABEL] = json!("other-team"),
            _ => (),
        }
        let mut candidates = list("PodList");
        candidates["items"] = if case == "duplicate" {
            json!([pod.clone(), pod])
        } else {
            json!([pod])
        };
        if case == "incomplete" {
            candidates["metadata"]["continue"] = json!("next");
        }
        let fixture = pod_read_tests::Reads::new((200, object.clone()), (200, candidates));
        let result = fixture.client.pod_binding(&handle).await;
        assert_eq!(result.is_ok(), case == "warm", "{case}");
        if let Ok(binding) = result {
            assert_eq!(binding.pod_uid, "warm-pod");
            assert_eq!(binding.sandbox_uid, "sandbox-uid");
        }
        let calls = fixture.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        let query = reqwest::Url::parse(&format!(
            "http://fixture{}",
            calls
                .iter()
                .find(|uri| uri.path().ends_with("/pods"))
                .unwrap()
        ))
        .unwrap();
        assert_eq!(
            query
                .query_pairs()
                .find(|(k, _)| k == "labelSelector")
                .unwrap()
                .1,
            pods::selector(name)
        );
    }
}

#[tokio::test]
async fn deletion_observes_original_crd_uid_even_when_its_management_label_changes() {
    let (_, mut object) = object();
    let name = object["metadata"]["name"].as_str().unwrap().to_owned();
    let handle = RuntimeHandle(format!("agent-sandbox:{name}:sandbox-uid"));
    object["metadata"]["labels"][OWNER_LABEL] = json!("changed");
    let mut objects = list("SandboxList");
    objects["items"] = json!([object]);
    let fixture = Fixture::start(vec![
        (404, missing()),
        (404, missing()),
        (200, objects),
        (200, list("SandboxList")),
        (200, list("PodList")),
    ])
    .await;
    fixture.client.delete(&handle).await.unwrap();
    let calls = fixture.calls.lock().unwrap();
    assert_eq!(calls.len(), 5);
    for (call, _) in &calls[2..4] {
        assert!(!call.contains("labelSelector="));
        assert!(call.contains("fieldSelector=metadata.name%3D"));
    }
    assert!(calls[4].0.contains("/pods?"));
    assert!(calls.iter().all(|(line, _)| line.starts_with("GET ")));
}

#[tokio::test]
async fn foreign_home_claim_is_refused_before_creating_sandbox() {
    let request = super::tests::request();
    let allocation = HomeVolumeRequest {
        scope: None,
        volume_id: "home-one".into(),
        sandbox_id: "someone-elses-sandbox".into(),
        policy: HomeVolumePolicy {
            mode: Default::default(),
            storage_class: "standard".into(),
            capacity_gib: 10,
            template_name: "seed".into(),
            template_generation: "one".into(),
        },
    };
    let claim = json!({"apiVersion":"v1","kind":"PersistentVolumeClaim","metadata":{"name":"cma-home-test","namespace":"cma-test","labels":{"cma/home-managed-by":"cma-test"},"annotations":{"cma/home-allocation":serde_json::to_string(&allocation).unwrap(),"cma/home-request-sha256":allocation.computed_sha256()}}});
    let fixture = Fixture::start(vec![(200, claim)]).await;
    assert!(fixture.client.create(&request).await.is_err());
    let calls = fixture.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert!(calls[0]
        .0
        .starts_with("GET /api/v1/namespaces/cma-test/persistentvolumeclaims/cma-home-test"));
}

#[tokio::test]
async fn stale_ready_condition_cannot_bind_a_pod() {
    let (_, mut object) = object();
    object["status"]["conditions"] =
        json!([{"type":"Ready","status":"True","observedGeneration":1}]);
    object["metadata"]["generation"] = json!(2);
    let handle = RuntimeHandle(format!(
        "agent-sandbox:{}:sandbox-uid",
        object["metadata"]["name"].as_str().unwrap()
    ));
    let fixture = pod_read_tests::Reads::new((200, object), (200, list("PodList")));
    assert!(fixture.client.pod_binding(&handle).await.is_err());
    assert_eq!(fixture.calls.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn pvc_mount_requires_the_exact_admitted_uid_even_with_identical_labels() {
    let mut request = super::tests::request();
    let allocation = HomeVolumeRequest {
        scope: None,
        volume_id: "home-one".into(),
        sandbox_id: "logical-sandbox".into(),
        policy: HomeVolumePolicy {
            mode: Default::default(),
            storage_class: "standard".into(),
            capacity_gib: 10,
            template_name: "seed".into(),
            template_generation: "one".into(),
        },
    };
    let attachment = crate::HomeVolumeAttachment {
        receipt: crate::HomeVolumeReceipt {
            native_authority: None,
            volume_id: allocation.volume_id.clone(),
            request_sha256: allocation.computed_sha256(),
            storage_reference: "cma-home-test".into(),
            storage_generation: "original-pvc-uid".into(),
        },
        request: allocation.clone(),
    };
    request
        .metadata
        .insert("cma/home-identity".into(), attachment.identity().unwrap());
    let claim = json!({"apiVersion":"v1","kind":"PersistentVolumeClaim","metadata":{
        "name":"cma-home-test","namespace":"cma-test","uid":"replacement-pvc-uid",
        "labels":{"cma/home-managed-by":"cma-test"},"annotations":{
            "cma/home-allocation":serde_json::to_string(&allocation).unwrap(),"cma/home-request-sha256":allocation.computed_sha256()}}});
    let fixture = Fixture::start(vec![(200, claim)]).await;
    assert!(fixture.client.create(&request).await.is_err());
    assert_eq!(fixture.calls.lock().unwrap().len(), 1);
}

#[path = "suspension_http_tests.rs"]
mod suspension;

#[path = "funding_http_tests.rs"]
mod funding_http_tests;
