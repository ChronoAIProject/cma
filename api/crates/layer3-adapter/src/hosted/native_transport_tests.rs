//! A local Kubernetes WebSocket fixture carries real HTTP and nested WebSocket bytes.
use super::*;
use crate::agent_sandbox::{AgentSandboxClient, AgentSandboxConfig};
use crate::ResourceReading;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Mutex as StdMutex,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::{
    accept_hdr_async,
    tungstenite::{
        handshake::server::{Request, Response},
        Message,
    },
};

const API_TOKEN: &str = "fixture-api-credential";
const ERROR_SECRET: &str = "fixture-sensitive-error-body";

#[derive(Clone, Copy)]
enum Mode {
    Success,
    Persistent,
    Forward(std::net::SocketAddr),
    ReplaceOnOpen,
    Forbidden,
    Throttled,
    ErrorChannel,
    SilentClose,
    LostResponse,
    Stall,
    StallAfterOpen,
    LargeBody,
    HttpError,
    WireError,
    StallBeforeReady,
    InvalidReady,
    BufferedExit,
    BufferedHistoryExit,
}

#[derive(Default)]
struct State {
    replaced: AtomicBool,
    wrong_owner: AtomicBool,
    restarted: AtomicBool,
    missing_incarnation: AtomicBool,
    hold_response: AtomicBool,
    retained_evidence: AtomicBool,
    startup_policy: StdMutex<Option<bool>>,
    hold_next_pod_reply: AtomicBool,
    pod_reply_entered: tokio::sync::Notify,
    pod_reply_release: tokio::sync::Notify,
    hold_ownership_response: AtomicBool,
    ownership_confirmations: AtomicUsize,
    hold_next_upgrade: AtomicBool,
    preparation_entered: tokio::sync::Notify,
    preparation_release: tokio::sync::Notify,
    runtime_bytes: AtomicUsize,
    reject_next_upgrade: AtomicBool,
    fault: AtomicUsize,
    pod_reads: AtomicUsize,
    opened: AtomicUsize,
    active: AtomicUsize,
    requests: StdMutex<Vec<String>>,
    runtimes: StdMutex<BTreeMap<String, String>>,
}
struct Active(Arc<State>);
impl Drop for Active {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::SeqCst);
    }
}
struct Fixture {
    transport: Arc<ProviderHostedControlTransport>,
    state: Arc<State>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn handle() -> RuntimeHandle {
    RuntimeHandle::new("agent-sandbox:claimed:sandbox-uid")
}
fn object(name: &str) -> Value {
    json!({"apiVersion":"agents.x-k8s.io/v1beta1","kind":"Sandbox",
        "metadata":{"name":name,"namespace":"fixture","uid":sandbox_uid(name),"resourceVersion":"7","generation":1,
            "labels":{"cma/control-plane":"fixture"}},
        "status":{"conditions":[{"type":"Ready","status":"True","observedGeneration":1}]}})
}
fn sandbox_uid(name: &str) -> String {
    if name == "claimed" {
        "sandbox-uid".into()
    } else {
        format!("{name}-uid")
    }
}
fn name_hash(name: &str) -> String {
    let hash = name.bytes().fold(2_166_136_261u32, |hash, byte| {
        (hash ^ u32::from(byte)).wrapping_mul(16_777_619)
    });
    format!("{hash:08x}")
}
fn pods(state: &State, name: &str) -> Value {
    let mut pods = json!({"apiVersion":"v1","kind":"PodList","metadata":{"resourceVersion":"8"},"items":[{
        "metadata":{"name":format!("{name}-pod"),"namespace":"fixture","uid":if state.replaced.load(Ordering::SeqCst) {"replacement-pod"} else {"original-pod"},
            "labels":{"cma/control-plane":"fixture","agents.x-k8s.io/sandbox-name-hash":name_hash(name)},
            "ownerReferences":[{"apiVersion":"agents.x-k8s.io/v1beta1","kind":"Sandbox","name":name,
                "uid":if state.wrong_owner.load(Ordering::SeqCst) {"foreign-owner".to_string()} else {sandbox_uid(name)},"controller":true}]},
        "status":{"phase":"Running","podIP":"10.0.0.1","conditions":[{"type":"Ready","status":"True"}],
        "containerStatuses":[{"name":"runtime","image":"runtime","imageID":"sha256:runtime","ready":true,
            "restartCount":if state.restarted.load(Ordering::SeqCst) {1} else {0},
            "containerID":if state.missing_incarnation.load(Ordering::SeqCst) {""} else if state.restarted.load(Ordering::SeqCst) {"containerd://replacement"} else {"containerd://original"},
            "state":{"running":{}}}]}}]});
    if state.retained_evidence.load(Ordering::SeqCst) {
        let pod = &mut pods["items"][0];
        pod["spec"] = json!({"containers":[{"name":"runtime"}]});
        let container = &mut pod["status"]["containerStatuses"][0];
        container["imageID"] = json!(format!("containerd://sha256:{}", "c".repeat(64)));
        container["state"]["running"]["startedAt"] = json!("2026-09-05T00:00:00Z");
    }
    pods
}

impl Fixture {
    async fn start(mode: Mode, transport_mode: NativeRuntimeTransport) -> Self {
        Self::with_capacity(mode, transport_mode, Default::default()).await
    }
    async fn with_capacity(
        mode: Mode,
        transport_mode: NativeRuntimeTransport,
        capacity: crate::agent_sandbox::RuntimeCapacityConfig,
    ) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let mut configuration = kube::Config::new(format!("http://{address}").parse().unwrap());
        // Classify an individual upgrade response deterministically. Production
        // retains the shared kube client's retry policy within the open deadline.
        configuration.default_retry = false;
        configuration.auth_info.token = Some(API_TOKEN.into());
        let client = AgentSandboxClient::new_with_capacity(
            kube::Client::try_from(configuration).unwrap(),
            AgentSandboxConfig {
                namespace: "fixture".into(),
                deployment: "fixture".into(),
                runtime_class: None,
                service_account: "default".into(),
                image_pull_secrets: vec![],
                template_network_policy: None,
                template_fs_group_change_policy: None,
                native_claim_guard_enabled: false,
                runtime_startup_breaker_enabled: false,
                control_key: "fixture".repeat(8),
            },
            "fixture",
            capacity,
        )
        .unwrap();
        let token = client.control_token("claimed");
        let transport = ProviderHostedControlTransport::for_agent_sandbox_with_transport(
            client,
            transport_mode,
        )
        .unwrap();
        let state = Arc::new(State::default());
        state
            .runtimes
            .lock()
            .unwrap()
            .insert("claimed".into(), token.clone());
        let served = state.clone();
        let task = tokio::spawn(async move {
            let mut tasks = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let (socket, _) = accepted.unwrap();
                        let (state, token) = (served.clone(), token.clone());
                        tasks.spawn(async move { api_request(socket, mode, transport_mode, state, token).await; });
                    }
                    result = tasks.join_next(), if !tasks.is_empty() => { result.unwrap().unwrap(); }
                }
            }
        });
        Self {
            transport: Arc::new(transport),
            state,
            task,
        }
    }
    async fn endpoint(&self) -> RuntimeEndpoint {
        self.transport.endpoint(&handle()).await.unwrap()
    }
    async fn drained(&self) {
        tokio::time::timeout(Duration::from_secs(2), async {
            while self.state.active.load(Ordering::SeqCst) != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("forwarding task closed after drop/cancellation");
        assert!(!self.task.is_finished(), "fixture task failed");
    }
}

async fn api_request(
    mut socket: tokio::net::TcpStream,
    mode: Mode,
    transport_mode: NativeRuntimeTransport,
    state: Arc<State>,
    token: String,
) {
    let mut header = vec![0; 64 * 1024];
    let count = loop {
        let count = socket.peek(&mut header).await.unwrap();
        if count == 0 {
            return;
        }
        if header[..count].windows(4).any(|w| w == b"\r\n\r\n") {
            break count;
        }
        assert!(count < header.len());
        tokio::task::yield_now().await;
    };
    let text = String::from_utf8_lossy(&header[..count]);
    let path = text
        .lines()
        .next()
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap();
    assert!(text
        .to_ascii_lowercase()
        .contains(&format!("authorization: bearer {API_TOKEN}")));
    assert!(
        !text.contains(&token),
        "runtime token cannot enter Kubernetes request headers or URLs"
    );
    if path.contains("/portforward?") || path.contains("/exec?") {
        let is_exec = transport_mode == NativeRuntimeTransport::KubernetesExec;
        let url = url::Url::parse(&format!("http://fixture{path}")).unwrap();
        let name = url
            .path()
            .strip_prefix("/api/v1/namespaces/fixture/pods/")
            .unwrap()
            .split('/')
            .next()
            .unwrap()
            .strip_suffix("-pod")
            .unwrap();
        let token = state.runtimes.lock().unwrap()[name].clone();
        assert!(!text.contains(&token));
        if is_exec {
            assert_eq!(
                url.query_pairs()
                    .filter(|(k, _)| k == "command")
                    .map(|(_, v)| v.into_owned())
                    .collect::<Vec<_>>(),
                [
                    "/usr/bin/python3",
                    "-I",
                    "-u",
                    "-c",
                    crate::runtime_control_relay::SCRIPT
                ]
            );
            let fields: std::collections::BTreeMap<_, _> = url.query_pairs().collect();
            assert_eq!(fields.get("container").map(|v| v.as_ref()), Some("runtime"));
            assert_eq!(fields.get("stdin").map(|v| v.as_ref()), Some("true"));
            assert_eq!(fields.get("stdout").map(|v| v.as_ref()), Some("true"));
            assert!(fields.get("stderr").is_none_or(|v| v == "false"));
            assert!(fields.get("tty").is_none_or(|v| v == "false"));
        } else {
            assert!(path.starts_with("/api/v1/namespaces/fixture/pods/claimed-pod/portforward?"));
            assert_eq!(
                url.query_pairs()
                    .filter(|(key, _)| key == "ports")
                    .map(|(_, value)| value.into_owned())
                    .collect::<Vec<_>>(),
                ["9090"]
            );
        }
        state.opened.fetch_add(1, Ordering::SeqCst);
        let preparing = desktop_tests::pause_preparation(&state).await;
        if matches!(mode, Mode::Forbidden | Mode::Throttled)
            || state.reject_next_upgrade.swap(false, Ordering::SeqCst)
        {
            let status = if matches!(mode, Mode::Throttled) {
                429
            } else {
                403
            };
            let body = json!({"apiVersion":"v1","kind":"Status","status":"Failure","reason":"Forbidden","message":ERROR_SECRET,"code":status}).to_string();
            let _ = socket.write_all(format!("HTTP/1.1 {status} Rejected\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await;
            return;
        }
        if matches!(mode, Mode::ReplaceOnOpen) {
            state.replaced.store(true, Ordering::SeqCst);
        }
        #[allow(clippy::result_large_err)]
        let ws = accept_hdr_async(socket, |_request: &Request, mut response: Response| {
            response.headers_mut().insert(
                "sec-websocket-protocol",
                "v5.channel.k8s.io".parse().unwrap(),
            );
            Ok(response)
        })
        .await;
        let Ok(ws) = ws else {
            assert!(preparing.is_some(), "unexpected upgrade failure");
            return;
        };
        state.active.fetch_add(1, Ordering::SeqCst);
        let _active = Active(state.clone());
        serve_forwarder(ws, mode, is_exec, state, token).await;
    } else {
        if matches!(mode, Mode::StallAfterOpen) && state.opened.load(Ordering::SeqCst) > 0 {
            std::future::pending::<()>().await;
        }
        let body = if let Some(name) =
            path.strip_prefix("/apis/agents.x-k8s.io/v1beta1/namespaces/fixture/sandboxes/")
        {
            let mut object = object(name);
            if state.retained_evidence.load(Ordering::SeqCst) {
                object["metadata"]["annotations"] = json!({"cma/runtime-generation":"1"});
            }
            object
        } else {
            assert!(path.starts_with("/api/v1/namespaces/fixture/pods?"));
            state.pod_reads.fetch_add(1, Ordering::SeqCst);
            if state.hold_next_pod_reply.swap(false, Ordering::SeqCst) {
                state.pod_reply_entered.notify_one();
                state.pod_reply_release.notified().await;
            }
            let url = url::Url::parse(&format!("http://fixture{path}")).unwrap();
            let selector = url
                .query_pairs()
                .find(|(key, _)| key == "labelSelector")
                .unwrap()
                .1
                .into_owned();
            let name = state
                .runtimes
                .lock()
                .unwrap()
                .keys()
                .find(|name| selector.contains(&name_hash(name)))
                .unwrap()
                .clone();
            pods(&state, &name)
        }
        .to_string();
        let _ = socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await;
    }
}

async fn serve_forwarder(
    mut ws: tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
    mode: Mode,
    is_exec: bool,
    state: Arc<State>,
    token: String,
) {
    if is_exec {
        if matches!(mode, Mode::StallBeforeReady) {
            while ws.next().await.is_some_and(|message| message.is_ok()) {}
            return;
        }
        let mut marker = vec![1];
        marker.extend_from_slice(if matches!(mode, Mode::InvalidReady) {
            b"BAD-EXEC-1\n"
        } else {
            b"CMA-EXEC-1\n"
        });
        if ws.send(Message::Binary(marker)).await.is_err() {
            return;
        }
        if matches!(mode, Mode::BufferedExit) {
            let _ = ws
                .send(Message::Binary(b"\x01buffered-response".to_vec()))
                .await;
            let _ = ws
                .send(Message::Binary(b"\x03{\"status\":\"Success\"}".to_vec()))
                .await;
            return;
        }
    } else {
        for channel in [0, 1] {
            let mut bytes = vec![channel];
            bytes.extend(9090u16.to_le_bytes());
            if ws.send(Message::Binary(bytes)).await.is_err() {
                return;
            }
        }
    }
    if matches!(mode, Mode::SilentClose) {
        let _ = ws.close(None).await;
        return;
    }
    if matches!(mode, Mode::ErrorChannel) {
        let mut error = vec![if is_exec { 3 } else { 1 }];
        let detail = if is_exec {
            json!({"status":"Failure","message":ERROR_SECRET}).to_string()
        } else {
            ERROR_SECRET.into()
        };
        error.extend_from_slice(detail.as_bytes());
        let _ = ws.send(Message::Binary(error)).await;
        while ws.next().await.is_some_and(|value| value.is_ok()) {}
        return;
    }
    if matches!(mode, Mode::BufferedHistoryExit) {
        let mut request = Vec::new();
        while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
            let Some(Ok(Message::Binary(bytes))) = ws.next().await else {
                panic!("history request was not delivered");
            };
            assert_eq!(bytes[0], 0);
            request.extend_from_slice(&bytes[1..]);
        }
        let request = String::from_utf8(request).unwrap();
        assert!(request.starts_with("GET /runtime/agents/agent-a/history "));
        state.requests.lock().unwrap().push(request);
        let body = "{\"history\":\"".to_owned() + &"x".repeat(48 * 1024) + "\"}\n";
        let mut response = vec![1];
        response.extend_from_slice(format!("HTTP/1.1 200 OK\r\nContent-Type: application/x-ndjson\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes());
        // Flush the complete finite response and successful exec exit together.
        // The HTTP consumer must drain stdout even if the exit is already ready.
        ws.feed(Message::Binary(response)).await.unwrap();
        ws.feed(Message::Binary(b"\x03{\"status\":\"Success\"}".to_vec()))
            .await
            .unwrap();
        ws.flush().await.unwrap();
        return;
    }
    let (runtime, forward) = tokio::io::duplex(64 * 1024);
    let (mut source, mut sink) = tokio::io::split(forward);
    let bridge = async {
        let mut buffer = vec![0; 8192];
        loop {
            tokio::select! {
                incoming = ws.next() => {
                    let bytes = match incoming {
                        Some(Ok(Message::Binary(bytes))) => bytes,
                        Some(Ok(Message::Ping(bytes))) => { if ws.send(Message::Pong(bytes)).await.is_err() { return; } continue; },
                        _ => return,
                    };
                    if bytes[0] == 255 { return; }
                    assert_eq!(bytes[0], 0);
                    state.runtime_bytes.fetch_add(bytes.len() - 1, Ordering::SeqCst);
                    if sink.write_all(&bytes[1..]).await.is_err() { return; }
                }
                read = source.read(&mut buffer) => {
                    let Ok(count) = read else { return; };
                    if count == 0 {
                        if is_exec { let _ = ws.send(Message::Binary(b"\x03{\"status\":\"Success\"}".to_vec())).await; }
                        else { let _ = ws.close(None).await; }
                        return;
                    }
                    let mut bytes = vec![u8::from(is_exec)]; bytes.extend_from_slice(&buffer[..count]);
                    if ws.send(Message::Binary(bytes)).await.is_err() { return; }
                }
            }
        }
    };
    // Runtime completion still lets the bridge drain its final response bytes.
    let runtime = async {
        if let Mode::Forward(address) = mode {
            let mut upstream = tokio::net::TcpStream::connect(address).await.unwrap();
            upstream.set_nodelay(true).unwrap();
            let mut runtime = runtime;
            match tokio::io::copy_bidirectional(&mut runtime, &mut upstream).await {
                Ok(_) => (),
                Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => (),
                Err(error) => panic!("runtime tunnel relay failed: {error}"),
            }
        } else {
            serve_runtime(runtime, mode, state.clone(), token).await;
        }
    };
    tokio::pin!(bridge, runtime);
    tokio::select! {
        _ = &mut bridge => {},
        _ = &mut runtime => { bridge.await; }
    }
}

#[path = "native_transport_server_fixture.rs"]
mod runtime_fixture;
use runtime_fixture::serve_runtime;

#[path = "native_open_overlap_tests.rs"]
mod open_overlap_tests;
#[path = "native_http_reuse_tests.rs"]
mod reuse_tests;

#[path = "native_ownership_contention_tests.rs"]
mod ownership_contention_tests;

#[path = "native_desktop_tests.rs"]
mod desktop_tests;

#[path = "native_transport_cases.rs"]
mod cases;

#[path = "native_startup_policy_tests.rs"]
mod startup_policy;

#[path = "native_capacity_tests.rs"]
mod capacity_tests;
