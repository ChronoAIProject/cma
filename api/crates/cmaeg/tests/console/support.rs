//! A console served on a loopback listener next to readiness and health,
//! wired to a fake NyxID.
use super::fake_nyxid::{console as oauth, FakeNyxid};
use axum::{routing::get, Json, Router};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use cmaeg::{
    auth::CreatorAuthenticator,
    console::{
        clock::Clock,
        directory::{ChannelDirectory, NoChannels},
        source_changes::{PendingSourceChanges, SourceChanges},
        token::TokenSet,
        Console, ConsoleDeps,
    },
    http::readiness::{readyz, Readiness},
    ConsoleConfig,
};
use cmaeg_core::{channel::Secret, seal::MasterKeyRing};
use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio::sync::oneshot;

/// Log output captured in memory, for scans of what a run logged.
#[derive(Clone, Default)]
pub struct Captured(pub Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for Captured {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Captured {
    type Writer = Captured;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

impl Captured {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

/// A clock tests move by hand.
pub struct ManualClock {
    base: Instant,
    offset_ms: AtomicU64,
}

impl ManualClock {
    pub fn advance(&self, by: Duration) {
        self.offset_ms
            .fetch_add(by.as_millis() as u64, Ordering::SeqCst);
    }
}

impl Clock for ManualClock {
    fn now(&self) -> Instant {
        self.base + Duration::from_millis(self.offset_ms.load(Ordering::SeqCst))
    }
}

/// Render the process metrics, installing the recorder on first use.
pub fn metrics() -> String {
    cmaeg::telemetry::install().unwrap().render()
}

/// The value of one sample, `name{labels}` exactly as rendered.
pub fn sample(rendered: &str, series: &str) -> f64 {
    rendered
        .lines()
        .find_map(|line| line.strip_prefix(&format!("{series} ")))
        .map_or(0.0, |value| value.trim().parse().unwrap())
}

pub struct Harness {
    pub base: String,
    pub nyxid: FakeNyxid,
    pub console: Console,
    pub clock: Arc<ManualClock>,
    pub keys: Arc<MasterKeyRing>,
    /// The authenticator REST shares; its per-token cache is the console's
    /// only authority cache.
    pub creator_auth: Arc<CreatorAuthenticator>,
    pub http: reqwest::Client,
    shutdown: Option<oneshot::Sender<()>>,
}

/// A complete client configuration with the console switched on.
pub fn config() -> ConsoleConfig {
    ConsoleConfig {
        enabled: true,
        client_id: oauth::CLIENT_ID.into(),
        client_secret: Some(Secret::new(oauth::CLIENT_SECRET)),
        ..ConsoleConfig::default()
    }
}

/// What the console reads channels from and changes sources through.
pub struct Composition {
    pub directory: Arc<dyn ChannelDirectory>,
    pub source_changes: Arc<dyn SourceChanges>,
}

impl Default for Composition {
    /// No channel memory, and no source changes: the sign-in tests' console.
    fn default() -> Self {
        Self {
            directory: Arc::new(NoChannels),
            source_changes: Arc::new(PendingSourceChanges),
        }
    }
}

impl Harness {
    /// Serve a console built from `config`, without discovering yet.
    pub async fn start(config: ConsoleConfig) -> Self {
        Self::compose(config, Composition::default()).await
    }

    /// Serve an enabled console over `composition`.
    pub async fn enabled_with(composition: Composition) -> Self {
        let harness = Self::compose(config(), composition).await;
        harness.console.discover_once().await.unwrap();
        assert!(harness.console.is_enabled());
        harness
    }

    /// Serve a console built from `config` over `composition`, without
    /// discovering yet.
    pub async fn compose(config: ConsoleConfig, composition: Composition) -> Self {
        // Before the console exists, so every console metric is recorded.
        cmaeg::telemetry::install().unwrap();
        let nyxid = FakeNyxid::start().await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let clock = Arc::new(ManualClock {
            base: Instant::now(),
            offset_ms: AtomicU64::new(0),
        });
        let keys =
            Arc::new(MasterKeyRing::parse(&format!("1:{}", STANDARD.encode([5; 32]))).unwrap());
        let creator_auth = Arc::new(CreatorAuthenticator::new(&nyxid.base_url).unwrap());
        let console = Console::new(ConsoleDeps {
            config,
            public_url: base.clone(),
            public_api_url: base.clone(),
            nyxid_base_url: nyxid.base_url.clone(),
            clock: clock.clone(),
            keys: keys.clone(),
            creator_auth: creator_auth.clone(),
            directory: composition.directory,
            source_changes: composition.source_changes,
            multi_consumer: false,
        })
        .unwrap();
        let router = Router::new()
            .route("/readyz", get(readyz).with_state(Readiness::initialized()))
            .route(
                "/healthz",
                get(|| async { Json(serde_json::json!({"status": "ok"})) }),
            )
            .merge(console.router());
        let (shutdown, signal) = oneshot::channel();
        tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    let _ = signal.await;
                })
                .await
                .unwrap();
        });
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        Self {
            base,
            nyxid,
            console,
            clock,
            keys,
            creator_auth,
            http,
            shutdown: Some(shutdown),
        }
    }

    /// Serve an enabled console: complete client and a successful discovery.
    pub async fn enabled() -> Self {
        let harness = Self::start(config()).await;
        harness.console.discover_once().await.unwrap();
        assert!(harness.console.is_enabled());
        harness
    }

    /// Tokens the fake NyxID issued for `subject`, as a sign-in would hold.
    pub fn tokens(&self, subject: &str) -> TokenSet {
        let (access, refresh) = self.nyxid.oauth.issue_tokens(subject);
        TokenSet {
            access: Secret::new(access),
            refresh: Secret::new(refresh),
            expires_in: Duration::from_secs(900),
        }
    }

    /// Move time by `by` for both clocks the console reads: its own session
    /// clock and tokio's, which times REST's creator cache. Call it only
    /// while no request is in flight; tokio's clock runs on in real time
    /// afterwards.
    pub async fn advance(&self, by: Duration) {
        self.clock.advance(by);
        tokio::time::pause();
        tokio::time::advance(by).await;
        tokio::time::resume();
    }

    pub async fn get(&self, path: &str) -> reqwest::Response {
        self.http
            .get(format!("{}{path}", self.base))
            .send()
            .await
            .unwrap()
    }

    /// The full sign-in as a browser makes it: the explicit link, NyxID's
    /// approval for `subject`, and the callback. Returns the session cookie
    /// value and where the callback sent the browser.
    pub async fn sign_in(&self, subject: &str, return_to: &str) -> (String, String) {
        *self.nyxid.oauth.login_subject.lock().unwrap() = subject.into();
        let (authorize, login_cookie) = self.start_login(return_to).await;
        let callback = self.approve(&authorize).await;
        let response = self.callback(&callback, Some(&login_cookie)).await;
        assert_eq!(response.status(), 303, "{subject} sign-in");
        let location = response.headers()["location"].to_str().unwrap().to_owned();
        let sid = set_cookie(&response, "__Host-cmaeg-console").expect("a session cookie");
        (sid, location)
    }

    /// `GET /console/login`: NyxID's authorization URL and the login cookie.
    pub async fn start_login(&self, return_to: &str) -> (String, String) {
        let encoded: String = url::form_urlencoded::byte_serialize(return_to.as_bytes()).collect();
        let response = self
            .get(&format!("/console/login?return_to={encoded}"))
            .await;
        assert_eq!(response.status(), 302);
        let authorize = response.headers()["location"].to_str().unwrap().to_owned();
        let cookie = set_cookie(&response, "__Host-cmaeg-console-login").expect("a login cookie");
        (authorize, cookie)
    }

    /// Follow NyxID's authorization URL; returns the callback URL it sends
    /// the browser to.
    pub async fn approve(&self, authorize: &str) -> String {
        let response = self.http.get(authorize).send().await.unwrap();
        assert_eq!(response.status(), 303);
        response.headers()["location"].to_str().unwrap().to_owned()
    }

    pub async fn callback(&self, url: &str, login_cookie: Option<&str>) -> reqwest::Response {
        let mut request = self.http.get(url);
        if let Some(cookie) = login_cookie {
            request = request.header("cookie", format!("__Host-cmaeg-console-login={cookie}"));
        }
        request.send().await.unwrap()
    }

    /// GET with the session cookie `sid`.
    pub async fn get_as(&self, path: &str, sid: &str) -> reqwest::Response {
        self.http
            .get(format!("{}{path}", self.base))
            .header("cookie", format!("__Host-cmaeg-console={sid}"))
            .send()
            .await
            .unwrap()
    }

    /// POST a form from `origin` (the console's own when `None`), with the
    /// session cookie when `sid` is given.
    pub async fn post(
        &self,
        path: &str,
        sid: Option<&str>,
        origin: Option<&str>,
        fields: &[(&str, &str)],
    ) -> reqwest::Response {
        let mut request = self
            .http
            .post(format!("{}{path}", self.base))
            .header("origin", origin.unwrap_or(&self.base))
            .form(fields);
        if let Some(sid) = sid {
            request = request.header("cookie", format!("__Host-cmaeg-console={sid}"));
        }
        request.send().await.unwrap()
    }

    /// Ask the owner switch for `owner`, with the CSRF value of the form the
    /// signed-in page shows.
    pub async fn switch_owner(&self, sid: &str, owner: &str) -> reqwest::Response {
        let page = self.get_as("/console", sid).await.text().await.unwrap();
        let csrf = csrf_for(&page, "/console/owner");
        self.post(
            "/console/owner",
            Some(sid),
            None,
            &[("csrf", &csrf), ("owner", owner)],
        )
        .await
    }

    /// A session for `subject` created directly in the store; its cookie value.
    pub fn session(&self, subject: &str) -> String {
        let (sid, _) = self
            .console
            .sessions()
            .create(subject.into(), self.tokens(subject))
            .unwrap();
        sid.as_str().to_owned()
    }

    /// Close a harness shared with spawned requests.
    pub async fn close_shared(self: Arc<Self>) {
        if let Ok(harness) = Arc::try_unwrap(self) {
            harness.close().await;
        }
    }

    pub async fn close(mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        self.nyxid.close().await;
    }
}

/// The CSRF value the console mints for the route template `action` on
/// `record` with a session's `key`: base64url(HMAC-SHA256(key, action ␟
/// record)). Tests use it for forms no fixture page shows.
pub fn csrf_token(key: &[u8; 32], action: &str, record: &str) -> String {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, key);
    let tag = ring::hmac::sign(&key, format!("{action}\u{1f}{record}").as_bytes());
    URL_SAFE_NO_PAD.encode(tag.as_ref())
}

/// The CSRF value of the form posting to `action` in `page`.
pub fn csrf_for(page: &str, action: &str) -> String {
    let form = format!("action=\"{action}\"><input type=\"hidden\" name=\"csrf\" value=\"");
    let start = page
        .find(&form)
        .unwrap_or_else(|| panic!("no {action} form in {page}"))
        + form.len();
    page[start..].split('"').next().unwrap().to_owned()
}

/// The value a response sets for cookie `name`, if any.
pub fn set_cookie(response: &reqwest::Response, name: &str) -> Option<String> {
    response
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find_map(|value| value.strip_prefix(&format!("{name}=")))
        .map(|rest| rest.split(';').next().unwrap().to_owned())
        .filter(|value| !value.is_empty())
}
