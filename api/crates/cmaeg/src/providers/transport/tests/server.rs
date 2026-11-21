use crate::providers::transport::{io::BoxIo, ResponseTransport};
use axum::http::{Request, Response};
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::{body::Incoming, service::service_fn};
use hyper_util::rt::{TokioExecutor, TokioIo};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use std::{
    convert::Infallible,
    net::SocketAddr,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::{net::TcpListener, sync::Semaphore, task::JoinSet};

pub struct Server {
    pub address: SocketAddr,
    pub connections: Arc<AtomicUsize>,
    pub requests: Arc<Mutex<Vec<(String, usize)>>>,
    pub tls: Option<Arc<rustls::ClientConfig>>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Server {
    pub async fn start(h2: bool, tls: bool, delay: Duration) -> Self {
        Self::start_inner(h2, tls, delay, None).await
    }
    pub async fn start_gated() -> (Self, Arc<Semaphore>) {
        let gate = Arc::new(Semaphore::new(0));
        let server = Self::start_inner(true, false, Duration::ZERO, Some(gate.clone())).await;
        (server, gate)
    }
    async fn start_inner(
        h2: bool,
        tls: bool,
        delay: Duration,
        gate: Option<Arc<Semaphore>>,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let connections = Arc::new(AtomicUsize::new(0));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let count = connections.clone();
        let recording = requests.clone();
        let (acceptor, client_tls) = if tls {
            let (server, client) = certificates(h2);
            (
                Some(tokio_rustls::TlsAcceptor::from(Arc::new(server))),
                Some(Arc::new(client)),
            )
        } else {
            (None, None)
        };
        let task = tokio::spawn(async move {
            let mut tasks = JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let (tcp, _) = accepted.unwrap();
                        tcp.set_nodelay(true).unwrap();
                        count.fetch_add(1, Ordering::SeqCst);
                        let recording = recording.clone();
                        let acceptor = acceptor.clone();
                        let gate = gate.clone();
                        tasks.spawn(async move {
                            let io: BoxIo = match acceptor {
                                Some(acceptor) => match acceptor.accept(tcp).await {
                                    Ok(tls) => Box::new(tls), Err(_) => return,
                                },
                                None => Box::new(tcp),
                            };
                            let service = service_fn(move |request: Request<Incoming>| {
                                let recording = recording.clone();
                                let gate = gate.clone();
                                async move {
                                    assert_eq!(request.method(), "POST");
                                    assert_eq!(request.uri().path(), "/responses");
                                    let id = request.headers()["idempotency-key"].to_str().unwrap().to_owned();
                                    let bytes = request.into_body().collect().await.unwrap().to_bytes();
                                    recording.lock().unwrap().push((id, bytes.len()));
                                    if let Some(gate) = gate {
                                        gate.acquire().await.unwrap().forget();
                                    }
                                    tokio::time::sleep(delay).await;
                                    Ok::<_, Infallible>(Response::builder().header("content-type", "text/event-stream")
                                        .body(Full::new(Bytes::from_static(b"data: {\"type\":\"response.completed\"}\n\n"))).unwrap())
                                }
                            });
                            if h2 {
                                let _ = hyper::server::conn::http2::Builder::new(TokioExecutor::new())
                                    .serve_connection(TokioIo::new(io), service).await;
                            } else {
                                let _ = hyper::server::conn::http1::Builder::new()
                                    .serve_connection(TokioIo::new(io), service).await;
                            }
                        });
                    }
                    result = tasks.join_next(), if !tasks.is_empty() => { result.unwrap().unwrap(); }
                }
            }
        });
        Self {
            address,
            connections,
            requests,
            tls: client_tls,
            task,
        }
    }
    pub async fn wait_for_requests(&self, count: usize) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while self.requests.lock().unwrap().len() < count {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("server did not receive the expected request bodies");
    }
    pub fn client(&self, h2: bool) -> ResponseTransport {
        let origin = if self.tls.is_some() {
            "https://localhost".to_owned()
        } else if h2 {
            "http://provider.svc".to_owned()
        } else {
            format!("http://{}", self.address)
        };
        let mut transport = ResponseTransport::new(origin.parse().unwrap(), Duration::from_secs(2));
        transport.address = Some(self.address);
        if let Some(tls) = &self.tls {
            transport.tls = tls.clone();
        }
        transport
    }
}

fn certificates(h2: bool) -> (rustls::ServerConfig, rustls::ClientConfig) {
    let directory = std::env::temp_dir().join(format!("cmaeg-tls-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&directory).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(directory.clone());
    for arguments in [
        vec![
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-keyout",
            "key.pem",
            "-out",
            "cert.pem",
            "-days",
            "1",
            "-subj",
            "/CN=localhost",
            "-addext",
            "subjectAltName=DNS:localhost",
            "-addext",
            "basicConstraints=critical,CA:FALSE",
        ],
        vec![
            "x509", "-in", "cert.pem", "-outform", "DER", "-out", "cert.der",
        ],
        vec![
            "pkcs8", "-topk8", "-nocrypt", "-in", "key.pem", "-outform", "DER", "-out", "key.der",
        ],
    ] {
        let output = std::process::Command::new("openssl")
            .args(arguments)
            .current_dir(&directory)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "openssl certificate generation failed"
        );
    }
    let certificate = CertificateDer::from(std::fs::read(directory.join("cert.der")).unwrap());
    let key = PrivateKeyDer::try_from(std::fs::read(directory.join("key.der")).unwrap()).unwrap();
    let crypto = Arc::new(rustls::crypto::ring::default_provider());
    let mut server = rustls::ServerConfig::builder_with_provider(crypto.clone())
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![certificate.clone()], key)
        .unwrap();
    server.alpn_protocols = vec![if h2 {
        b"h2".to_vec()
    } else {
        b"http/1.1".to_vec()
    }];
    let mut roots = rustls::RootCertStore::empty();
    roots.add(certificate).unwrap();
    let mut client = rustls::ClientConfig::builder_with_provider(crypto)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
    client.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    (server, client)
}
