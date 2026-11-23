use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Semaphore;

#[derive(Clone, Copy)]
enum ResponseEnd {
    Disconnect,
    Truncated,
    Stall,
}

struct AcceptedRequest {
    head: String,
    body: Vec<u8>,
}

struct Peer {
    origin: String,
    calls: Arc<Mutex<Vec<AcceptedRequest>>>,
    received: Arc<Semaphore>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Peer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Peer {
    async fn start(end: ResponseEnd) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let calls = Arc::new(Mutex::new(Vec::new()));
        let recorded = calls.clone();
        let received = Arc::new(Semaphore::new(0));
        let observed = received.clone();
        let task = tokio::spawn(async move {
            let mut connections = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let (stream, _) = accepted.unwrap();
                        connections.spawn(accept_request(stream, recorded.clone(), observed.clone(), end));
                    }
                    result = connections.join_next(), if !connections.is_empty() => {
                        result.unwrap().unwrap();
                    }
                }
            }
        });
        Self {
            origin,
            calls,
            received,
            task,
        }
    }
}

async fn accept_request(
    mut stream: tokio::net::TcpStream,
    calls: Arc<Mutex<Vec<AcceptedRequest>>>,
    received: Arc<Semaphore>,
    end: ResponseEnd,
) {
    let mut bytes = Vec::new();
    let (head, offset, length) = loop {
        let mut chunk = [0; 4096];
        let n = stream.read(&mut chunk).await.unwrap();
        assert!(n > 0 && bytes.len() + n <= 64 * 1024);
        bytes.extend_from_slice(&chunk[..n]);
        if let Some(offset) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            let head = String::from_utf8(bytes[..offset].to_vec()).unwrap();
            let length = head
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().unwrap())
                })
                .unwrap();
            assert!(length <= 32 * 1024);
            break (head, offset + 4, length);
        }
    };
    while bytes.len() < offset + length {
        let mut chunk = [0; 4096];
        let n = stream.read(&mut chunk).await.unwrap();
        assert!(n > 0 && bytes.len() + n <= 64 * 1024);
        bytes.extend_from_slice(&chunk[..n]);
    }
    calls.lock().unwrap().push(AcceptedRequest {
        head,
        body: bytes[offset..offset + length].to_vec(),
    });
    if !matches!(end, ResponseEnd::Disconnect) {
        stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 1024\r\nConnection: close\r\n\r\n{\"status\":\"completed\"").await.unwrap();
    }
    received.add_permits(1);
    if matches!(end, ResponseEnd::Stall) {
        std::future::pending::<()>().await;
    }
}

async fn verify(end: ResponseEnd) {
    for suggestions in [false, true] {
        let budget = budget(None);
        let peer = Peer::start(end).await;
        let origin = peer.origin.clone();
        let selected_budget = budget.clone();
        let task = tokio::spawn(async move {
            if suggestions {
                assert!(suggestion(&origin, selected_budget).await.is_err());
            } else {
                assert_eq!(
                    title(&origin, selected_budget).await,
                    Err(AgentTitleServiceError::GenerationUnavailable)
                );
            }
        });
        tokio::time::timeout(std::time::Duration::from_secs(5), peer.received.acquire())
            .await
            .unwrap()
            .unwrap()
            .forget();
        if matches!(end, ResponseEnd::Stall) {
            tokio::time::pause();
            tokio::time::advance(std::time::Duration::from_secs(
                GENERATION_TIMEOUT_SECONDS + 1,
            ))
            .await;
            tokio::time::resume();
        }
        tokio::time::timeout(std::time::Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(budget.reserved.load(Ordering::SeqCst), 1);
        assert!(budget.settled.lock().unwrap().is_empty());
        let calls = peer.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert!(calls[0].head.starts_with("POST /v1/responses HTTP/1.1\r\n"));
        let holds = budget.holds.lock().unwrap();
        assert_eq!(holds.len(), 1);
        assert_eq!(
            holds[0].scope,
            context(
                budget.clone(),
                if suggestions {
                    "suggestion-proof"
                } else {
                    "title-proof"
                }
            )
            .scope
        );
        assert_eq!(holds[0].request.model, "budget-model");
        assert_eq!(
            holds[0].request.fingerprint,
            format!("{:x}", Sha256::digest(&calls[0].body))
        );
        assert_eq!(holds[0].request.input_bytes, calls[0].body.len() as u64);
    }
}

#[tokio::test]
async fn accepted_disconnect_keeps_the_exact_auxiliary_hold_without_http_retry() {
    verify(ResponseEnd::Disconnect).await;
}

#[tokio::test]
async fn truncated_success_keeps_the_exact_auxiliary_hold_without_http_retry() {
    verify(ResponseEnd::Truncated).await;
}

#[tokio::test]
async fn response_body_timeout_keeps_the_exact_auxiliary_hold_without_http_retry() {
    verify(ResponseEnd::Stall).await;
}
