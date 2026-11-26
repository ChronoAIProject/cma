use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{mpsc, Semaphore},
    task::{JoinHandle, JoinSet},
};

pub(super) struct Captured {
    pub headers: String,
    pub body: Vec<u8>,
}

pub(super) struct Peer {
    pub origin: String,
    incoming: mpsc::UnboundedReceiver<Captured>,
    release: Arc<Semaphore>,
    calls: Arc<AtomicUsize>,
    task: Option<JoinHandle<()>>,
}

impl Peer {
    pub async fn new(response: Option<Vec<u8>>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let (sender, incoming) = mpsc::unbounded_channel();
        let release = Arc::new(Semaphore::new(0));
        let calls = Arc::new(AtomicUsize::new(0));
        let gate = release.clone();
        let counter = calls.clone();
        let task = tokio::spawn(async move {
            let mut connections = JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let (mut socket, _) = accepted.unwrap();
                        let sender = sender.clone();
                        let gate = gate.clone();
                        let counter = counter.clone();
                        let response = response.clone();
                        connections.spawn(async move {
                            let captured = read_request(&mut socket).await;
                            counter.fetch_add(1, Ordering::SeqCst);
                            sender.send(captured).unwrap();
                            gate.acquire().await.unwrap().forget();
                            if let Some(response) = response {
                                socket.write_all(&response).await.unwrap();
                            }
                            socket.shutdown().await.unwrap();
                        });
                    }
                    completed = connections.join_next(), if !connections.is_empty() => {
                        completed.unwrap().unwrap();
                    }
                }
            }
        });
        Self {
            origin,
            incoming,
            release,
            calls,
            task: Some(task),
        }
    }

    pub async fn received(&mut self) -> Captured {
        tokio::time::timeout(Duration::from_secs(10), self.incoming.recv())
            .await
            .expect("request reaches the peer")
            .unwrap()
    }

    pub fn respond(&self) {
        self.release.add_permits(1);
    }

    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    pub async fn stop(mut self) {
        let task = self.task.take().unwrap();
        task.abort();
        assert!(task.await.is_err_and(|error| error.is_cancelled()));
    }
}

impl Drop for Peer {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

async fn read_request(socket: &mut TcpStream) -> Captured {
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 1024];
    let end = loop {
        let size = socket.read(&mut chunk).await.unwrap();
        assert!(size > 0 && bytes.len() + size <= 64 * 1024);
        bytes.extend_from_slice(&chunk[..size]);
        if let Some(index) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break index + 4;
        }
    };
    let headers = String::from_utf8(bytes[..end].to_vec()).unwrap();
    assert!(headers.starts_with("POST /v1/responses HTTP/1.1\r\n"));
    let length = headers
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .unwrap()
        .1
        .trim()
        .parse::<usize>()
        .unwrap();
    assert!(length <= 32 * 1024);
    while bytes.len() < end + length {
        let size = socket.read(&mut chunk).await.unwrap();
        assert!(size > 0 && bytes.len() + size <= 64 * 1024);
        bytes.extend_from_slice(&chunk[..size]);
    }
    assert_eq!(bytes.len(), end + length);
    Captured {
        headers,
        body: bytes[end..].to_vec(),
    }
}

pub(super) fn json_response(value: &serde_json::Value) -> Vec<u8> {
    let body = serde_json::to_vec(value).unwrap();
    let mut response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    response.extend(body);
    response
}
