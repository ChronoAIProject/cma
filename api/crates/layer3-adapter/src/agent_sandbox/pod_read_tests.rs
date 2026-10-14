use super::*;
use std::time::Duration;
use tokio::sync::Semaphore;

struct Gate {
    started: Semaphore,
    release: Semaphore,
}

impl Gate {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            started: Semaphore::new(0),
            release: Semaphore::new(0),
        })
    }

    async fn wait(&self) {
        tokio::time::timeout(Duration::from_secs(2), self.started.acquire())
            .await
            .expect("independent read did not start")
            .unwrap()
            .forget();
    }

    fn respond(&self) {
        self.release.add_permits(1);
    }
}

pub(super) struct Reads {
    pub(super) client: AgentSandboxClient,
    pub(super) calls: Arc<Mutex<Vec<http::Uri>>>,
}

impl Reads {
    pub(super) fn new(sandbox: (u16, Value), pods: (u16, Value)) -> Self {
        Self::gated(sandbox, pods, None, None)
    }

    fn gated(
        sandbox: (u16, Value),
        pods: (u16, Value),
        sandbox_gate: Option<Arc<Gate>>,
        pod_gate: Option<Arc<Gate>>,
    ) -> Self {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let observed = calls.clone();
        let service = tower::service_fn(move |request: http::Request<kube::client::Body>| {
            assert_eq!(request.method(), http::Method::GET);
            let is_pods = request.uri().path().ends_with("/pods");
            assert!(is_pods || request.uri().path().contains("/sandboxes/"));
            observed.lock().unwrap().push(request.uri().clone());
            let (status, body) = if is_pods {
                pods.clone()
            } else {
                sandbox.clone()
            };
            let gate = if is_pods {
                pod_gate.clone()
            } else {
                sandbox_gate.clone()
            };
            async move {
                if let Some(gate) = gate {
                    gate.started.add_permits(1);
                    gate.release.acquire().await.unwrap().forget();
                }
                Ok::<_, std::convert::Infallible>(
                    http::Response::builder()
                        .status(status)
                        .header(http::header::CONTENT_TYPE, "application/json")
                        .body(kube::client::Body::from(serde_json::to_vec(&body).unwrap()))
                        .unwrap(),
                )
            }
        });
        let client = AgentSandboxClient::new(
            kube::Client::new(service, "cma-test"),
            super::super::tests::config(),
            "fixture",
        )
        .unwrap();
        Self { client, calls }
    }
}

fn ready() -> (RuntimeHandle, Value, Value) {
    let (_, mut object) = object();
    object["status"]["conditions"] =
        json!([{"type":"Ready","status":"True","observedGeneration":1}]);
    let name = object["metadata"]["name"].as_str().unwrap();
    let handle = RuntimeHandle(format!("agent-sandbox:{name}:sandbox-uid"));
    let mut candidates = list("PodList");
    candidates["items"] = json!([{"apiVersion":"v1","kind":"Pod","metadata":{
        "name":"runtime-pod","namespace":"cma-test","uid":"pod-uid",
        "labels":{OWNER_LABEL:"cma-test",pods::TRACKING_LABEL:pods::name_hash(name)},
        "ownerReferences":[{"apiVersion":"agents.x-k8s.io/v1beta1","kind":"Sandbox","name":name,"uid":"sandbox-uid","controller":true}]},
        "status":{"phase":"Running","podIP":"10.0.0.7","conditions":[{"type":"Ready","status":"True"}],
        "containerStatuses":[{"name":"runtime","image":"fixture","imageID":"fixture","containerID":"containerd://original",
            "ready":true,"restartCount":2,"state":{"running":{}}}]}}]);
    (handle, object, candidates)
}

fn refusal(code: u16) -> (u16, Value) {
    (
        code,
        json!({"apiVersion":"v1","kind":"Status","status":"Failure","reason":"Fixture","message":"fixture failure","code":code}),
    )
}

#[tokio::test]
async fn pod_binding_starts_both_reads_and_retains_exact_container_incarnation() {
    let (handle, object, pods) = ready();
    let sandbox_gate = Gate::new();
    let pod_gate = Gate::new();
    let reads = Reads::gated(
        (200, object),
        (200, pods),
        Some(sandbox_gate.clone()),
        Some(pod_gate.clone()),
    );
    let client = reads.client.clone();
    let task = tokio::spawn(async move { client.pod_binding(&handle).await });
    sandbox_gate.wait().await;
    pod_gate.wait().await;
    assert!(!task.is_finished());
    sandbox_gate.respond();
    pod_gate.respond();
    let binding = tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(binding.sandbox_uid, "sandbox-uid");
    assert_eq!(binding.pod_uid, "pod-uid");
    assert_eq!(
        binding.runtime_container_id.as_deref(),
        Some("containerd://original")
    );
    assert_eq!(binding.runtime_restart_count, Some(2));
    assert_eq!(reads.calls.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn pod_binding_consumes_sandbox_authority_before_faster_pod_failure() {
    for change in [
        "absent",
        "deleting",
        "not_ready",
        "stale",
        "uid",
        "owner",
        "api",
    ] {
        let (handle, mut object, _) = ready();
        match change {
            "deleting" => object["metadata"]["deletionTimestamp"] = json!("2026-10-14T00:00:00Z"),
            "not_ready" => object["status"]["conditions"][0]["status"] = json!("False"),
            "stale" => object["metadata"]["generation"] = json!(2),
            "uid" => object["metadata"]["uid"] = json!("replacement"),
            "owner" => object["metadata"]["labels"][OWNER_LABEL] = json!("foreign"),
            _ => (),
        }
        let reply = match change {
            "absent" => (404, missing()),
            "api" => refusal(429),
            _ => (200, object),
        };
        let gate = Gate::new();
        let pod_gate = Gate::new();
        let reads = Reads::gated(
            reply,
            refusal(403),
            Some(gate.clone()),
            Some(pod_gate.clone()),
        );
        let client = reads.client.clone();
        let mut task = tokio::spawn(async move { client.pod_binding(&handle).await });
        gate.wait().await;
        pod_gate.wait().await;
        pod_gate.respond();
        assert!(tokio::time::timeout(Duration::from_millis(25), &mut task)
            .await
            .is_err());
        gate.respond();
        let error = tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        match change {
            "absent" => assert!(
                matches!(error, AdapterError::Unavailable(detail) if detail == "Sandbox no longer exists")
            ),
            "deleting" | "not_ready" | "stale" => assert!(
                matches!(error, AdapterError::Unavailable(detail) if detail == "Sandbox is not ready")
            ),
            "uid" => assert!(
                matches!(error, AdapterError::Rejected(detail) if detail == "Sandbox name now belongs to a different UID")
            ),
            "owner" => assert!(
                matches!(error, AdapterError::Rejected(detail) if detail == "Sandbox is not owned by this CMA deployment")
            ),
            _ => assert!(matches!(error, AdapterError::AtCapacity(_))),
        }
        assert_eq!(reads.calls.lock().unwrap().len(), 2);
    }
}

#[tokio::test]
async fn pod_binding_returns_pod_error_only_after_valid_sandbox_and_never_reads_invalid_handle() {
    let (handle, object, _) = ready();
    let reads = Reads::new((200, object), refusal(403));
    assert!(
        matches!(reads.client.pod_binding(&handle).await, Err(AdapterError::Rejected(detail))
        if detail == "Kubernetes rejected Sandbox operation (403)")
    );
    reads.calls.lock().unwrap().clear();
    assert!(reads
        .client
        .pod_binding(&RuntimeHandle("invalid".into()))
        .await
        .is_err());
    assert!(reads.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn pod_binding_retains_returned_name_discovery_for_a_misaddressed_response() {
    let (_, object, pods) = ready();
    let name = object["metadata"]["name"].as_str().unwrap().to_owned();
    let reads = Reads::new((200, object), (200, pods));
    let binding = reads
        .client
        .pod_binding(&RuntimeHandle(
            "agent-sandbox:different-name:sandbox-uid".into(),
        ))
        .await
        .unwrap();
    assert_eq!(binding.sandbox_name, name);
    let calls = reads.calls.lock().unwrap();
    assert_eq!(calls.len(), 3);
    let selectors = calls
        .iter()
        .filter(|uri| uri.path().ends_with("/pods"))
        .map(|uri| {
            reqwest::Url::parse(&format!("http://fixture{uri}"))
                .unwrap()
                .query_pairs()
                .find(|(key, _)| key == "labelSelector")
                .unwrap()
                .1
                .into_owned()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        selectors,
        [pods::selector("different-name"), pods::selector(&name)]
    );
}
