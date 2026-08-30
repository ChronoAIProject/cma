//! HTTP adapter for the provider-side sponsored Responses protocol.
//!
//! Protocol version 1 treats `confirmed` as a durable, linearizable terminal
//! one-effect result for the immutable operation ID. Before returning it, the
//! provider must ensure every older in-flight and later execution converges
//! without another effect, and every later execute or observe returns the exact
//! same response and actual quota. It treats `rejected` as a provider-enforced,
//! linearizable tombstone. Before returning `rejected`, the provider must fence
//! every execution with that ID, including invocations already in flight, and
//! guarantee that none has produced or can later produce an effect. Every later
//! execute must remain effect-free. CMA trusts these wire promises when settling
//! or releasing reserved sponsorship capacity.
//! `not_found` is not a tombstone, and transport-level pre-dispatch failure
//! proves only that the particular HTTP call sent no provider bytes.

use std::fmt;
use std::time::Duration;

use async_trait::async_trait;
use futures_util::StreamExt as _;
use layer2_core::{
    SponsoredModelProvider, SponsoredModelProviderAuthority, SponsoredModelProviderExecution,
    SponsoredModelProviderObservation, SponsoredModelProviderRequest,
    SponsoredModelProviderResponse, SponsoredQuota, MAX_SPONSORED_MODEL_REQUEST_BYTES,
    MAX_SPONSORED_MODEL_RESPONSE_BYTES,
};
use reqwest::header::{
    HeaderMap, HeaderValue, ACCEPT, ACCEPT_ENCODING, AUTHORIZATION, CONTENT_ENCODING, CONTENT_TYPE,
};
use reqwest::{StatusCode, Url};
use serde::{Deserialize, Serialize};

const PROTOCOL_VERSION: u32 = 1;
const EXECUTE_PATH: &str = "/v1/sponsorship/model/responses:execute";
const OBSERVE_PATH: &str = "/v1/sponsorship/model/responses:observe";
const EXECUTE_KIND: &str = "sponsored_model.execute";
const OBSERVE_KIND: &str = "sponsored_model.observe";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const ENVELOPE_ALLOWANCE_BYTES: usize = 64 * 1024;
const MAX_ADAPTER_ID_BYTES: usize = 128;
const MAX_ORIGIN_BYTES: usize = 2_048;

/// Provider adapter whose only mutating operation is one idempotent Responses call.
#[derive(Clone)]
pub(crate) struct HttpSponsoredModelProvider {
    adapter_id: String,
    origin: String,
    client: reqwest::Client,
    maximum_request_body: usize,
    maximum_response_body: usize,
}

impl fmt::Debug for HttpSponsoredModelProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HttpSponsoredModelProvider")
            .field("adapter_id", &self.adapter_id)
            .field("origin", &self.origin)
            .field("maximum_request_body", &self.maximum_request_body)
            .field("maximum_response_body", &self.maximum_response_body)
            .finish_non_exhaustive()
    }
}

impl HttpSponsoredModelProvider {
    pub(crate) fn new(adapter_id: String, origin: String) -> Result<Self, String> {
        Self::build(
            adapter_id,
            origin,
            false,
            REQUEST_TIMEOUT,
            MAX_SPONSORED_MODEL_REQUEST_BYTES + ENVELOPE_ALLOWANCE_BYTES,
            MAX_SPONSORED_MODEL_RESPONSE_BYTES + ENVELOPE_ALLOWANCE_BYTES,
        )
    }

    fn build(
        adapter_id: String,
        origin: String,
        allow_loopback_http: bool,
        request_timeout: Duration,
        maximum_request_body: usize,
        maximum_response_body: usize,
    ) -> Result<Self, String> {
        validate_adapter_id(&adapter_id)?;
        let origin = validate_origin(&origin, allow_loopback_http)?;
        if request_timeout.is_zero() || maximum_request_body == 0 || maximum_response_body == 0 {
            return Err("sponsored model provider bounds are invalid".into());
        }
        let client = reqwest::Client::builder()
            .https_only(!allow_loopback_http)
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(CONNECT_TIMEOUT.min(request_timeout))
            .timeout(request_timeout)
            .build()
            .map_err(|_| "sponsored model provider client could not be constructed".to_string())?;
        Ok(Self {
            adapter_id,
            origin,
            client,
            maximum_request_body,
            maximum_response_body,
        })
    }

    #[cfg(test)]
    fn fixture(origin: String, maximum_response_body: usize) -> Self {
        Self::build(
            "provider-adapter-v1".into(),
            origin,
            true,
            Duration::from_secs(2),
            MAX_SPONSORED_MODEL_REQUEST_BYTES + ENVELOPE_ALLOWANCE_BYTES,
            maximum_response_body,
        )
        .unwrap()
    }

    async fn call(
        &self,
        path: &'static str,
        kind: &'static str,
        provider: &str,
        authority: &SponsoredModelProviderAuthority,
        request: &SponsoredModelProviderRequest,
    ) -> Result<ProviderHttpResponse, CallFailure> {
        let provider_grant_id = authority
            .grant_receipt
            .provider_grant_id
            .expose_for_provider_or_encryption();
        let revocation_handle = authority
            .grant_receipt
            .revocation_handle
            .expose_for_provider_or_encryption();
        let envelope = ProviderRequestEnvelope {
            version: PROTOCOL_VERSION,
            adapter_id: &self.adapter_id,
            kind,
            operation_id: &request.operation_id.0,
            provider,
            grant: ProviderGrantReference {
                provider_grant_id,
                provider_state_version: authority.grant_receipt.provider_state_version,
            },
            request_digest: &request.request_digest,
            maximum_quota: WireQuota::from(request.maximum_quota),
            model: &request.model,
            resource: &request.resource,
            request: &request.request,
        };
        let body = serde_json::to_vec(&envelope).map_err(|_| CallFailure::BeforeDispatch)?;
        if body.len() > self.maximum_request_body {
            return Err(CallFailure::BeforeDispatch);
        }

        let authorization =
            HeaderValue::from_str(&format!("Bearer {}", authority.root_credential.expose()))
                .map_err(|_| CallFailure::BeforeDispatch)?;
        let operation_id = HeaderValue::from_str(&request.operation_id.0)
            .map_err(|_| CallFailure::BeforeDispatch)?;
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, authorization);
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("identity"));
        headers.insert("idempotency-key", operation_id.clone());
        headers.insert("x-cma-operation-id", operation_id);

        let outbound = self
            .client
            .post(format!("{}{path}", self.origin))
            .headers(headers)
            .body(body)
            .build()
            .map_err(|_| CallFailure::BeforeDispatch)?;
        let response = self.client.execute(outbound).await.map_err(|error| {
            if error.is_builder() || error.is_connect() {
                CallFailure::BeforeDispatch
            } else {
                CallFailure::AfterDispatch
            }
        })?;
        let protected_values = [
            authority.root_credential.expose().as_bytes(),
            provider_grant_id.as_bytes(),
            revocation_handle.as_bytes(),
        ];
        if response.headers().iter().any(|(name, value)| {
            protected_values.iter().any(|protected| {
                contains_bytes(name.as_str().as_bytes(), protected)
                    || contains_bytes(value.as_bytes(), protected)
            })
        }) {
            return Err(CallFailure::AfterDispatch);
        }
        let status = response.status();
        let exact_json = response.headers().get_all(CONTENT_TYPE).iter().count() == 1
            && response
                .headers()
                .get(CONTENT_TYPE)
                .is_some_and(|value| value.as_bytes() == b"application/json")
            && !response.headers().contains_key(CONTENT_ENCODING);
        let mut body = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| CallFailure::AfterDispatch)?;
            let size = body
                .len()
                .checked_add(chunk.len())
                .ok_or(CallFailure::AfterDispatch)?;
            if size > self.maximum_response_body {
                return Err(CallFailure::AfterDispatch);
            }
            body.extend_from_slice(&chunk);
        }
        if protected_values
            .iter()
            .any(|protected| contains_bytes(&body, protected))
        {
            return Err(CallFailure::AfterDispatch);
        }
        Ok(ProviderHttpResponse {
            status,
            exact_json,
            body,
        })
    }

    fn decode(
        &self,
        response: ProviderHttpResponse,
        kind: &'static str,
        request: &SponsoredModelProviderRequest,
    ) -> Result<DecodedOutcome, ()> {
        if !response.exact_json {
            return Err(());
        }
        let envelope: ProviderResponseEnvelope =
            serde_json::from_slice(&response.body).map_err(|_| ())?;
        if envelope.version != PROTOCOL_VERSION
            || envelope.adapter_id != self.adapter_id
            || envelope.kind != kind
            || envelope.operation_id != request.operation_id.0
            || envelope.request_digest != request.request_digest
        {
            return Err(());
        }
        match (response.status, envelope.status, envelope.result) {
            (StatusCode::OK, ProviderStatus::Confirmed, Some(result))
                if result.response.is_object()
                    && result.actual_quota.requests == 1
                    && SponsoredQuota::from(result.actual_quota)
                        .fits_within(request.maximum_quota) =>
            {
                Ok(DecodedOutcome::Confirmed(SponsoredModelProviderResponse {
                    response: result.response,
                    actual_quota: result.actual_quota.into(),
                }))
            }
            (status, ProviderStatus::Rejected, None)
                if status.is_client_error()
                    && status != StatusCode::NOT_FOUND
                    && status != StatusCode::TOO_MANY_REQUESTS =>
            {
                Ok(DecodedOutcome::Rejected)
            }
            (StatusCode::ACCEPTED, ProviderStatus::Pending, None) => Ok(DecodedOutcome::Pending),
            (StatusCode::NOT_FOUND, ProviderStatus::NotFound, None) => Ok(DecodedOutcome::NotFound),
            _ => Err(()),
        }
    }
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack.len() >= needle.len()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

#[async_trait]
impl SponsoredModelProvider for HttpSponsoredModelProvider {
    async fn execute(
        &self,
        provider: &str,
        authority: &SponsoredModelProviderAuthority,
        request: &SponsoredModelProviderRequest,
    ) -> SponsoredModelProviderExecution {
        let response = match self
            .call(EXECUTE_PATH, EXECUTE_KIND, provider, authority, request)
            .await
        {
            Ok(response) => response,
            Err(CallFailure::BeforeDispatch) => {
                return SponsoredModelProviderExecution::UnavailableBeforeDispatch
            }
            Err(CallFailure::AfterDispatch) => return SponsoredModelProviderExecution::Ambiguous,
        };
        match self.decode(response, EXECUTE_KIND, request) {
            Ok(DecodedOutcome::Confirmed(response)) => {
                SponsoredModelProviderExecution::Confirmed(response)
            }
            Ok(DecodedOutcome::Rejected) => SponsoredModelProviderExecution::Rejected,
            Ok(DecodedOutcome::Pending | DecodedOutcome::NotFound) | Err(()) => {
                SponsoredModelProviderExecution::Ambiguous
            }
        }
    }

    async fn observe(
        &self,
        provider: &str,
        authority: &SponsoredModelProviderAuthority,
        request: &SponsoredModelProviderRequest,
    ) -> SponsoredModelProviderObservation {
        let response = match self
            .call(OBSERVE_PATH, OBSERVE_KIND, provider, authority, request)
            .await
        {
            Ok(response) => response,
            Err(CallFailure::BeforeDispatch | CallFailure::AfterDispatch) => {
                return SponsoredModelProviderObservation::Pending
            }
        };
        match self.decode(response, OBSERVE_KIND, request) {
            Ok(DecodedOutcome::Confirmed(response)) => {
                SponsoredModelProviderObservation::Confirmed(response)
            }
            Ok(DecodedOutcome::Rejected) => SponsoredModelProviderObservation::Rejected,
            Ok(DecodedOutcome::Pending) | Err(()) => SponsoredModelProviderObservation::Pending,
            Ok(DecodedOutcome::NotFound) => SponsoredModelProviderObservation::NotFound,
        }
    }
}

fn validate_adapter_id(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > MAX_ADAPTER_ID_BYTES
        || !value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':' | b'/')
        })
    {
        return Err("sponsored model provider adapter ID is invalid".into());
    }
    Ok(())
}

fn validate_origin(raw: &str, allow_loopback_http: bool) -> Result<String, String> {
    let parsed =
        Url::parse(raw).map_err(|_| "sponsored model provider origin must be canonical")?;
    let canonical = parsed.origin().ascii_serialization();
    let loopback_http = allow_loopback_http
        && parsed.scheme() == "http"
        && parsed.host_str().is_some_and(|host| {
            host == "localhost"
                || host
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|address| address.is_loopback())
        });
    if raw.len() > MAX_ORIGIN_BYTES
        || (parsed.scheme() != "https" && !loopback_http)
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.path() != "/"
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || raw != canonical
    {
        return Err("sponsored model provider origin must be one canonical HTTPS origin".into());
    }
    Ok(canonical)
}

#[derive(Debug)]
enum CallFailure {
    BeforeDispatch,
    AfterDispatch,
}

struct ProviderHttpResponse {
    status: StatusCode,
    exact_json: bool,
    body: Vec<u8>,
}

enum DecodedOutcome {
    Confirmed(SponsoredModelProviderResponse),
    Rejected,
    Pending,
    NotFound,
}

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
struct ProviderRequestEnvelope<'a> {
    version: u32,
    adapter_id: &'a str,
    kind: &'static str,
    operation_id: &'a str,
    provider: &'a str,
    grant: ProviderGrantReference<'a>,
    request_digest: &'a str,
    maximum_quota: WireQuota,
    model: &'a str,
    resource: &'a str,
    request: &'a serde_json::Value,
}

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
struct ProviderGrantReference<'a> {
    provider_grant_id: &'a str,
    provider_state_version: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireQuota {
    requests: u64,
    input_tokens: u64,
    output_tokens: u64,
}

impl From<SponsoredQuota> for WireQuota {
    fn from(value: SponsoredQuota) -> Self {
        Self {
            requests: value.requests,
            input_tokens: value.input_tokens,
            output_tokens: value.output_tokens,
        }
    }
}

impl From<WireQuota> for SponsoredQuota {
    fn from(value: WireQuota) -> Self {
        Self {
            requests: value.requests,
            input_tokens: value.input_tokens,
            output_tokens: value.output_tokens,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProviderResponseEnvelope {
    version: u32,
    adapter_id: String,
    kind: String,
    operation_id: String,
    request_digest: String,
    status: ProviderStatus,
    #[serde(default)]
    result: Option<ProviderResult>,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum ProviderStatus {
    Confirmed,
    Rejected,
    Pending,
    NotFound,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProviderResult {
    response: serde_json::Value,
    actual_quota: WireQuota,
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use axum::extract::State;
    use axum::http::{header, HeaderMap};
    use axum::response::{IntoResponse, Response};
    use axum::routing::post;
    use axum::{Json, Router};
    use layer2_core::{
        ProviderCredentialSecret, ProviderGrantReceipt, SensitiveProviderValue,
        SponsoredUsageReservationId,
    };

    use super::*;

    #[derive(Default)]
    struct Capture {
        authorization: Mutex<Option<String>>,
        idempotency_key: Mutex<Option<String>>,
        body: Mutex<Option<serde_json::Value>>,
    }

    async fn confirmed(
        State(capture): State<Arc<Capture>>,
        headers: HeaderMap,
        Json(body): Json<serde_json::Value>,
    ) -> Response {
        *capture.authorization.lock().unwrap() = headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        *capture.idempotency_key.lock().unwrap() = headers
            .get("idempotency-key")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        *capture.body.lock().unwrap() = Some(body.clone());
        Json(serde_json::json!({
            "version": PROTOCOL_VERSION,
            "adapter_id": "provider-adapter-v1",
            "kind": EXECUTE_KIND,
            "operation_id": body["operation_id"],
            "request_digest": body["request_digest"],
            "status": "confirmed",
            "result": {
                "response": {"id": "provider-response-1", "object": "response"},
                "actual_quota": {"requests": 1, "input_tokens": 4, "output_tokens": 5}
            }
        }))
        .into_response()
    }

    async fn spawn(router: Router) -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        (origin, task)
    }

    fn authority() -> SponsoredModelProviderAuthority {
        SponsoredModelProviderAuthority {
            root_credential: ProviderCredentialSecret::new("root-provider-secret").unwrap(),
            grant_receipt: ProviderGrantReceipt {
                provider_grant_id: SensitiveProviderValue::new("provider-grant-reference").unwrap(),
                provider_state_version: 3,
                expires_at: "2027-01-02T03:04:05Z".into(),
                revocation_handle: SensitiveProviderValue::new("provider-revocation-reference")
                    .unwrap(),
                issued_at: "2026-01-02T03:04:05Z".into(),
            },
        }
    }

    fn provider_request() -> SponsoredModelProviderRequest {
        SponsoredModelProviderRequest {
            operation_id: SponsoredUsageReservationId::parse(
                "33333333-3333-4333-8333-333333333333",
            )
            .unwrap(),
            request_digest: "d".repeat(64),
            maximum_quota: SponsoredQuota {
                requests: 1,
                input_tokens: 128,
                output_tokens: 16,
            },
            model: "gpt-5".into(),
            resource: layer3_adapter::SPONSORED_MODEL_RESOURCE.into(),
            request: serde_json::json!({
                "max_output_tokens": 16,
                "model": "gpt-5",
                "stream": false
            }),
        }
    }

    #[test]
    fn production_origin_is_one_exact_https_origin() {
        assert!(HttpSponsoredModelProvider::new(
            "provider-adapter-v1".into(),
            "https://provider.example".into()
        )
        .is_ok());
        for invalid in [
            "http://provider.example",
            "https://provider.example/",
            "https://provider.example/path",
            "https://provider.example?query=true",
        ] {
            assert!(
                HttpSponsoredModelProvider::new("provider-adapter-v1".into(), invalid.into())
                    .is_err()
            );
        }
    }

    #[tokio::test]
    async fn operation_binding_is_stable_and_grant_reference_is_never_the_bearer() {
        let capture = Arc::new(Capture::default());
        let (origin, server) = spawn(
            Router::new()
                .route(EXECUTE_PATH, post(confirmed))
                .with_state(Arc::clone(&capture)),
        )
        .await;
        let adapter = HttpSponsoredModelProvider::fixture(
            origin,
            MAX_SPONSORED_MODEL_RESPONSE_BYTES + ENVELOPE_ALLOWANCE_BYTES,
        );
        let request = provider_request();
        let result = adapter.execute("openai", &authority(), &request).await;
        assert!(matches!(
            result,
            SponsoredModelProviderExecution::Confirmed(_)
        ));

        assert_eq!(
            capture.authorization.lock().unwrap().as_deref(),
            Some("Bearer root-provider-secret")
        );
        assert_eq!(
            capture.idempotency_key.lock().unwrap().as_deref(),
            Some(request.operation_id.0.as_str())
        );
        let body = capture.body.lock().unwrap().clone().unwrap();
        assert_eq!(body["operation_id"], request.operation_id.0);
        assert_eq!(
            body["grant"]["provider_grant_id"],
            "provider-grant-reference"
        );
        assert_eq!(body["resource"], layer3_adapter::SPONSORED_MODEL_RESOURCE);
        assert_ne!(
            capture.authorization.lock().unwrap().as_deref(),
            Some("Bearer provider-grant-reference")
        );
        server.abort();
    }

    #[tokio::test]
    async fn redirects_are_not_followed_after_dispatch() {
        async fn redirect() -> impl IntoResponse {
            (
                StatusCode::TEMPORARY_REDIRECT,
                [(header::LOCATION, "/redirect-target")],
            )
        }
        async fn target(State(calls): State<Arc<AtomicUsize>>) -> StatusCode {
            calls.fetch_add(1, Ordering::SeqCst);
            StatusCode::NO_CONTENT
        }

        let calls = Arc::new(AtomicUsize::new(0));
        let (origin, server) = spawn(
            Router::new()
                .route(EXECUTE_PATH, post(redirect))
                .route("/redirect-target", post(target))
                .with_state(Arc::clone(&calls)),
        )
        .await;
        let adapter = HttpSponsoredModelProvider::fixture(origin, 64 * 1024);
        assert!(matches!(
            adapter
                .execute("openai", &authority(), &provider_request())
                .await,
            SponsoredModelProviderExecution::Ambiguous
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        server.abort();
    }

    #[tokio::test]
    async fn reflected_provider_authority_is_never_returned() {
        async fn reflected(
            State(kind): State<&'static str>,
            headers: HeaderMap,
            Json(body): Json<serde_json::Value>,
        ) -> Response {
            let root = headers
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.strip_prefix("Bearer "))
                .unwrap();
            let grant = body["grant"]["provider_grant_id"].as_str().unwrap();
            let reflected = match kind.split_once('_').map(|(authority, _)| authority) {
                Some("root") => root,
                Some("grant") => grant,
                Some("revocation") => "provider-revocation-reference",
                _ => unreachable!(),
            };
            let response = serde_json::json!({
                "version": PROTOCOL_VERSION,
                "adapter_id": "provider-adapter-v1",
                "kind": EXECUTE_KIND,
                "operation_id": body["operation_id"],
                "request_digest": body["request_digest"],
                "status": "confirmed",
                "result": {
                    "response": {
                        "id": "provider-response-1",
                        "reflected": reflected
                    },
                    "actual_quota": {"requests": 1, "input_tokens": 4, "output_tokens": 5}
                }
            });
            if kind.ends_with("header_value") {
                ([("x-provider-debug", reflected)], Json(response)).into_response()
            } else if kind.ends_with("header_name") {
                let mut response = Json(response).into_response();
                response.headers_mut().insert(
                    reqwest::header::HeaderName::from_bytes(reflected.as_bytes()).unwrap(),
                    HeaderValue::from_static("reflected"),
                );
                response
            } else {
                Json(response).into_response()
            }
        }

        for kind in [
            "root_body",
            "grant_body",
            "revocation_body",
            "root_header_value",
            "grant_header_value",
            "revocation_header_value",
            "root_header_name",
            "grant_header_name",
            "revocation_header_name",
        ] {
            let (origin, server) = spawn(
                Router::new()
                    .route(EXECUTE_PATH, post(reflected))
                    .with_state(kind),
            )
            .await;
            let adapter = HttpSponsoredModelProvider::fixture(origin, 64 * 1024);
            assert!(matches!(
                adapter
                    .execute("openai", &authority(), &provider_request())
                    .await,
                SponsoredModelProviderExecution::Ambiguous
            ));
            server.abort();
        }
    }

    #[tokio::test]
    async fn oversized_and_malformed_responses_are_ambiguous() {
        async fn oversized() -> Response {
            (
                [(header::CONTENT_TYPE, "application/json")],
                vec![b'x'; 1024],
            )
                .into_response()
        }
        let (origin, oversized_server) =
            spawn(Router::new().route(EXECUTE_PATH, post(oversized))).await;
        let adapter = HttpSponsoredModelProvider::fixture(origin, 128);
        assert!(matches!(
            adapter
                .execute("openai", &authority(), &provider_request())
                .await,
            SponsoredModelProviderExecution::Ambiguous
        ));
        oversized_server.abort();

        async fn malformed() -> impl IntoResponse {
            Json(serde_json::json!({
                "version": PROTOCOL_VERSION,
                "adapter_id": "provider-adapter-v1",
                "kind": EXECUTE_KIND,
                "operation_id": "substituted-operation",
                "request_digest": "d".repeat(64),
                "status": "confirmed",
                "result": {
                    "response": {"id": "response"},
                    "actual_quota": {"requests": 1, "input_tokens": 1, "output_tokens": 1}
                }
            }))
        }
        let (origin, malformed_server) =
            spawn(Router::new().route(EXECUTE_PATH, post(malformed))).await;
        let adapter = HttpSponsoredModelProvider::fixture(origin, 64 * 1024);
        assert!(matches!(
            adapter
                .execute("openai", &authority(), &provider_request())
                .await,
            SponsoredModelProviderExecution::Ambiguous
        ));
        malformed_server.abort();
    }

    #[tokio::test]
    async fn observation_not_found_is_distinct_from_pending() {
        async fn not_found(Json(body): Json<serde_json::Value>) -> impl IntoResponse {
            (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({
                    "version": PROTOCOL_VERSION,
                    "adapter_id": "provider-adapter-v1",
                    "kind": OBSERVE_KIND,
                    "operation_id": body["operation_id"],
                    "request_digest": body["request_digest"],
                    "status": "not_found"
                })),
            )
        }
        let (origin, server) = spawn(Router::new().route(OBSERVE_PATH, post(not_found))).await;
        let adapter = HttpSponsoredModelProvider::fixture(origin, 64 * 1024);
        assert!(matches!(
            adapter
                .observe("openai", &authority(), &provider_request())
                .await,
            SponsoredModelProviderObservation::NotFound
        ));
        server.abort();
    }
}
