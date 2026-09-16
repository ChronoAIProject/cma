use super::*;
use std::{collections::VecDeque, sync::Mutex, time::Duration};

pub(super) const PNG: &[u8] = b"\x89PNG\r\n\x1a\nfixture bytes for mandatory scanner";
pub(super) type Trace = Arc<Mutex<Vec<&'static str>>>;

pub(super) fn request() -> FetchRequest {
    FetchRequest {
        source: SourceBinding {
            owner: layer2_core::resource_access::ResourceOwner::Person {
                subject: layer2_core::SubjectId::new("owner"),
            },
            bridge: layer2_core::bridge::BridgeWorkPin {
                bridge_id: layer2_core::bridge::BridgeId("brg_fixture".into()),
                config_revision: 1,
            },
            bot_id: "bot".into(),
            platform: layer2_core::bridge::BridgePlatform::Lark,
            source_id: "signed-source".into(),
        },
        reference: layer2_core::bridge::ingress::InboxAttachment {
            content_type: "image".into(),
            url: "opaque-provider-handle".into(),
            platform_message_id: Some("platform-message".into()),
            file_key: None,
            image_key: Some("image-handle".into()),
            filename: None,
            mime_type: Some("image/png".into()),
            size_bytes: None,
        },
    }
}

pub(super) struct Authority {
    pub trace: Trace,
    pub request: FetchRequest,
    pub url: String,
    pub ttl: Duration,
    pub limit: usize,
    pub mime: Mime,
    pub fail_check: Option<usize>,
    pub authorize_delay: Duration,
    pub check_delay: Duration,
}

#[async_trait::async_trait]
impl ConnectorAuthority for Authority {
    async fn authorize(&self, _: &FetchRequest) -> Result<FetchGrant, Unavailable> {
        self.trace.lock().unwrap().push("authorize");
        tokio::time::sleep(self.authorize_delay).await;
        FetchGrant::issue(
            self.request.clone(),
            &ConnectorHosts::new(&["media.example"]).unwrap(),
            &self.url,
            self.mime,
            self.limit,
            Instant::now() + self.ttl,
        )
    }
    async fn revalidate(&self, _: &FetchGrant) -> Result<(), Unavailable> {
        let count = {
            let mut trace = self.trace.lock().unwrap();
            trace.push("revalidate");
            trace.iter().filter(|entry| **entry == "revalidate").count()
        };
        tokio::time::sleep(self.check_delay).await;
        if self.fail_check == Some(count) {
            Err(Unavailable::Authorization)
        } else {
            Ok(())
        }
    }
}

pub(super) struct Dns {
    pub trace: Trace,
    pub addresses: Vec<IpAddr>,
    pub delay: Duration,
}

#[async_trait::async_trait]
impl Resolver for Dns {
    async fn resolve(&self, host: &str) -> Result<Vec<IpAddr>, Unavailable> {
        assert_eq!(host, "media.example");
        self.trace.lock().unwrap().push("resolve");
        tokio::time::sleep(self.delay).await;
        Ok(self.addresses.clone())
    }
}

pub(super) struct Http {
    pub trace: Trace,
    pub status: u16,
    pub peer: SocketAddr,
    pub mime: Option<String>,
    pub encoding: Option<String>,
    pub length: Option<u64>,
    pub chunks: Vec<Result<Vec<u8>, Unavailable>>,
    pub body_delay: Duration,
    pub connect_delay: Duration,
    pub targets: Mutex<Vec<Vec<SocketAddr>>>,
    pub deadlines: Mutex<Vec<Instant>>,
    pub body_budgets: Arc<Mutex<Vec<usize>>>,
}

#[async_trait::async_trait]
impl Transport for Http {
    async fn get(&self, target: &PinnedTarget) -> Result<network::DownloadResponse, Unavailable> {
        self.trace.lock().unwrap().push("connect");
        assert_eq!(target.host, "media.example");
        assert_eq!(target.url.as_str(), "https://media.example/resource");
        self.targets.lock().unwrap().push(target.addresses.clone());
        self.deadlines.lock().unwrap().push(target.deadline);
        tokio::time::sleep(self.connect_delay).await;
        Ok(network::DownloadResponse {
            status: self.status,
            peer: self.peer,
            mime: self.mime.clone(),
            encoding: self.encoding.clone(),
            length: self.length,
            body: Box::new(Chunks {
                trace: self.trace.clone(),
                chunks: self.chunks.clone().into(),
                delay: self.body_delay,
                budgets: self.body_budgets.clone(),
            }),
        })
    }
}

struct Chunks {
    trace: Trace,
    chunks: VecDeque<Result<Vec<u8>, Unavailable>>,
    delay: Duration,
    budgets: Arc<Mutex<Vec<usize>>>,
}

#[async_trait::async_trait]
impl network::DownloadBody for Chunks {
    async fn next(&mut self, remaining: usize) -> Result<Option<Vec<u8>>, Unavailable> {
        self.trace.lock().unwrap().push("body");
        self.budgets.lock().unwrap().push(remaining);
        tokio::time::sleep(self.delay).await;
        let chunk = self.chunks.pop_front().transpose()?;
        if chunk.as_ref().is_some_and(|chunk| chunk.len() > remaining) {
            Err(Unavailable::Bounds)
        } else {
            Ok(chunk)
        }
    }
}

impl Drop for Chunks {
    fn drop(&mut self) {
        self.trace.lock().unwrap().push("body_dropped");
    }
}

pub(super) struct Scan {
    pub trace: Trace,
    pub outcome: ScanOutcome,
    pub delay: Duration,
}

#[async_trait::async_trait]
impl Scanner for Scan {
    async fn scan(&self, bytes: &[u8], mime: Mime) -> ScanOutcome {
        assert_eq!(bytes, PNG);
        assert_eq!(mime, Mime::Png);
        self.trace.lock().unwrap().push("scan");
        tokio::time::sleep(self.delay).await;
        self.outcome
    }
}

pub(super) struct Fixture {
    pub trace: Trace,
    pub authority: Authority,
    pub dns: Dns,
    pub http: Http,
    pub scanner: Scan,
}

impl Fixture {
    pub fn new() -> Self {
        let trace = Arc::new(Mutex::new(Vec::new()));
        Self {
            authority: Authority {
                trace: trace.clone(),
                request: request(),
                url: "https://media.example/resource".into(),
                ttl: Duration::from_secs(10),
                limit: 128,
                mime: Mime::Png,
                fail_check: None,
                authorize_delay: Duration::ZERO,
                check_delay: Duration::ZERO,
            },
            dns: Dns {
                trace: trace.clone(),
                addresses: vec!["8.8.8.8".parse().unwrap()],
                delay: Duration::ZERO,
            },
            http: Http {
                trace: trace.clone(),
                status: 200,
                peer: "8.8.8.8:443".parse().unwrap(),
                mime: Some("image/png".into()),
                encoding: None,
                length: Some(PNG.len() as u64),
                chunks: vec![Ok(PNG[..12].to_vec()), Ok(PNG[12..].to_vec())],
                body_delay: Duration::ZERO,
                connect_delay: Duration::ZERO,
                targets: Mutex::new(Vec::new()),
                deadlines: Mutex::new(Vec::new()),
                body_budgets: Arc::new(Mutex::new(Vec::new())),
            },
            scanner: Scan {
                trace: trace.clone(),
                outcome: ScanOutcome::Clean,
                delay: Duration::ZERO,
            },
            trace,
        }
    }
    pub async fn run(&self) -> Result<FetchedAttachment, Unavailable> {
        retrieve(
            &request(),
            &self.authority,
            &self.dns,
            &self.http,
            &self.scanner,
        )
        .await
    }
    pub fn events(&self) -> Vec<&'static str> {
        self.trace.lock().unwrap().clone()
    }
}
