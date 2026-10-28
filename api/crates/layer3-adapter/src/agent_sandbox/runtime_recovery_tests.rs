use super::*;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tokio_tungstenite::{
    accept_hdr_async,
    tungstenite::{
        handshake::server::{Request, Response},
        Message,
    },
};

const INPUT: &[u8] = b"fixture-private-inspection-configuration";
const OUTPUT: &[u8] = br#"{"verified":true}"#;
const SCRIPT: &str = "import sys; sys.stdout.write('{}')";

#[derive(Clone, Copy)]
enum Mode {
    Success,
    EmptyInput,
    ExactLimit,
    Failure,
    MissingStatus,
    LargeOutput,
    ReplacedPod,
    ReplacedSandbox,
    RestartedContainer,
    MissingIncarnation,
    ForeignOwner,
    Forbidden,
    Stall,
    StallDiscovery,
}

#[derive(Default)]
struct State {
    calls: AtomicUsize,
    executions: AtomicUsize,
    active: AtomicUsize,
}
struct Active(Arc<State>);
impl Drop for Active {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::SeqCst);
    }
}
struct Fixture {
    client: AgentSandboxClient,
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
fn command() -> RuntimeRecoveryCommand {
    RuntimeRecoveryCommand {
        program: "/usr/bin/python3".into(),
        args: ["-I", "-u", "-c", SCRIPT].map(String::from).to_vec(),
        input: INPUT.to_vec(),
    }
}
fn object(replaced: bool) -> Value {
    json!({"apiVersion":"agents.x-k8s.io/v1beta1","kind":"Sandbox",
        "metadata":{"name":"claimed","namespace":"cma-test",
            "uid":if replaced {"replacement-sandbox"} else {"sandbox-uid"},
            "resourceVersion":"7","generation":1,"labels":{"cma/control-plane":"cma-test"}},
        "status":{"conditions":[{"type":"Ready","status":"True","observedGeneration":1}]}})
}
fn pods(mode: Mode, after: bool) -> Value {
    let restarted = after && matches!(mode, Mode::RestartedContainer);
    json!({"apiVersion":"v1","kind":"PodList","metadata":{"resourceVersion":"8"},"items":[{
        "metadata":{"name":"claimed-pod","namespace":"cma-test",
            "uid":if after && matches!(mode, Mode::ReplacedPod) {"replacement-pod"} else {"original-pod"},
            "labels":{"cma/control-plane":"cma-test",super::super::pods::TRACKING_LABEL:super::super::pods::name_hash("claimed")},
            "ownerReferences":[{"apiVersion":"agents.x-k8s.io/v1beta1","kind":"Sandbox","name":"claimed",
                "uid":if matches!(mode, Mode::ForeignOwner) {"foreign-owner"} else {"sandbox-uid"},"controller":true}]},
        "status":{"phase":"Running","podIP":"10.0.0.1","conditions":[{"type":"Ready","status":"True"}],
            "containerStatuses":[{"name":"runtime","image":"runtime","imageID":"sha256:runtime","ready":true,
                "restartCount":if restarted {1} else {0},
                "containerID":if matches!(mode, Mode::MissingIncarnation) {""} else if restarted {"containerd://replacement"} else {"containerd://original"},
                "state":{"running":{}}}]}}]})
}

impl Fixture {
    async fn start(mode: Mode) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let state = Arc::new(State::default());
        let served = state.clone();
        let task = tokio::spawn(async move {
            let mut tasks = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let (socket, _) = accepted.unwrap();
                        let state = served.clone();
                        tasks.spawn(async move { serve(socket, mode, state).await; });
                    }
                    result = tasks.join_next(), if !tasks.is_empty() => { result.unwrap().unwrap(); }
                }
            }
        });
        let mut configuration = kube::Config::new(format!("http://{address}").parse().unwrap());
        configuration.default_retry = false;
        let client = AgentSandboxClient::new(
            kube::Client::try_from(configuration).unwrap(),
            super::super::tests::config(),
            "fixture",
        )
        .unwrap();
        Self {
            client,
            state,
            task,
        }
    }

    async fn inspect(&self, command: &RuntimeRecoveryCommand) -> Result<Vec<u8>> {
        tokio::time::timeout(
            Duration::from_secs(3),
            self.client.run_recovery_command(&handle(), command),
        )
        .await
        .expect("inspection must finish within test deadline")
    }

    async fn wait_for(&self, count: &AtomicUsize) {
        tokio::time::timeout(Duration::from_secs(3), async {
            while count.load(Ordering::SeqCst) == 0 {
                assert!(!self.task.is_finished(), "fixture task failed");
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("fixture received inspection");
    }

    async fn drained(&self) {
        tokio::time::timeout(Duration::from_secs(2), async {
            while self.state.active.load(Ordering::SeqCst) != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("inspection transport closed after cancellation or completion");
        assert!(!self.task.is_finished(), "fixture task failed");
        assert_eq!(self.client.runtime_requests.available_permits(), 32);
    }
}

async fn serve(mut socket: tokio::net::TcpStream, mode: Mode, state: Arc<State>) {
    let mut header = vec![0; 8192];
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
    let head = String::from_utf8_lossy(&header[..count]);
    let line = head.lines().next().unwrap();
    assert!(
        line.starts_with("GET "),
        "inspection never mutates Kubernetes resources"
    );
    assert!(!head.contains(std::str::from_utf8(INPUT).unwrap()));
    let path = line.split_whitespace().nth(1).unwrap();
    state.calls.fetch_add(1, Ordering::SeqCst);
    if path.contains("/exec?") {
        assert!(path.starts_with("/api/v1/namespaces/cma-test/pods/claimed-pod/exec?"));
        if matches!(mode, Mode::Forbidden) {
            let body = json!({"apiVersion":"v1","kind":"Status","status":"Failure","reason":"Forbidden","code":403,
                "message":std::str::from_utf8(INPUT).unwrap()}).to_string();
            socket.write_all(format!("HTTP/1.1 403 Forbidden\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
            return;
        }
        #[allow(clippy::result_large_err)]
        let mut ws = accept_hdr_async(socket, |request: &Request, mut response: Response| {
            let url = url::Url::parse(&format!("http://fixture{}", request.uri())).unwrap();
            let parameters: Vec<_> = url.query_pairs().collect();
            let commands: Vec<_> = parameters
                .iter()
                .filter(|(key, _)| key == "command")
                .map(|(_, value)| value.as_ref())
                .collect();
            assert_eq!(commands, ["/usr/bin/python3", "-I", "-u", "-c", SCRIPT]);
            for (key, value) in [
                ("container", "runtime"),
                ("stdin", "true"),
                ("stdout", "true"),
            ] {
                assert!(parameters.iter().any(|(k, v)| k == key && v == value));
            }
            for key in ["stderr", "tty"] {
                assert!(!parameters.iter().any(|(k, v)| k == key && v == "true"));
            }
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
        assert_eq!(
            input,
            match mode {
                Mode::ExactLimit => vec![b'x'; IO_LIMIT],
                Mode::EmptyInput => vec![],
                _ => INPUT.to_vec(),
            }
        );
        state.executions.fetch_add(1, Ordering::SeqCst);
        if matches!(mode, Mode::Stall) {
            while ws.next().await.is_some_and(|message| message.is_ok()) {}
            return;
        }
        let mut output = vec![1];
        output.extend(match mode {
            Mode::LargeOutput => vec![b'x'; IO_LIMIT + 1],
            Mode::ExactLimit => vec![b'x'; IO_LIMIT],
            _ => OUTPUT.to_vec(),
        });
        ws.send(Message::Binary(output)).await.unwrap();
        if matches!(mode, Mode::LargeOutput) {
            // Overflow must fail without depending on a remote EOF or exit status.
            while ws.next().await.is_some_and(|message| message.is_ok()) {}
            return;
        }
        if !matches!(mode, Mode::MissingStatus) {
            let mut status = vec![3];
            status.extend(
                serde_json::to_vec(&json!({"apiVersion":"v1","kind":"Status",
                "status":if matches!(mode, Mode::Failure) {"Failure"} else {"Success"},
                "message":std::str::from_utf8(INPUT).unwrap()}))
                .unwrap(),
            );
            let _ = ws.send(Message::Binary(status)).await;
        }
        let _ = ws.close(None).await;
        return;
    }
    if matches!(mode, Mode::StallDiscovery) {
        std::future::pending::<()>().await;
    }
    let after = state.executions.load(Ordering::SeqCst) > 0;
    let value = if path.starts_with("/api/v1/namespaces/cma-test/pods?") {
        pods(mode, after)
    } else {
        assert_eq!(
            path,
            "/apis/agents.x-k8s.io/v1beta1/namespaces/cma-test/sandboxes/claimed"
        );
        object(after && matches!(mode, Mode::ReplacedSandbox))
    };
    let mut consumed = vec![0; count];
    socket.read_exact(&mut consumed).await.unwrap();
    let body = value.to_string();
    socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
}

#[tokio::test]
async fn recovery_command_uses_exact_argv_private_stdin_and_owned_identity_before_and_after() {
    let fixture = Fixture::start(Mode::Success).await;
    assert_eq!(fixture.inspect(&command()).await.unwrap(), OUTPUT);
    assert_eq!(fixture.state.calls.load(Ordering::SeqCst), 5);
    assert_eq!(fixture.state.executions.load(Ordering::SeqCst), 1);
    fixture.drained().await;
    let fixture = Fixture::start(Mode::EmptyInput).await;
    assert_eq!(
        fixture
            .inspect(&RuntimeRecoveryCommand {
                input: vec![],
                ..command()
            })
            .await
            .unwrap(),
        OUTPUT
    );
    fixture.drained().await;
    let fixture = Fixture::start(Mode::ExactLimit).await;
    let mut command = command();
    command.input = vec![b'x'; IO_LIMIT];
    assert_eq!(
        fixture.inspect(&command).await.unwrap(),
        vec![b'x'; IO_LIMIT]
    );
    fixture.drained().await;
}

#[tokio::test]
async fn recovery_command_rejects_exit_output_and_identity_failures_without_private_diagnostics() {
    for mode in [
        Mode::Failure,
        Mode::MissingStatus,
        Mode::LargeOutput,
        Mode::ReplacedPod,
        Mode::ReplacedSandbox,
        Mode::RestartedContainer,
        Mode::Forbidden,
    ] {
        let fixture = Fixture::start(mode).await;
        let error = fixture.inspect(&command()).await.unwrap_err();
        assert!(!error
            .to_string()
            .contains(std::str::from_utf8(INPUT).unwrap()));
        assert_eq!(
            fixture.state.executions.load(Ordering::SeqCst),
            usize::from(!matches!(mode, Mode::Forbidden))
        );
        fixture.drained().await;
    }
}

#[tokio::test]
async fn recovery_command_refuses_missing_incarnation_or_foreign_owner_before_exec() {
    for mode in [Mode::MissingIncarnation, Mode::ForeignOwner] {
        let fixture = Fixture::start(mode).await;
        assert!(fixture.inspect(&command()).await.is_err());
        assert_eq!(fixture.state.calls.load(Ordering::SeqCst), 2);
        assert_eq!(fixture.state.executions.load(Ordering::SeqCst), 0);
        fixture.drained().await;
    }
}

#[tokio::test]
async fn recovery_command_bounds_input_and_argv_and_refuses_capacity_without_kubernetes_requests() {
    let fixture = Fixture::start(Mode::Success).await;
    for invalid in [
        RuntimeRecoveryCommand {
            input: vec![0; IO_LIMIT + 1],
            ..command()
        },
        RuntimeRecoveryCommand {
            program: "python3".into(),
            ..command()
        },
        RuntimeRecoveryCommand {
            args: vec!["nul\0argument".into()],
            ..command()
        },
        RuntimeRecoveryCommand {
            args: vec!["x".repeat(ARGUMENT_LIMIT)],
            ..command()
        },
    ] {
        assert!(matches!(
            fixture.inspect(&invalid).await,
            Err(AdapterError::Rejected(_))
        ));
    }
    let permits = fixture
        .client
        .runtime_requests
        .clone()
        .acquire_many_owned(32)
        .await
        .unwrap();
    let clone = fixture.client.clone();
    assert!(matches!(
        clone.run_recovery_command(&handle(), &command()).await,
        Err(AdapterError::AtCapacity(_))
    ));
    assert_eq!(fixture.state.calls.load(Ordering::SeqCst), 0);
    drop(permits);
    fixture.drained().await;
}

#[tokio::test]
async fn recovery_command_cancellation_aborts_exec_and_returns_shared_request_capacity() {
    let fixture = Fixture::start(Mode::Stall).await;
    let client = fixture.client.clone();
    let task =
        tokio::spawn(async move { client.run_recovery_command(&handle(), &command()).await });
    fixture.wait_for(&fixture.state.executions).await;
    assert_eq!(fixture.client.runtime_requests.available_permits(), 31);
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    fixture.drained().await;
}

#[tokio::test]
async fn recovery_command_deadline_covers_discovery_and_exec_and_releases_capacity() {
    for mode in [Mode::StallDiscovery, Mode::Stall] {
        let fixture = Fixture::start(mode).await;
        let client = fixture.client.clone();
        let task =
            tokio::spawn(async move { client.run_recovery_command(&handle(), &command()).await });
        fixture
            .wait_for(if matches!(mode, Mode::Stall) {
                &fixture.state.executions
            } else {
                &fixture.state.calls
            })
            .await;
        tokio::time::pause();
        tokio::time::advance(RECOVERY_TIMEOUT + Duration::from_secs(1)).await;
        let error = task.await.unwrap().unwrap_err();
        tokio::time::resume();
        assert!(
            matches!(error, AdapterError::Unavailable(ref message) if message == "runtime recovery command timed out")
        );
        fixture.drained().await;
    }
}
