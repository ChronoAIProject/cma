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
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
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
    json!({"apiVersion":"v1","kind":"PodList","metadata":{"resourceVersion":"8"},"items":[{
        "metadata":{"name":format!("{name}-pod"),"namespace":"fixture","uid":if state.replaced.load(Ordering::SeqCst) {"replacement-pod"} else {"original-pod"},
            "labels":{"cma/control-plane":"fixture","agents.x-k8s.io/sandbox-name-hash":name_hash(name)},
            "ownerReferences":[{"apiVersion":"agents.x-k8s.io/v1beta1","kind":"Sandbox","name":name,
                "uid":if state.wrong_owner.load(Ordering::SeqCst) {"foreign-owner".to_string()} else {sandbox_uid(name)},"controller":true}]},
        "status":{"phase":"Running","podIP":"10.0.0.1","conditions":[{"type":"Ready","status":"True"}],
        "containerStatuses":[{"name":"runtime","image":"runtime","imageID":"sha256:runtime","ready":true,
            "restartCount":if state.restarted.load(Ordering::SeqCst) {1} else {0},
            "containerID":if state.missing_incarnation.load(Ordering::SeqCst) {""} else if state.restarted.load(Ordering::SeqCst) {"containerd://replacement"} else {"containerd://original"},
            "state":{"running":{}}}]}}]})
}

impl Fixture {
    async fn start(mode: Mode, transport_mode: NativeRuntimeTransport) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let mut configuration = kube::Config::new(format!("http://{address}").parse().unwrap());
        // Classify an individual upgrade response deterministically. Production
        // retains the shared kube client's retry policy within the open deadline.
        configuration.default_retry = false;
        configuration.auth_info.token = Some(API_TOKEN.into());
        let client = AgentSandboxClient::new(
            kube::Client::try_from(configuration).unwrap(),
            AgentSandboxConfig {
                namespace: "fixture".into(),
                deployment: "fixture".into(),
                runtime_class: None,
                service_account: "default".into(),
                image_pull_secrets: vec![],
                template_network_policy: None,
                template_fs_group_change_policy: None,
                control_key: "fixture".repeat(8),
            },
            "fixture",
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
        .await
        .unwrap();
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
            object(name)
        } else {
            assert!(path.starts_with("/api/v1/namespaces/fixture/pods?"));
            state.pod_reads.fetch_add(1, Ordering::SeqCst);
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
    let runtime = serve_runtime(runtime, mode, state, token);
    tokio::pin!(bridge, runtime);
    tokio::select! {
        _ = &mut bridge => {},
        _ = &mut runtime => { bridge.await; }
    }
}

async fn read_request(stream: &mut (impl AsyncRead + Unpin)) -> Option<String> {
    let mut request = Vec::new();
    let mut buffer = [0; 4096];
    loop {
        let count = stream.read(&mut buffer).await.ok()?;
        if count == 0 {
            return None;
        }
        request.extend_from_slice(&buffer[..count]);
        assert!(request.len() < 64 * 1024);
        if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&request[..end]);
            let length = head
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length: ")
                        .map(str::to_owned)
                })
                .map(|s| s.parse::<usize>().unwrap())
                .unwrap_or(0);
            if request.len() >= end + 4 + length {
                return String::from_utf8(request).ok();
            }
        }
    }
}

async fn serve_runtime(
    mut stream: tokio::io::DuplexStream,
    mode: Mode,
    state: Arc<State>,
    token: String,
) {
    loop {
        let Some(request) = read_request(&mut stream).await else {
            return;
        };
        assert!(request
            .to_ascii_lowercase()
            .contains(&format!("x-cma-runtime-token: {token}")));
        assert!(!request.to_ascii_lowercase().contains("authorization:"));
        assert!(!request.contains(API_TOKEN));
        state.requests.lock().unwrap().push(request.clone());
        while state.hold_response.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        match state.fault.swap(0, Ordering::SeqCst) {
            1 => return,
            2 => {
                let _ = stream
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n{}")
                    .await;
                return;
            }
            3 => {
                let _ = stream
                    .write_all(b"HTTP/1.1 500 Error\r\nContent-Length: 100\r\n\r\nprivate")
                    .await;
                return;
            }
            4 => {
                let _ = stream
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nbad")
                    .await;
                return;
            }
            5 => {
                let _ = stream
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 999999999\r\n\r\n")
                    .await;
                std::future::pending::<()>().await;
            }
            6 => {
                let body =
                    serde_json::to_string(&ResponseFrame::Failure(crate::wire::WireFailure {
                        operation: crate::wire::WireOperation::Install,
                        code: "mismatched".into(),
                        detail: "wrong operation".into(),
                        retryable: false,
                    }))
                    .unwrap();
                let _ = stream
                    .write_all(
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}",
                            body.len()
                        )
                        .as_bytes(),
                    )
                    .await;
                continue;
            }
            7 => {
                let mut response: ResponseFrame =
                    serde_json::from_str(&persistent_response(&request)).unwrap();
                let ResponseFrame::OpenAgent(ref mut receipt) = response else {
                    panic!("invalid Open receipt fixture requires an actual Open");
                };
                receipt.authority_fence += 1;
                let body = serde_json::to_string(&response).unwrap();
                let _ = stream
                    .write_all(
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}",
                            body.len()
                        )
                        .as_bytes(),
                    )
                    .await;
                continue;
            }
            _ => {}
        }
        match mode {
            Mode::LostResponse => return,
            Mode::Stall => {
                std::future::pending::<()>().await;
                return;
            }
            Mode::HttpError => {
                let body = format!("{ERROR_SECRET} {token}");
                let _ = stream
                    .write_all(
                        format!(
                        "HTTP/1.1 500 Internal Server Error\r\nContent-Length: {}\r\n\r\n{body}",
                        body.len()
                    )
                        .as_bytes(),
                    )
                    .await;
                return;
            }
            Mode::LargeBody => {
                let _ = stream
                    .write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n40\r\n")
                    .await;
                let _ = stream.write_all(&[b'x'; 64]).await;
                let _ = stream.write_all(b"\r\n").await;
                std::future::pending::<()>().await;
            }
            _ => {}
        }
        if request.to_ascii_lowercase().contains("upgrade: websocket") {
            assert!(request.starts_with("GET /runtime/agents/agent-a/conversation?outputAck=1 "));
            let key = request
                .lines()
                .find_map(|line| {
                    line.split_once(':')
                        .filter(|(k, _)| k.eq_ignore_ascii_case("sec-websocket-key"))
                        .map(|(_, v)| v.trim())
                })
                .unwrap();
            let accept =
                tokio_tungstenite::tungstenite::handshake::derive_accept_key(key.as_bytes());
            stream.write_all(format!("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n").as_bytes()).await.unwrap();
            let mut socket = tokio_tungstenite::WebSocketStream::from_raw_socket(
                stream,
                tokio_tungstenite::tungstenite::protocol::Role::Server,
                None,
            )
            .await;
            while let Some(Ok(message)) = socket.next().await {
                if socket.send(message).await.is_err() {
                    return;
                }
            }
        } else {
            let body = if request.starts_with("POST /runtime/control ") {
                if matches!(mode, Mode::WireError) {
                    serde_json::to_string(&ResponseFrame::Failure(crate::wire::WireFailure {
                        operation: crate::wire::WireOperation::Resources,
                        code: "runtime_failure".into(),
                        detail: format!("echoed credential {token}"),
                        retryable: false,
                    }))
                    .unwrap()
                } else if matches!(mode, Mode::Persistent) {
                    persistent_response(&request)
                } else {
                    serde_json::to_string(&ResponseFrame::Resources(ResourceReading::default()))
                        .unwrap()
                }
            } else {
                assert!(request.starts_with("GET /runtime/agents/agent-a/history "));
                "{\"history\":[]}".into()
            };
            let connection = if matches!(mode, Mode::Persistent) {
                "keep-alive"
            } else {
                "close"
            };
            if stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: {connection}\r\n\r\n{body}", body.len()).as_bytes()).await.is_err() { return; }
            if matches!(mode, Mode::Persistent) {
                continue;
            }
        }
        return;
    }
}

fn persistent_response(request: &str) -> String {
    let (_, body) = request.split_once("\r\n\r\n").unwrap();
    let response = match serde_json::from_str::<RequestFrame>(body).unwrap() {
        RequestFrame::OpenAgent(request) => {
            let launch = request.request;
            ResponseFrame::OpenAgent(FencedReceipt::new(request.authority_fence, AgentOpenReceipt {
                operation_id: launch.operation_id,
                request_sha256: launch.request_sha256,
                agent_id: launch.agent_id,
                cwd: launch.cwd,
                home: launch.home,
                conversation_id: None,
            }))
        },
        RequestFrame::Install(request) => ResponseFrame::Install(crate::InstallReceipt {
            operation_id: request.operation_id,
            request_sha256: request.request_sha256,
            commands: vec![],
        }),
        RequestFrame::RuntimeStatus(request) => ResponseFrame::RuntimeStatus(serde_json::from_value(json!({
            "sandbox_id":request.sandbox_id,"generation":request.generation,"home_identity":request.home_identity,
            "boot_id":"a".repeat(32),"installation_ready":true,"credential_ready":false,"restore_pending":false,"agent_processes":0
        })).unwrap()),
        RequestFrame::Resources => ResponseFrame::Resources(ResourceReading::default()),
        _ => panic!("unexpected persistent fixture operation"),
    };
    serde_json::to_string(&response).unwrap()
}

#[path = "native_open_overlap_tests.rs"]
mod open_overlap_tests;
#[path = "native_http_reuse_tests.rs"]
mod reuse_tests;

#[tokio::test]
async fn native_api_stream_carries_authenticated_control_history_and_websocket() {
    for transport_mode in [
        NativeRuntimeTransport::KubernetesPortforward,
        NativeRuntimeTransport::KubernetesExec,
    ] {
        let fixture = Fixture::start(Mode::Success, transport_mode).await;
        let endpoint = fixture.endpoint().await;
        assert_eq!(
            fixture
                .transport
                .control(&endpoint, &RequestFrame::Resources)
                .await
                .unwrap(),
            ResponseFrame::Resources(ResourceReading::default())
        );
        assert_eq!(
            fixture
                .transport
                .export_agent_history(&handle(), &AgentSlot::new("agent-a"))
                .await
                .unwrap(),
            b"{\"history\":[]}"
        );
        let mut socket = fixture
            .transport
            .connect(&endpoint, &AgentSlot::new("agent-a"))
            .await
            .unwrap();
        socket
            .send(Message::Binary(vec![0x02, 1, 2, 3]))
            .await
            .unwrap();
        assert_eq!(
            socket.next().await.unwrap().unwrap(),
            Message::Binary(vec![0x02, 1, 2, 3])
        );
        drop(socket);
        fixture.drained().await;
        assert_eq!(fixture.state.opened.load(Ordering::SeqCst), 3);
        assert_eq!(fixture.state.requests.lock().unwrap().len(), 3);
    }
}

#[tokio::test]
async fn native_api_stream_rejects_replaced_pod_before_open_and_wrong_ownership() {
    for transport_mode in [
        NativeRuntimeTransport::KubernetesPortforward,
        NativeRuntimeTransport::KubernetesExec,
    ] {
        for ownership in [false, true] {
            let fixture = Fixture::start(Mode::Success, transport_mode).await;
            let endpoint = fixture.endpoint().await;
            if ownership {
                fixture.state.wrong_owner.store(true, Ordering::SeqCst);
            } else {
                fixture.state.replaced.store(true, Ordering::SeqCst);
            }
            assert!(fixture
                .transport
                .control(&endpoint, &RequestFrame::Resources)
                .await
                .is_err());
            assert_eq!(fixture.state.opened.load(Ordering::SeqCst), 0);
            assert!(fixture.state.requests.lock().unwrap().is_empty());
        }
    }
}

#[tokio::test]
async fn native_api_stream_rejects_replacement_during_open_before_sending_token() {
    for transport_mode in [
        NativeRuntimeTransport::KubernetesPortforward,
        NativeRuntimeTransport::KubernetesExec,
    ] {
        let fixture = Fixture::start(Mode::ReplaceOnOpen, transport_mode).await;
        let endpoint = fixture.endpoint().await;
        assert!(fixture
            .transport
            .control(&endpoint, &RequestFrame::Resources)
            .await
            .unwrap_err()
            .to_string()
            .contains("Pod changed"));
        fixture.drained().await;
        assert!(fixture.state.requests.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn native_api_stream_scrubs_refusals_channel_errors_and_runtime_error_bodies() {
    for transport_mode in [
        NativeRuntimeTransport::KubernetesPortforward,
        NativeRuntimeTransport::KubernetesExec,
    ] {
        for mode in [
            Mode::Forbidden,
            Mode::Throttled,
            Mode::ErrorChannel,
            Mode::HttpError,
        ] {
            let fixture = Fixture::start(mode, transport_mode).await;
            let endpoint = fixture.endpoint().await;
            let error = fixture
                .transport
                .control(&endpoint, &RequestFrame::Resources)
                .await
                .unwrap_err();
            let text = format!("{error:?}");
            assert!(!text.contains(ERROR_SECRET));
            assert!(!text.contains(API_TOKEN));
            assert!(!text.contains(endpoint.control_token().unwrap()));
            if matches!(mode, Mode::Forbidden) {
                assert!(matches!(error, AdapterError::Rejected(_)));
            }
            if matches!(mode, Mode::Throttled) {
                assert!(matches!(error, AdapterError::AtCapacity(_)));
            }
            fixture.drained().await;
            assert_eq!(fixture.state.opened.load(Ordering::SeqCst), 1);
        }
    }
}

#[tokio::test]
async fn native_api_stream_never_retries_a_control_request_after_lost_response() {
    for transport_mode in [
        NativeRuntimeTransport::KubernetesPortforward,
        NativeRuntimeTransport::KubernetesExec,
    ] {
        let fixture = Fixture::start(Mode::LostResponse, transport_mode).await;
        let endpoint = fixture.endpoint().await;
        let mutation = RequestFrame::Install(crate::InstallRequest::new(
            "install-once",
            vec!["echo side-effect".into()],
        ));
        assert!(fixture
            .transport
            .control(&endpoint, &mutation)
            .await
            .is_err());
        fixture.drained().await;
        assert_eq!(fixture.state.opened.load(Ordering::SeqCst), 1);
        assert_eq!(fixture.state.requests.lock().unwrap().len(), 1);
        assert!(fixture.state.requests.lock().unwrap()[0].contains("install-once"));
    }
}

#[tokio::test]
async fn native_api_stream_cancellation_and_oversized_chunked_body_close_forwarder() {
    for transport_mode in [
        NativeRuntimeTransport::KubernetesPortforward,
        NativeRuntimeTransport::KubernetesExec,
    ] {
        for mode in [Mode::Stall, Mode::StallAfterOpen, Mode::LargeBody] {
            let fixture = Fixture::start(mode, transport_mode).await;
            let endpoint = fixture.endpoint().await;
            let operation = fixture.transport.native_http(
                &endpoint,
                http::Method::POST,
                endpoint.control().unwrap(),
                b"{}".to_vec(),
                32,
            );
            let result = tokio::time::timeout(Duration::from_millis(250), operation).await;
            if matches!(mode, Mode::Stall | Mode::StallAfterOpen) {
                assert!(result.is_err());
            } else {
                assert!(matches!(result.unwrap(), Err(AdapterError::Rejected(_))));
            }
            fixture.drained().await;
            assert_eq!(fixture.state.opened.load(Ordering::SeqCst), 1);
        }
    }
}

#[tokio::test]
async fn native_api_stream_scrubs_runtime_token_from_typed_failure() {
    for transport_mode in [
        NativeRuntimeTransport::KubernetesPortforward,
        NativeRuntimeTransport::KubernetesExec,
    ] {
        let fixture = Fixture::start(Mode::WireError, transport_mode).await;
        let endpoint = fixture.endpoint().await;
        let response = fixture
            .transport
            .control(&endpoint, &RequestFrame::Resources)
            .await
            .unwrap();
        let ResponseFrame::Failure(failure) = response else {
            panic!("expected typed failure");
        };
        assert_eq!(failure.detail, "echoed credential [redacted]");
        fixture.drained().await;
    }
}

#[tokio::test]
async fn native_api_stream_silent_upstream_close_refuses_further_writes() {
    for transport_mode in [
        NativeRuntimeTransport::KubernetesPortforward,
        NativeRuntimeTransport::KubernetesExec,
    ] {
        let fixture = Fixture::start(Mode::SilentClose, transport_mode).await;
        let endpoint = fixture.endpoint().await;
        let stream = fixture
            .transport
            .native_stream(&endpoint, RuntimeStreamKind::Conversation)
            .await;
        let Ok(mut stream) = stream else {
            assert_eq!(transport_mode, NativeRuntimeTransport::KubernetesExec);
            fixture.drained().await;
            continue;
        };
        let mut bytes = [0; 1];
        let closed = tokio::time::timeout(Duration::from_secs(2), stream.read(&mut bytes))
            .await
            .unwrap();
        assert!(closed.is_err() || closed.unwrap() == 0);
        assert!(stream
            .write_all(b"must-not-reach-a-closed-runtime")
            .await
            .is_err());
        drop(stream);
        fixture.drained().await;
        assert!(fixture.state.requests.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn native_api_stream_conversations_have_a_shared_bound_and_reserve_control_capacity() {
    for transport_mode in [
        NativeRuntimeTransport::KubernetesPortforward,
        NativeRuntimeTransport::KubernetesExec,
    ] {
        let fixture = Fixture::start(Mode::Success, transport_mode).await;
        let endpoint = fixture.endpoint().await;
        let mut streams = Vec::new();
        for _ in 0..64 {
            streams.push(
                fixture
                    .transport
                    .native_stream(&endpoint, RuntimeStreamKind::Conversation)
                    .await
                    .unwrap(),
            );
        }
        assert!(matches!(
            fixture
                .transport
                .native_stream(&endpoint, RuntimeStreamKind::Conversation)
                .await,
            Err(AdapterError::AtCapacity(_))
        ));
        fixture
            .transport
            .control(&endpoint, &RequestFrame::Resources)
            .await
            .unwrap();
        drop(streams.pop());
        streams.push(
            fixture
                .transport
                .native_stream(&endpoint, RuntimeStreamKind::Conversation)
                .await
                .unwrap(),
        );
        drop(streams);
        fixture.drained().await;
        assert_eq!(fixture.state.opened.load(Ordering::SeqCst), 66);
    }
}

#[tokio::test]
async fn native_exec_success_status_preserves_buffered_stdout_and_refuses_writes() {
    let fixture = Fixture::start(Mode::BufferedExit, NativeRuntimeTransport::KubernetesExec).await;
    let endpoint = fixture.endpoint().await;
    let mut stream = fixture
        .transport
        .native_stream(&endpoint, RuntimeStreamKind::Request)
        .await
        .unwrap();
    fixture.drained().await;
    // Let the kube driver observe the exit while the caller deliberately delays reading.
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(stream.write_all(b"after-exit").await.is_err());
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes).await.unwrap();
    assert_eq!(bytes, b"buffered-response");
    assert!(fixture.state.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn native_exec_history_drains_large_buffered_response_after_successful_exit() {
    let fixture = Fixture::start(
        Mode::BufferedHistoryExit,
        NativeRuntimeTransport::KubernetesExec,
    )
    .await;
    let expected = "{\"history\":\"".to_owned() + &"x".repeat(48 * 1024) + "\"}\n";
    for _ in 0..8 {
        let history = fixture
            .transport
            .export_agent_history(&handle(), &AgentSlot::new("agent-a"))
            .await
            .unwrap();
        assert_eq!(history, expected.as_bytes());
    }
    fixture.drained().await;
    assert_eq!(fixture.state.opened.load(Ordering::SeqCst), 8);
    assert_eq!(fixture.state.requests.lock().unwrap().len(), 8);
}

#[tokio::test]
async fn native_exec_readiness_failure_and_cancellation_never_send_runtime_credentials() {
    for mode in [Mode::StallBeforeReady, Mode::InvalidReady] {
        let fixture = Fixture::start(mode, NativeRuntimeTransport::KubernetesExec).await;
        let endpoint = fixture.endpoint().await;
        let result = tokio::time::timeout(
            Duration::from_millis(100),
            fixture
                .transport
                .control(&endpoint, &RequestFrame::Resources),
        )
        .await;
        if matches!(mode, Mode::StallBeforeReady) {
            assert!(result.is_err());
        } else {
            assert!(matches!(result.unwrap(), Err(AdapterError::Rejected(_))));
        }
        fixture.drained().await;
        assert!(fixture.state.requests.lock().unwrap().is_empty());
    }
}
