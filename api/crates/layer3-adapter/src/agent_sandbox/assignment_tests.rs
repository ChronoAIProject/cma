use super::*;
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio_tungstenite::{
    accept_hdr_async,
    tungstenite::{
        handshake::server::{Request, Response},
        Message,
    },
};

#[derive(Clone, Copy)]
enum Mode {
    Success,
    Failure,
    MissingStatus,
    WrongReceipt,
    LargeStdout,
    LargeStderr,
    ReplacedPod,
    ChangedSpec,
}

struct Fixture {
    client: AgentSandboxClient,
    calls: Arc<AtomicUsize>,
    executions: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn binding() -> PodBinding {
    PodBinding {
        sandbox_name: "claimed".into(),
        sandbox_uid: "sandbox-uid".into(),
        pod_name: "claimed-pod".into(),
        pod_uid: "original-pod".into(),
        pod_ip: "10.0.0.1".parse().unwrap(),
        runtime_container_id: None,
        runtime_restart_count: None,
    }
}

fn object() -> Value {
    json!({"apiVersion":"agents.x-k8s.io/v1beta1", "kind":"Sandbox",
        "metadata":{"name":"claimed","namespace":"cma-test","uid":"sandbox-uid","resourceVersion":"7","generation":1,
            "labels":{"cma/control-plane":"cma-test"},
            "annotations":{"cma/sandbox":"logical-sandbox","cma/runtime-generation":"2","cma/home-identity":"a".repeat(43)}},
        "spec":{"operatingMode":"Running"},
        "status":{"conditions":[{"type":"Ready","status":"True","observedGeneration":1}]}})
}

fn pods(replaced: bool) -> Value {
    json!({"apiVersion":"v1","kind":"PodList","metadata":{"resourceVersion":"8"},"items":[{
        "metadata":{"name":"claimed-pod","namespace":"cma-test","uid":if replaced {"replacement-pod"} else {"original-pod"},
            "labels":{"cma/control-plane":"cma-test",super::super::pods::TRACKING_LABEL:super::super::pods::name_hash("claimed")},
            "ownerReferences":[{"apiVersion":"agents.x-k8s.io/v1beta1","kind":"Sandbox","name":"claimed","uid":"sandbox-uid","controller":true}]},
        "status":{"phase":"Running","podIP":"10.0.0.1","conditions":[{"type":"Ready","status":"True"}]}}]})
}

impl Fixture {
    async fn start(mode: Mode) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let executions = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        let executed = executions.clone();
        let task = tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut header = vec![0; 8192];
                let length = loop {
                    let count = socket.peek(&mut header).await.unwrap();
                    assert_ne!(count, 0);
                    if header[..count].windows(4).any(|w| w == b"\r\n\r\n") {
                        break count;
                    }
                    tokio::task::yield_now().await;
                };
                let head = String::from_utf8_lossy(&header[..length]);
                let line = head.lines().next().unwrap();
                let path = line.split_whitespace().nth(1).unwrap();
                seen.fetch_add(1, Ordering::SeqCst);
                if path.contains("/exec?") {
                    assert!(line.starts_with("GET "));
                    // tungstenite's handshake callback requires this response type.
                    #[allow(clippy::result_large_err)]
                    let mut ws =
                        accept_hdr_async(socket, |request: &Request, mut response: Response| {
                            let url = url::Url::parse(&format!("http://fixture{}", request.uri()))
                                .unwrap();
                            let parameters: Vec<_> = url
                                .query_pairs()
                                .map(|(k, v)| (k.into_owned(), v.into_owned()))
                                .collect();
                            let commands: Vec<_> = parameters
                                .iter()
                                .filter(|(k, _)| k == "command")
                                .map(|(_, v)| v.as_str())
                                .collect();
                            assert_eq!(
                                commands,
                                ["/usr/local/bin/layer4-runtime", "--assign-runtime"]
                            );
                            for (key, value) in [
                                ("container", "runtime"),
                                ("stdin", "true"),
                                ("stdout", "true"),
                                ("stderr", "true"),
                            ] {
                                assert!(parameters.iter().any(|(k, v)| k == key && v == value));
                            }
                            assert!(!parameters.iter().any(|(k, v)| k == "tty" && v == "true"));
                            assert!(!request.uri().to_string().contains("CHRONO_"));
                            response.headers_mut().insert(
                                "sec-websocket-protocol",
                                "v5.channel.k8s.io".parse().unwrap(),
                            );
                            Ok(response)
                        })
                        .await
                        .unwrap();
                    let mut input = Vec::new();
                    while let Some(message) = ws.next().await {
                        let Message::Binary(bytes) = message.unwrap() else {
                            continue;
                        };
                        if bytes.as_ref() == [255, 0] {
                            break;
                        }
                        assert_eq!(bytes[0], 0);
                        input.extend_from_slice(&bytes[1..]);
                    }
                    let request: RuntimeBootstrapRequest = serde_json::from_slice(&input).unwrap();
                    request.validate().unwrap();
                    executed.fetch_add(1, Ordering::SeqCst);
                    let mut receipt = RuntimeBootstrapReceipt::for_request(&request).unwrap();
                    if matches!(mode, Mode::WrongReceipt) {
                        receipt.pod_uid = "another-pod".into();
                    }
                    let mut stdout = vec![1];
                    stdout.extend(serde_json::to_vec(&receipt).unwrap());
                    if matches!(mode, Mode::LargeStdout) {
                        stdout = vec![b'x'; RECEIPT_LIMIT + 2];
                        stdout[0] = 1;
                    }
                    ws.send(Message::Binary(stdout)).await.unwrap();
                    let mut stderr = vec![2];
                    if matches!(mode, Mode::LargeStderr) {
                        stderr.extend(vec![b'x'; STDERR_LIMIT + 1]);
                    } else {
                        // Even a buggy CLI that echoes the payload must not leak it
                        // through AdapterError or caller logs.
                        stderr.extend(input);
                    }
                    ws.send(Message::Binary(stderr)).await.unwrap();
                    if matches!(mode, Mode::LargeStdout | Mode::LargeStderr) {
                        // No EOF/status: the sender must enforce the stream bound.
                        while ws.next().await.is_some_and(|m| m.is_ok()) {}
                    } else if !matches!(mode, Mode::MissingStatus) {
                        let mut status = vec![3];
                        status.extend(
                            serde_json::to_vec(&json!({"apiVersion":"v1","kind":"Status",
                            "status": if matches!(mode, Mode::Failure) {"Failure"} else {"Success"},
                            "message": request.environment["CHRONO_RUNTIME_CONTROL_TOKEN"]}))
                            .unwrap(),
                        );
                        let _ = ws.send(Message::Binary(status)).await;
                    }
                    let _ = ws.close(None).await;
                    continue;
                }
                assert!(line.starts_with("GET "));
                let after = executed.load(Ordering::SeqCst) > 0;
                let value = if path.starts_with("/api/v1/namespaces/cma-test/pods?") {
                    pods(after && matches!(mode, Mode::ReplacedPod))
                } else {
                    assert_eq!(
                        path,
                        "/apis/agents.x-k8s.io/v1beta1/namespaces/cma-test/sandboxes/claimed"
                    );
                    let mut value = object();
                    if after && matches!(mode, Mode::ChangedSpec) {
                        value["spec"]["service"] = json!(true);
                    }
                    value
                };
                let mut consumed = vec![0; length];
                socket.read_exact(&mut consumed).await.unwrap();
                let body = value.to_string();
                socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
            }
        });
        let client = AgentSandboxClient::new(
            kube::Client::try_from(kube::Config::new(
                format!("http://{address}").parse().unwrap(),
            ))
            .unwrap(),
            super::super::tests::config(),
            "fixture",
        )
        .unwrap();
        Self {
            client,
            calls,
            executions,
            task,
        }
    }

    fn request(&self) -> RuntimeBootstrapRequest {
        RuntimeBootstrapRequest {
            version: crate::runtime_bootstrap::RUNTIME_BOOTSTRAP_VERSION,
            staging_startup_failure: None,
            pod_uid: "original-pod".into(),
            environment: BTreeMap::from([
                ("CHRONO_SANDBOX_ID".into(), "logical-sandbox".into()),
                ("CHRONO_RUNTIME_GENERATION".into(), "2".into()),
                ("CHRONO_RUNTIME_HOME_IDENTITY".into(), "a".repeat(43)),
                (
                    "CHRONO_RUNTIME_CONTROL_TOKEN".into(),
                    self.client.control_token("claimed"),
                ),
            ]),
        }
    }

    async fn assign(&self, request: &RuntimeBootstrapRequest) -> Result<RuntimeBootstrapReceipt> {
        tokio::time::timeout(
            Duration::from_secs(3),
            self.client.assign_runtime(
                &RuntimeHandle("agent-sandbox:claimed:sandbox-uid".into()),
                &binding(),
                request,
            ),
        )
        .await
        .expect("assignment must finish within test deadline")
    }
}

#[tokio::test]
async fn exec_uses_only_stdin_and_accepts_exact_successful_receipt_after_identity_recheck() {
    let fixture = Fixture::start(Mode::Success).await;
    let request = fixture.request();
    for _ in 0..2 {
        fixture
            .assign(&request)
            .await
            .unwrap()
            .validate_for(&request)
            .unwrap();
    }
    assert_eq!(fixture.executions.load(Ordering::SeqCst), 2);
    assert_eq!(fixture.calls.load(Ordering::SeqCst), 14);
}

#[tokio::test]
async fn exec_refuses_failed_missing_oversized_or_mismatched_receipts_and_identity_races() {
    for mode in [
        Mode::Failure,
        Mode::MissingStatus,
        Mode::WrongReceipt,
        Mode::LargeStdout,
        Mode::LargeStderr,
        Mode::ReplacedPod,
        Mode::ChangedSpec,
    ] {
        let fixture = Fixture::start(mode).await;
        let request = fixture.request();
        let error = fixture.assign(&request).await.unwrap_err();
        assert!(!error
            .to_string()
            .contains(&request.environment["CHRONO_RUNTIME_CONTROL_TOKEN"]));
        assert_eq!(fixture.executions.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn exec_requires_matching_local_and_kubernetes_authority_before_sending_credentials() {
    let fixture = Fixture::start(Mode::Success).await;
    let original = fixture.request();
    for field in [
        "pod_uid",
        "CHRONO_RUNTIME_CONTROL_TOKEN",
        "CHRONO_SANDBOX_ID",
        "CHRONO_RUNTIME_GENERATION",
        "CHRONO_RUNTIME_HOME_IDENTITY",
    ] {
        let mut request = original.clone();
        if field == "pod_uid" {
            request.pod_uid = "another-pod".into();
        } else {
            request
                .environment
                .get_mut(field)
                .unwrap()
                .replace_range(..1, "3");
        }
        assert!(fixture.assign(&request).await.is_err());
        assert_eq!(fixture.executions.load(Ordering::SeqCst), 0);
    }
}
