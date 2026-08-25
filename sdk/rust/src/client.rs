//! HTTP client, authentication, and retry policy.

use std::collections::HashSet;
use std::fmt;
use std::future::Future;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use base64::Engine;
use futures_util::StreamExt;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderName, HeaderValue, LINK, RETRY_AFTER};
use reqwest::{Method, Response, StatusCode, Url};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::{
    ApprovalDecision, Conformance, CreateSessionRequest, Error, EventStream, MAX_PAGE_SIZE, Page,
    PageContinuation, PageWalkLimits, Result, ScreenSnapshot, Session, SessionId, SessionInput,
    SessionPageRequest, TerminalGeometry, Usage,
};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const USER_AGENT: &str = concat!("cma-sdk/", env!("CARGO_PKG_VERSION"));
const X_REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");

type CredentialProvider = dyn Fn() -> std::result::Result<String, ()> + Send + Sync;
type RequestIdProvider = dyn Fn() -> std::result::Result<String, ()> + Send + Sync;
type ResponseObserver = dyn Fn(&ResponseMetadata) -> std::result::Result<(), String> + Send + Sync;

/// Correlation metadata available as soon as an HTTP response is received.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResponseMetadata {
    /// HTTP response status.
    pub status: u16,
    /// Visible, non-blank `X-Request-Id` response value, when usable.
    pub request_id: Option<String>,
}

/// Retry behavior for transient request and stream failures.
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    max_attempts: usize,
    initial_backoff: Duration,
    max_backoff: Duration,
    jitter_ratio: f64,
}

impl RetryPolicy {
    /// Build a retry policy.
    ///
    /// `max_attempts` includes the first request and must be non-zero. Backoff
    /// grows exponentially from `initial_backoff` and is capped by
    /// `max_backoff`.
    pub fn new(
        max_attempts: usize,
        initial_backoff: Duration,
        max_backoff: Duration,
    ) -> Result<Self> {
        if max_attempts == 0 {
            return Err(Error::protocol(
                "a retry policy requires at least one attempt",
            ));
        }
        if initial_backoff > max_backoff {
            return Err(Error::protocol(
                "retry initial_backoff cannot exceed max_backoff",
            ));
        }
        Ok(Self {
            max_attempts,
            initial_backoff,
            max_backoff,
            jitter_ratio: 0.2,
        })
    }

    /// Set the fractional random spread around exponential backoff.
    ///
    /// A value of `0.2` selects a delay between 80% and 120% of the computed
    /// backoff. The accepted range is zero through one.
    pub fn with_jitter(mut self, jitter_ratio: f64) -> Result<Self> {
        if !jitter_ratio.is_finite() || !(0.0..=1.0).contains(&jitter_ratio) {
            return Err(Error::protocol(
                "retry jitter must be a finite ratio from zero through one",
            ));
        }
        self.jitter_ratio = jitter_ratio;
        Ok(self)
    }

    /// Number of attempts, including the initial request.
    pub fn max_attempts(&self) -> usize {
        self.max_attempts
    }

    /// Initial exponential-backoff delay.
    pub fn initial_backoff(&self) -> Duration {
        self.initial_backoff
    }

    /// Maximum exponential-backoff delay.
    pub fn max_backoff(&self) -> Duration {
        self.max_backoff
    }

    /// Fractional jitter spread.
    pub fn jitter_ratio(&self) -> f64 {
        self.jitter_ratio
    }

    pub(crate) fn delay(&self, failed_attempt: usize) -> Duration {
        let exponent = failed_attempt.saturating_sub(1).min(31) as u32;
        let multiplier = 1_u32.checked_shl(exponent).unwrap_or(u32::MAX);
        let base = self
            .initial_backoff
            .saturating_mul(multiplier)
            .min(self.max_backoff);
        if self.jitter_ratio == 0.0 || base.is_zero() {
            return base;
        }
        let spread = (fastrand::f64() * 2.0 - 1.0) * self.jitter_ratio;
        Duration::from_secs_f64((base.as_secs_f64() * (1.0 + spread)).max(0.0))
            .min(self.max_backoff)
    }
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 4,
            initial_backoff: Duration::from_millis(150),
            max_backoff: Duration::from_secs(3),
            jitter_ratio: 0.2,
        }
    }
}

/// Builder for a secret-safe [`Client`].
pub struct ClientBuilder {
    base_url: Url,
    credential_provider: Option<Arc<CredentialProvider>>,
    request_id_provider: Option<Arc<RequestIdProvider>>,
    response_observer: Option<Arc<ResponseObserver>>,
    request_timeout: Duration,
    retry_policy: RetryPolicy,
}

impl ClientBuilder {
    /// Supply a rotation-aware bearer credential callback.
    ///
    /// The callback runs for every HTTP attempt and every event-stream
    /// reconnect. Its value is immediately placed in a sensitive header and is
    /// never retained as a printable field.
    pub fn credential_provider<F, E>(mut self, provider: F) -> Self
    where
        F: Fn() -> std::result::Result<String, E> + Send + Sync + 'static,
    {
        self.credential_provider = Some(Arc::new(move || provider().map_err(|_| ())));
        self
    }

    /// Supply a correlation-id callback for logical HTTP operations.
    ///
    /// The callback runs once before an operation's retry loop. Its exact
    /// non-blank, HTTP-header-safe value is sent as `X-Request-Id` on every
    /// attempt in that loop. Provider failure prevents the request from being
    /// sent.
    pub fn request_id_provider<F, E>(mut self, provider: F) -> Self
    where
        F: Fn() -> std::result::Result<String, E> + Send + Sync + 'static,
    {
        self.request_id_provider = Some(Arc::new(move || provider().map_err(|_| ())));
        self
    }

    /// Observe metadata for every completed HTTP response before retry or decoding.
    ///
    /// Returning an error or panicking produces [`Error::Observer`] and stops
    /// protocol handling for that response. The callback must not retain the
    /// borrowed metadata.
    pub fn response_observer<F, E>(mut self, observer: F) -> Self
    where
        F: Fn(&ResponseMetadata) -> std::result::Result<(), E> + Send + Sync + 'static,
        E: fmt::Display,
    {
        self.response_observer = Some(Arc::new(move |metadata| {
            observer(metadata).map_err(|error| error.to_string())
        }));
        self
    }

    /// Set the timeout applied to each individual HTTP request.
    pub fn request_timeout(mut self, timeout: Duration) -> Result<Self> {
        if timeout.is_zero() {
            return Err(Error::protocol("request timeout must be non-zero"));
        }
        self.request_timeout = timeout;
        Ok(self)
    }

    /// Replace the default retry policy.
    pub fn retry_policy(mut self, retry_policy: RetryPolicy) -> Self {
        self.retry_policy = retry_policy;
        self
    }

    /// Build the client and its rustls-backed HTTP transport.
    pub fn build(self) -> Result<Client> {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| {
                Error::transport(format!("could not build HTTP transport: {error}"))
            })?;
        Ok(Client {
            inner: Arc::new(ClientInner {
                http,
                base_url: self.base_url,
                credential_provider: self.credential_provider,
                request_id_provider: self.request_id_provider,
                response_observer: self.response_observer,
                request_timeout: self.request_timeout,
                retry_policy: self.retry_policy,
            }),
        })
    }
}

impl fmt::Debug for ClientBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClientBuilder")
            .field("base_url", &self.base_url)
            .field(
                "credential_provider",
                &self.credential_provider.as_ref().map(|_| "[redacted]"),
            )
            .field(
                "request_id_provider",
                &self.request_id_provider.as_ref().map(|_| "[configured]"),
            )
            .field(
                "response_observer",
                &self.response_observer.as_ref().map(|_| "[configured]"),
            )
            .field("request_timeout", &self.request_timeout)
            .field("retry_policy", &self.retry_policy)
            .finish()
    }
}

/// Async client for CMA's backend-facing REST interface.
#[derive(Clone)]
pub struct Client {
    inner: Arc<ClientInner>,
}

struct ClientInner {
    http: reqwest::Client,
    base_url: Url,
    credential_provider: Option<Arc<CredentialProvider>>,
    request_id_provider: Option<Arc<RequestIdProvider>>,
    response_observer: Option<Arc<ResponseObserver>>,
    request_timeout: Duration,
    retry_policy: RetryPolicy,
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client")
            .field("base_url", &self.inner.base_url)
            .field(
                "credential_provider",
                &self
                    .inner
                    .credential_provider
                    .as_ref()
                    .map(|_| "[redacted]"),
            )
            .field(
                "request_id_provider",
                &self
                    .inner
                    .request_id_provider
                    .as_ref()
                    .map(|_| "[configured]"),
            )
            .field(
                "response_observer",
                &self
                    .inner
                    .response_observer
                    .as_ref()
                    .map(|_| "[configured]"),
            )
            .field("request_timeout", &self.inner.request_timeout)
            .field("retry_policy", &self.inner.retry_policy)
            .finish()
    }
}

impl Client {
    /// Start configuring a client for a deployment base URL.
    ///
    /// The base URL may include a reverse-proxy path prefix. It must use HTTP
    /// or HTTPS and may not carry user information, a query, or a fragment.
    pub fn builder(base_url: impl AsRef<str>) -> Result<ClientBuilder> {
        let mut base_url = Url::parse(base_url.as_ref())
            .map_err(|error| Error::protocol(format!("invalid CMA base URL: {error}")))?;
        if !matches!(base_url.scheme(), "http" | "https") {
            return Err(Error::protocol("CMA base URL must use HTTP or HTTPS"));
        }
        if base_url.host_str().is_none() {
            return Err(Error::protocol("CMA base URL requires a host"));
        }
        if !base_url.username().is_empty() || base_url.password().is_some() {
            return Err(Error::protocol(
                "CMA base URL must not contain user information",
            ));
        }
        if base_url.query().is_some() || base_url.fragment().is_some() {
            return Err(Error::protocol(
                "CMA base URL must not contain a query or fragment",
            ));
        }
        if !base_url.path().ends_with('/') {
            let path = format!("{}/", base_url.path());
            base_url.set_path(&path);
        }
        Ok(ClientBuilder {
            base_url,
            credential_provider: None,
            request_id_provider: None,
            response_observer: None,
            request_timeout: DEFAULT_TIMEOUT,
            retry_policy: RetryPolicy::default(),
        })
    }

    /// Read this deployment's anonymous REST conformance document.
    pub async fn read_conformance(&self) -> Result<Conformance> {
        let request = RequestSpec::new(Method::GET, self.endpoint(&["conformance"])?, None, false)
            .anonymous();
        let conformance: Conformance = self
            .execute(request, |response| async move {
                decode_json(response, StatusCode::OK).await
            })
            .await?;
        conformance.validate()
    }

    /// Create a session.
    pub async fn create_session(&self, request: CreateSessionRequest) -> Result<Session> {
        #[derive(Serialize)]
        struct WireRequest<'a> {
            #[serde(skip_serializing_if = "Option::is_none")]
            cols: Option<u16>,
            #[serde(skip_serializing_if = "Option::is_none")]
            rows: Option<u16>,
            #[serde(skip_serializing_if = "Option::is_none")]
            environment_profile: Option<&'a str>,
            #[serde(skip_serializing_if = "Option::is_none")]
            quoted_rate_card: Option<&'a str>,
        }

        let geometry = request
            .geometry
            .map(|geometry| geometry.validate("create request"))
            .transpose()?;
        let body = serde_json::to_value(WireRequest {
            cols: geometry.map(|value| value.cols),
            rows: geometry.map(|value| value.rows),
            environment_profile: request.environment_profile.as_deref(),
            quoted_rate_card: request.quoted_rate_card.as_deref(),
        })
        .map_err(|error| Error::protocol(format!("could not encode create request: {error}")))?;
        let session: Session = self
            .request_json(Method::POST, &["sessions"], Some(body), StatusCode::CREATED)
            .await?;
        session.validate()
    }

    /// Read one bounded session page.
    pub async fn list_session_page(&self, page: SessionPageRequest) -> Result<Page<Session>> {
        let url = match page.continuation() {
            Some(continuation) => self.session_page_url(continuation.as_str())?,
            None => {
                let mut url = self.endpoint(&["sessions"])?;
                if page.status().is_some() || page.limit().is_some() || page.cursor().is_some() {
                    let mut query = url.query_pairs_mut();
                    if let Some(status) = page.status() {
                        query.append_pair("status", status.query_value());
                    }
                    if let Some(limit) = page.limit() {
                        query.append_pair("limit", &limit.to_string());
                    }
                    if let Some(cursor) = page.cursor() {
                        query.append_pair("cursor", cursor);
                    }
                }
                url
            }
        };
        let request = RequestSpec::new(Method::GET, url, None, false);
        let (items, next): (Vec<Session>, Option<String>) = self
            .execute(request, |response| async move {
                decode_json_page(response, StatusCode::OK).await
            })
            .await?;
        if items.len() > usize::from(MAX_PAGE_SIZE) {
            return Err(Error::protocol(format!(
                "session page exceeded the {MAX_PAGE_SIZE}-item server boundary"
            )));
        }
        let items = items
            .into_iter()
            .map(Session::validate)
            .collect::<Result<Vec<_>>>()?;
        let next = match next {
            Some(target) => {
                self.session_page_url(&target)?;
                Some(PageContinuation::new(target))
            }
            None => None,
        };
        Ok(Page { items, next })
    }

    /// List every session visible to the current caller.
    pub async fn list_sessions(&self) -> Result<Vec<Session>> {
        self.list_sessions_with_limits(PageWalkLimits::default())
            .await
    }

    /// List every visible session within explicit page and retained-item ceilings.
    pub async fn list_sessions_with_limits(&self, limits: PageWalkLimits) -> Result<Vec<Session>> {
        let mut request = SessionPageRequest::new();
        let mut visited = HashSet::new();
        let mut sessions = Vec::new();
        for page_number in 1..=limits.max_pages() {
            let page = self.list_session_page(request).await?;
            if page.items.len() > limits.max_items().saturating_sub(sessions.len()) {
                return Err(Error::protocol(format!(
                    "pagination exceeded its {}-item boundary",
                    limits.max_items()
                )));
            }
            sessions.extend(page.items);
            let Some(next) = page.next else {
                return Ok(sessions);
            };
            if page_number == limits.max_pages() {
                return Err(Error::protocol(format!(
                    "pagination exceeded its {}-page boundary",
                    limits.max_pages()
                )));
            }
            if !visited.insert(next.as_str().to_owned()) {
                return Err(Error::protocol("pagination repeated a continuation link"));
            }
            request = SessionPageRequest::continuing(next);
        }
        Err(Error::protocol("pagination exhausted its page boundary"))
    }

    /// Read one session.
    pub async fn read_session(&self, session_id: &SessionId) -> Result<Session> {
        let session: Session = self
            .request_json(
                Method::GET,
                &["sessions", session_id.as_str()],
                None,
                StatusCode::OK,
            )
            .await?;
        session.validate()
    }

    /// Deliver input on the session's granted plane.
    pub async fn send_input(&self, session_id: &SessionId, input: SessionInput) -> Result<()> {
        let body = match input {
            SessionInput::Text { text, newline } => {
                serde_json::json!({ "text": text, "newline": newline })
            }
            SessionInput::Bytes(bytes) => serde_json::json!({
                "bytes_b64": base64::engine::general_purpose::STANDARD.encode(bytes)
            }),
        };
        self.request_empty(
            Method::POST,
            &["sessions", session_id.as_str(), "input"],
            Some(body),
            StatusCode::ACCEPTED,
        )
        .await
    }

    /// Answer a pending approval request.
    pub async fn resolve_approval(
        &self,
        session_id: &SessionId,
        request_id: &str,
        decision: ApprovalDecision,
    ) -> Result<()> {
        validate_path_identifier(request_id, "approval request id")?;
        let body = serde_json::to_value(ApprovalBody { decision }).map_err(|error| {
            Error::protocol(format!("could not encode approval decision: {error}"))
        })?;
        self.request_empty(
            Method::POST,
            &["sessions", session_id.as_str(), "approvals", request_id],
            Some(body),
            StatusCode::ACCEPTED,
        )
        .await
    }

    /// Stop the in-flight turn without ending the session.
    pub async fn interrupt(&self, session_id: &SessionId) -> Result<()> {
        self.request_empty(
            Method::POST,
            &["sessions", session_id.as_str(), "interrupt"],
            None,
            StatusCode::ACCEPTED,
        )
        .await
    }

    /// Redirect a conversation turn already in progress.
    pub async fn steer(&self, session_id: &SessionId, text: impl Into<String>) -> Result<()> {
        self.request_empty(
            Method::POST,
            &["sessions", session_id.as_str(), "steer"],
            Some(serde_json::json!({ "text": text.into() })),
            StatusCode::ACCEPTED,
        )
        .await
    }

    /// Inform a terminal session that its dimensions changed.
    pub async fn resize_terminal(
        &self,
        session_id: &SessionId,
        geometry: TerminalGeometry,
    ) -> Result<()> {
        let geometry = geometry.validate("resize request")?;
        self.request_empty(
            Method::POST,
            &["sessions", session_id.as_str(), "resize"],
            Some(serde_json::json!({
                "cols": geometry.cols,
                "rows": geometry.rows,
            })),
            StatusCode::NO_CONTENT,
        )
        .await
    }

    /// Fetch the current rendered terminal screen.
    pub async fn screen_snapshot(&self, session_id: &SessionId) -> Result<ScreenSnapshot> {
        let snapshot: ScreenSnapshot = self
            .request_json(
                Method::GET,
                &["sessions", session_id.as_str(), "screen"],
                None,
                StatusCode::OK,
            )
            .await?;
        snapshot.validate()
    }

    /// Read cumulative token and context usage.
    pub async fn get_usage(&self, session_id: &SessionId) -> Result<Usage> {
        self.request_json(
            Method::GET,
            &["sessions", session_id.as_str(), "usage"],
            None,
            StatusCode::OK,
        )
        .await
    }

    /// Open a typed event stream after an optional sequence cursor.
    ///
    /// The stream reconnects after a transport disconnect and sends the last
    /// validated sequence through both REST's compatible query cursor and the
    /// standard `Last-Event-ID` header.
    pub async fn stream_events(
        &self,
        session_id: &SessionId,
        starting_after: Option<u64>,
    ) -> Result<EventStream> {
        EventStream::open(self.clone(), session_id.clone(), starting_after).await
    }

    async fn request_json<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &[&str],
        body: Option<serde_json::Value>,
        expected: StatusCode,
    ) -> Result<T> {
        let request = RequestSpec::new(method, self.endpoint(path)?, body, false);
        self.execute(request, move |response| async move {
            decode_json(response, expected).await
        })
        .await
    }

    async fn request_empty(
        &self,
        method: Method,
        path: &[&str],
        body: Option<serde_json::Value>,
        expected: StatusCode,
    ) -> Result<()> {
        let request = RequestSpec::new(method, self.endpoint(path)?, body, false);
        self.execute(request, move |response| async move {
            decode_empty(response, expected).await
        })
        .await
    }

    pub(crate) async fn connect_events(
        &self,
        session_id: &SessionId,
        cursor: u64,
    ) -> Result<Response> {
        let mut url = self.endpoint(&["sessions", session_id.as_str(), "events"])?;
        if cursor > 0 {
            url.query_pairs_mut()
                .append_pair("starting_after", &cursor.to_string());
        }
        let mut request = RequestSpec::new(Method::GET, url, None, true);
        if cursor > 0 {
            request.last_event_id = Some(cursor);
        }
        self.execute(request, |response| async move {
            let status = response.status();
            if !status.is_success() {
                return Err(decode_problem(response).await);
            }
            if status != StatusCode::OK {
                return Err(Error::protocol(format!(
                    "event stream returned unexpected success status {}",
                    status.as_u16()
                )));
            }
            let content_type = response
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default();
            if !content_type
                .split(';')
                .next()
                .is_some_and(|value| value.trim().eq_ignore_ascii_case("text/event-stream"))
            {
                return Err(Error::protocol(format!(
                    "event stream returned content type '{content_type}'"
                )));
            }
            Ok(response)
        })
        .await
    }

    pub(crate) fn retry_policy_ref(&self) -> &RetryPolicy {
        &self.inner.retry_policy
    }

    fn endpoint(&self, path: &[&str]) -> Result<Url> {
        let mut url = self.inner.base_url.clone();
        let mut segments = url
            .path_segments_mut()
            .map_err(|_| Error::protocol("CMA base URL cannot be used as a path base"))?;
        segments.pop_if_empty();
        segments.push("api").push("v1");
        for segment in path {
            segments.push(segment);
        }
        drop(segments);
        Ok(url)
    }

    fn session_page_url(&self, target: &str) -> Result<Url> {
        if !target.starts_with('/') || target.starts_with("//") {
            return Err(Error::protocol(
                "pagination next link left the control plane",
            ));
        }
        let parsed = Url::parse(&format!("http://pagination.invalid{target}"))
            .map_err(|_| Error::protocol("pagination next link was not a valid URI"))?;
        if parsed.path() != "/api/v1/sessions" || parsed.fragment().is_some() {
            return Err(Error::protocol(
                "pagination next link left the session collection",
            ));
        }
        let mut url = self.endpoint(&["sessions"])?;
        url.set_query(parsed.query());
        Ok(url)
    }

    async fn execute<T, F, Fut>(&self, mut request: RequestSpec, decode: F) -> Result<T>
    where
        F: Fn(Response) -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        request.request_id = self.request_id_header()?;
        let attempts = self.inner.retry_policy.max_attempts;
        for attempt in 1..=attempts {
            let response = match self.send_once(&request).await {
                Ok(response) => response,
                Err(error @ Error::Transport { .. }) if attempt < attempts => {
                    tokio::time::sleep(self.inner.retry_policy.delay(attempt)).await;
                    drop(error);
                    continue;
                }
                Err(error) => return Err(error),
            };

            self.observe_response(&response)?;

            if let Some(delay) = retry_after(&response)?
                && attempt < attempts
            {
                drop(response);
                tokio::time::sleep(delay).await;
                continue;
            }

            match decode(response).await {
                Err(error @ Error::Transport { .. }) if attempt < attempts => {
                    tokio::time::sleep(self.inner.retry_policy.delay(attempt)).await;
                    drop(error);
                }
                result => return result,
            }
        }
        Err(Error::transport("request exhausted its retry policy"))
    }

    async fn send_once(&self, request: &RequestSpec) -> Result<Response> {
        let authorization = if request.authenticate {
            self.authorization_header()?
        } else {
            None
        };
        let mut builder = self
            .inner
            .http
            .request(request.method.clone(), request.url.clone());
        if let Some(authorization) = authorization {
            builder = builder.header(AUTHORIZATION, authorization);
        }
        if let Some(request_id) = &request.request_id {
            builder = builder.header(X_REQUEST_ID, request_id.clone());
        }
        if let Some(cursor) = request.last_event_id {
            builder = builder.header("last-event-id", cursor.to_string());
        }
        if let Some(body) = &request.body {
            builder = builder.json(body);
        }
        if !request.streaming {
            builder = builder.timeout(self.inner.request_timeout);
            return builder
                .send()
                .await
                .map_err(|error| Error::transport(format!("HTTP request failed: {error}")));
        }

        tokio::time::timeout(self.inner.request_timeout, builder.send())
            .await
            .map_err(|_| Error::transport("event-stream handshake timed out"))?
            .map_err(|error| Error::transport(format!("event-stream handshake failed: {error}")))
    }

    fn authorization_header(&self) -> Result<Option<HeaderValue>> {
        let Some(provider) = &self.inner.credential_provider else {
            return Ok(None);
        };
        let credential = provider().map_err(|_| Error::transport("credential provider failed"))?;
        if credential.is_empty() {
            return Err(Error::protocol(
                "credential provider returned an empty bearer credential",
            ));
        }
        let mut value = HeaderValue::from_str(&format!("Bearer {credential}"))
            .map_err(|_| Error::protocol("credential is not valid in an HTTP bearer header"))?;
        value.set_sensitive(true);
        Ok(Some(value))
    }

    fn request_id_header(&self) -> Result<Option<HeaderValue>> {
        let Some(provider) = &self.inner.request_id_provider else {
            return Ok(None);
        };
        let request_id = provider().map_err(|_| Error::transport("request-id provider failed"))?;
        if request_id.trim().is_empty() {
            return Err(Error::protocol(
                "request-id provider returned an empty or blank value",
            ));
        }
        HeaderValue::from_str(&request_id)
            .map(Some)
            .map_err(|_| Error::protocol("request id is not valid in an HTTP header"))
    }

    fn observe_response(&self, response: &Response) -> Result<()> {
        let Some(observer) = &self.inner.response_observer else {
            return Ok(());
        };
        let metadata = ResponseMetadata {
            status: response.status().as_u16(),
            request_id: response_request_id(response),
        };
        match catch_unwind(AssertUnwindSafe(|| observer(&metadata))) {
            Ok(Ok(())) => Ok(()),
            Ok(Err(message)) if message.trim().is_empty() => {
                Err(Error::observer("response observer failed"))
            }
            Ok(Err(message)) => Err(Error::observer(format!(
                "response observer failed: {message}"
            ))),
            Err(_) => Err(Error::observer("response observer panicked")),
        }
    }
}

#[derive(Serialize)]
struct ApprovalBody {
    decision: ApprovalDecision,
}

#[derive(Clone)]
struct RequestSpec {
    method: Method,
    url: Url,
    body: Option<serde_json::Value>,
    authenticate: bool,
    streaming: bool,
    last_event_id: Option<u64>,
    request_id: Option<HeaderValue>,
}

impl RequestSpec {
    fn new(method: Method, url: Url, body: Option<serde_json::Value>, streaming: bool) -> Self {
        Self {
            method,
            url,
            body,
            authenticate: true,
            streaming,
            last_event_id: None,
            request_id: None,
        }
    }

    fn anonymous(mut self) -> Self {
        self.authenticate = false;
        self
    }
}

fn validate_path_identifier(value: &str, name: &str) -> Result<()> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    {
        return Err(Error::protocol(format!(
            "{name} must be a non-empty wire identifier"
        )));
    }
    Ok(())
}

async fn decode_json<T: DeserializeOwned>(response: Response, expected: StatusCode) -> Result<T> {
    let status = response.status();
    if !status.is_success() {
        return Err(decode_problem(response).await);
    }
    if status != expected {
        return Err(Error::protocol(format!(
            "operation expected status {} but received {}",
            expected.as_u16(),
            status.as_u16()
        )));
    }
    let bytes = read_bounded(response).await?;
    serde_json::from_slice(&bytes).map_err(|error| {
        Error::protocol(format!(
            "success response does not match its JSON wire shape: {error}"
        ))
    })
}

async fn decode_json_page<T: DeserializeOwned>(
    response: Response,
    expected: StatusCode,
) -> Result<(T, Option<String>)> {
    let status = response.status();
    if !status.is_success() {
        return Err(decode_problem(response).await);
    }
    if status != expected {
        return Err(Error::protocol(format!(
            "operation expected status {} but received {}",
            expected.as_u16(),
            status.as_u16()
        )));
    }
    let next = response_next_link(&response)?;
    let bytes = read_bounded(response).await?;
    let body = serde_json::from_slice(&bytes).map_err(|error| {
        Error::protocol(format!(
            "success response does not match its JSON wire shape: {error}"
        ))
    })?;
    Ok((body, next))
}

fn response_next_link(response: &Response) -> Result<Option<String>> {
    for value in response.headers().get_all(LINK) {
        let value = value
            .to_str()
            .map_err(|_| Error::protocol("pagination Link header was not visible ASCII"))?;
        for entry in value.split(',') {
            let Some(rest) = entry.trim().strip_prefix('<') else {
                if link_declares_next(entry)? {
                    return Err(Error::protocol("pagination next link was malformed"));
                }
                continue;
            };
            let Some((target, parameters)) = rest.split_once('>') else {
                if link_declares_next(rest)? {
                    return Err(Error::protocol("pagination next link was malformed"));
                }
                continue;
            };
            if link_declares_next(parameters)? {
                return Ok(Some(target.to_string()));
            }
        }
    }
    Ok(None)
}

fn link_declares_next(parameters: &str) -> Result<bool> {
    for parameter in parameters.split(';') {
        let Some((name, value)) = parameter.split_once('=') else {
            continue;
        };
        if !name.trim().eq_ignore_ascii_case("rel") {
            continue;
        }
        let value = value.trim();
        let relations = if let Some(quoted) = value.strip_prefix('"') {
            quoted.strip_suffix('"').ok_or_else(|| {
                Error::protocol("pagination Link header carried a malformed relation")
            })?
        } else {
            if value.is_empty() || value.contains('"') {
                return Err(Error::protocol(
                    "pagination Link header carried a malformed relation",
                ));
            }
            value
        };
        return Ok(relations
            .split_ascii_whitespace()
            .any(|relation| relation.eq_ignore_ascii_case("next")));
    }
    Ok(false)
}

async fn decode_empty(response: Response, expected: StatusCode) -> Result<()> {
    let status = response.status();
    if !status.is_success() {
        return Err(decode_problem(response).await);
    }
    if status != expected {
        return Err(Error::protocol(format!(
            "operation expected status {} but received {}",
            expected.as_u16(),
            status.as_u16()
        )));
    }
    let bytes = read_bounded(response).await?;
    if bytes.iter().any(|byte| !byte.is_ascii_whitespace()) {
        return Err(Error::protocol(format!(
            "status {} response unexpectedly carried a body",
            status.as_u16()
        )));
    }
    Ok(())
}

async fn decode_problem(response: Response) -> Error {
    let response_status = response.status().as_u16();
    let request_id = response_request_id(&response);
    let bytes = match read_bounded(response).await {
        Ok(bytes) => bytes,
        Err(error) => return error,
    };
    #[derive(serde::Deserialize)]
    struct Problem {
        status: u16,
        code: String,
        detail: String,
        #[serde(default)]
        capability: Option<String>,
    }
    let problem: Problem = match serde_json::from_slice(&bytes) {
        Ok(problem) => problem,
        Err(error) => {
            return Error::protocol(format!(
                "HTTP {response_status} did not carry a machine-readable problem document: {error}"
            ));
        }
    };
    if problem.status != response_status {
        return Error::protocol(format!(
            "problem status {} does not match HTTP status {response_status}",
            problem.status
        ));
    }
    if problem.code.trim().is_empty() || problem.detail.trim().is_empty() {
        return Error::protocol("problem document requires non-empty code and detail");
    }
    if problem.code == "capability_unsupported" {
        return match problem.capability.filter(|name| !name.trim().is_empty()) {
            Some(capability) => Error::CapabilityRefused {
                capability,
                request_id,
            },
            None => {
                Error::protocol("capability_unsupported problem omitted its typed capability field")
            }
        };
    }
    Error::Api {
        status: response_status,
        code: problem.code,
        message: problem.detail,
        request_id,
    }
}

fn response_request_id(response: &Response) -> Option<String> {
    let mut values = response.headers().get_all(&X_REQUEST_ID).iter();
    let value = values.next()?;
    if values.next().is_some() {
        return None;
    }
    let value = value.to_str().ok()?;
    (!value.trim().is_empty()).then(|| value.to_owned())
}

async fn read_bounded(response: Response) -> Result<Vec<u8>> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
    {
        return Err(Error::protocol(format!(
            "response exceeds the {MAX_RESPONSE_BYTES}-byte boundary"
        )));
    }
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk =
            chunk.map_err(|error| Error::transport(format!("response body failed: {error}")))?;
        if body.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
            return Err(Error::protocol(format!(
                "response exceeds the {MAX_RESPONSE_BYTES}-byte boundary"
            )));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn retry_after(response: &Response) -> Result<Option<Duration>> {
    if !matches!(
        response.status(),
        StatusCode::TOO_MANY_REQUESTS | StatusCode::SERVICE_UNAVAILABLE
    ) {
        return Ok(None);
    }
    let Some(value) = response.headers().get(RETRY_AFTER) else {
        return Ok(None);
    };
    let value = value
        .to_str()
        .map_err(|_| Error::protocol("Retry-After is not a visible ASCII header"))?;
    if let Ok(seconds) = value.parse::<u64>() {
        return Ok(Some(Duration::from_secs(seconds)));
    }
    let date = httpdate::parse_http_date(value)
        .map_err(|_| Error::protocol("Retry-After is neither delta-seconds nor an HTTP date"))?;
    Ok(Some(
        date.duration_since(SystemTime::now())
            .unwrap_or(Duration::ZERO),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_base_urls_are_protocol_errors() {
        for base in [
            "not a URL",
            "file:///tmp/cma",
            "https://user:pass@example.com",
            "https://example.com?token=no",
        ] {
            assert!(matches!(Client::builder(base), Err(Error::Protocol { .. })));
        }
    }

    #[test]
    fn retry_policy_validates_its_boundaries() {
        assert!(RetryPolicy::new(0, Duration::ZERO, Duration::ZERO).is_err());
        assert!(RetryPolicy::new(1, Duration::from_secs(2), Duration::from_secs(1)).is_err());
        let policy = RetryPolicy::new(3, Duration::from_millis(2), Duration::from_millis(8))
            .expect("valid policy")
            .with_jitter(0.0)
            .expect("zero jitter is valid");
        assert_eq!(policy.delay(1), Duration::from_millis(2));
        assert_eq!(policy.delay(2), Duration::from_millis(4));
        assert_eq!(policy.delay(8), Duration::from_millis(8));
        assert!(policy.clone().with_jitter(f64::NAN).is_err());
        assert!(policy.with_jitter(1.1).is_err());
    }

    #[test]
    fn debug_never_reveals_a_held_credential() {
        const SECRET: &str = "fixture-private-credential";
        let builder = Client::builder("https://example.com/prefix")
            .expect("valid URL")
            .credential_provider(|| Ok::<_, std::convert::Infallible>(SECRET.to_owned()));
        assert!(!format!("{builder:?}").contains(SECRET));
        let client = builder.build().expect("client builds");
        assert!(!format!("{client:?}").contains(SECRET));
    }

    #[test]
    fn terminal_geometry_and_ids_fail_before_http() {
        assert!(TerminalGeometry::new(0, 40).is_err());
        assert!(SessionId::new("wrong_1").is_err());
        assert!(SessionId::new("sesn_good").is_ok());
        assert!(validate_path_identifier("request/one", "request id").is_err());
    }
}
