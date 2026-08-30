//! Private, session-authenticated entry point for sponsored model requests.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use axum::body::{to_bytes, Body};
use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::Router;
#[cfg(test)]
use layer2_core::MAX_SPONSORED_MODEL_REQUEST_BYTES;
use layer2_core::{
    InertGrantCapability, LaunchId, SessionId, SessionService, SponsoredGrantId,
    SponsoredModelDataPlaneError, SponsoredModelDataPlaneService, SponsoredModelProviderResponse,
    SponsoredModelRequest, SponsoredQuota, MAX_SPONSORED_MODEL_GATEWAY_ENVELOPE_BYTES,
    SPONSORED_MODEL_GATEWAY_REQUEST_VERSION,
};
use layer3_adapter::{
    SponsoredModelGatewayOrigin, SponsoredModelRuntimeAuthority, SPONSORED_MODEL_RESOURCE,
    SPONSORED_MODEL_RUNTIME_AUTHORITY_VERSION,
};
use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;
use zeroize::Zeroizing;

use crate::auth::AuthState;

const GATEWAY_PATH: &str = "/internal/v1/sponsored-model/responses";
const MAX_CONCURRENT_INGRESS_REQUESTS: usize = 64;
const MAX_CONCURRENT_PROVIDER_REQUESTS: usize = 32;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_BEARER_BYTES: usize = 64 * 1024;

/// Independently authenticated router for calls from sponsored runtimes.
pub(crate) struct SponsoredModelGateway {
    state: Arc<GatewayState>,
}

impl SponsoredModelGateway {
    pub(crate) fn new(
        auth: Arc<AuthState>,
        sessions: Arc<SessionService>,
        data_plane: Arc<SponsoredModelDataPlaneService>,
        gateway_origin: SponsoredModelGatewayOrigin,
    ) -> Self {
        Self::with_ports(
            auth,
            sessions,
            data_plane,
            gateway_origin,
            MAX_CONCURRENT_INGRESS_REQUESTS,
            MAX_CONCURRENT_PROVIDER_REQUESTS,
        )
    }

    fn with_ports(
        auth: Arc<dyn SessionCredentialAuthenticator>,
        sessions: Arc<dyn SponsoredRuntimeVerifier>,
        data_plane: Arc<dyn SponsoredModelExecutor>,
        gateway_origin: SponsoredModelGatewayOrigin,
        maximum_concurrent_ingress_requests: usize,
        maximum_concurrent_provider_requests: usize,
    ) -> Self {
        Self {
            state: Arc::new(GatewayState {
                auth,
                sessions,
                data_plane,
                gateway_origin,
                ingress_concurrency: Arc::new(Semaphore::new(maximum_concurrent_ingress_requests)),
                provider_concurrency: Arc::new(Semaphore::new(
                    maximum_concurrent_provider_requests,
                )),
            }),
        }
    }

    pub(crate) fn router(self) -> Router {
        Router::new()
            .route(GATEWAY_PATH, any(gateway_request))
            .with_state(self.state)
    }
}

struct GatewayState {
    auth: Arc<dyn SessionCredentialAuthenticator>,
    sessions: Arc<dyn SponsoredRuntimeVerifier>,
    data_plane: Arc<dyn SponsoredModelExecutor>,
    gateway_origin: SponsoredModelGatewayOrigin,
    ingress_concurrency: Arc<Semaphore>,
    provider_concurrency: Arc<Semaphore>,
}

#[async_trait]
trait SessionCredentialAuthenticator: Send + Sync {
    async fn authenticates(
        &self,
        session: &SessionId,
        custody_generation: u64,
        bearer: &str,
    ) -> Result<bool, ()>;
}

#[async_trait]
impl SessionCredentialAuthenticator for AuthState {
    async fn authenticates(
        &self,
        session: &SessionId,
        custody_generation: u64,
        bearer: &str,
    ) -> Result<bool, ()> {
        self.authenticates_session_credential(session, custody_generation, bearer)
            .await
            .map_err(|_| ())
    }
}

#[async_trait]
trait SponsoredRuntimeVerifier: Send + Sync {
    async fn verifies(&self, session: &SessionId, launch: &LaunchId, audience: &str) -> bool;
}

#[async_trait]
impl SponsoredRuntimeVerifier for SessionService {
    async fn verifies(&self, session: &SessionId, launch: &LaunchId, audience: &str) -> bool {
        self.verifies_current_sponsored_model_runtime(session, launch, audience)
            .await
    }
}

#[async_trait]
trait SponsoredModelExecutor: Send + Sync {
    async fn execute(
        &self,
        capability: &InertGrantCapability,
        request: SponsoredModelRequest,
    ) -> Result<SponsoredModelProviderResponse, SponsoredModelDataPlaneError>;
}

#[async_trait]
impl SponsoredModelExecutor for SponsoredModelDataPlaneService {
    async fn execute(
        &self,
        capability: &InertGrantCapability,
        request: SponsoredModelRequest,
    ) -> Result<SponsoredModelProviderResponse, SponsoredModelDataPlaneError> {
        self.execute(capability, request).await
    }
}

async fn gateway_request(State(state): State<Arc<GatewayState>>, request: Request) -> Response {
    let deadline = tokio::time::Instant::now() + REQUEST_TIMEOUT;
    let ingress_permit = match Arc::clone(&state.ingress_concurrency).try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => return private_error(StatusCode::SERVICE_UNAVAILABLE, ErrorCode::Unavailable),
    };

    // Everything before provider dispatch is cancellable. In particular, a
    // peer that never finishes its body must lose its permit at the request
    // deadline instead of leaving a detached validation task behind forever.
    let prepared = match tokio::time::timeout_at(deadline, prepare_request(&state, request)).await {
        Ok(Ok(prepared)) => prepared,
        Ok(Err(response)) => return response,
        Err(_) => return private_error(StatusCode::GATEWAY_TIMEOUT, ErrorCode::Unavailable),
    };
    drop(ingress_permit);
    let provider_permit = match Arc::clone(&state.provider_concurrency).try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => return private_error(StatusCode::SERVICE_UNAVAILABLE, ErrorCode::Unavailable),
    };

    // Crossing this spawn is the provider-dispatch boundary. The client
    // deadline must not cancel a provider operation after it: dropping the
    // JoinHandle detaches the task and keeps durable reconciliation plus its
    // concurrency permit alive until execution finishes.
    let work = tokio::spawn(async move {
        let response = execute_request(&state, prepared).await;
        drop(provider_permit);
        response
    });
    let outcome = tokio::time::timeout_at(deadline, work).await;
    match outcome {
        Ok(Ok(response)) => response,
        Ok(Err(_)) => private_error(StatusCode::SERVICE_UNAVAILABLE, ErrorCode::Unavailable),
        Err(_) => private_error(StatusCode::GATEWAY_TIMEOUT, ErrorCode::Unavailable),
    }
}

struct PreparedGatewayRequest {
    capability: InertGrantCapability,
    model_request: SponsoredModelRequest,
}

async fn prepare_request(
    state: &GatewayState,
    request: Request,
) -> Result<PreparedGatewayRequest, Response> {
    if request.method() != Method::POST
        || request.uri().path() != GATEWAY_PATH
        || request.uri().query().is_some()
        || !has_exact_json_content_type(request.headers())
    {
        return Err(private_error(
            StatusCode::BAD_REQUEST,
            ErrorCode::InvalidRequest,
        ));
    }
    let bearer = match exact_bearer(request.headers()) {
        Some(bearer) => bearer,
        None => {
            return Err(private_error(
                StatusCode::FORBIDDEN,
                ErrorCode::AuthorityRefused,
            ))
        }
    };
    let body = match to_bytes(
        request.into_body(),
        MAX_SPONSORED_MODEL_GATEWAY_ENVELOPE_BYTES,
    )
    .await
    {
        Ok(body) => body,
        Err(_) => {
            return Err(private_error(
                StatusCode::PAYLOAD_TOO_LARGE,
                ErrorCode::InvalidRequest,
            ))
        }
    };
    let envelope: GatewayEnvelope = match serde_json::from_slice(&body) {
        Ok(envelope) => envelope,
        Err(_) => {
            return Err(private_error(
                StatusCode::BAD_REQUEST,
                ErrorCode::InvalidRequest,
            ))
        }
    };
    let (capability, model_request, session, launch, audience, custody_generation) =
        match validate_envelope(envelope, &state.gateway_origin) {
            Ok(validated) => validated,
            Err(()) => {
                return Err(private_error(
                    StatusCode::FORBIDDEN,
                    ErrorCode::AuthorityRefused,
                ))
            }
        };

    let authenticated = state
        .auth
        .authenticates(&session, custody_generation, bearer.as_str())
        .await
        .unwrap_or(false);
    if !authenticated || !state.sessions.verifies(&session, &launch, &audience).await {
        return Err(private_error(
            StatusCode::FORBIDDEN,
            ErrorCode::AuthorityRefused,
        ));
    }

    Ok(PreparedGatewayRequest {
        capability,
        model_request,
    })
}

async fn execute_request(state: &GatewayState, prepared: PreparedGatewayRequest) -> Response {
    let PreparedGatewayRequest {
        capability,
        model_request,
    } = prepared;
    match state.data_plane.execute(&capability, model_request).await {
        Ok(response) => private_json(StatusCode::OK, response.response),
        Err(SponsoredModelDataPlaneError::InvalidRequest(_)) => {
            private_error(StatusCode::BAD_REQUEST, ErrorCode::InvalidRequest)
        }
        Err(
            SponsoredModelDataPlaneError::ProviderRejected
            | SponsoredModelDataPlaneError::ReservationReleased,
        ) => private_error(StatusCode::FORBIDDEN, ErrorCode::RequestRefused),
        Err(
            SponsoredModelDataPlaneError::Sponsorship(_)
            | SponsoredModelDataPlaneError::ProviderUnavailable
            | SponsoredModelDataPlaneError::ProviderAmbiguous
            | SponsoredModelDataPlaneError::Accounting(_),
        ) => private_error(StatusCode::SERVICE_UNAVAILABLE, ErrorCode::Unavailable),
    }
}

type ValidatedEnvelope = (
    InertGrantCapability,
    SponsoredModelRequest,
    SessionId,
    LaunchId,
    String,
    u64,
);

fn validate_envelope(
    envelope: GatewayEnvelope,
    gateway_origin: &SponsoredModelGatewayOrigin,
) -> Result<ValidatedEnvelope, ()> {
    let authority = envelope.authority;
    if envelope.version != SPONSORED_MODEL_GATEWAY_REQUEST_VERSION
        || authority.version != SPONSORED_MODEL_RUNTIME_AUTHORITY_VERSION
        || authority.gateway_origin != *gateway_origin
        || envelope.resource != SPONSORED_MODEL_RESOURCE
        || authority.session_credential_generation == 0
        || authority.runtime_audience.is_empty()
        || authority.runtime_audience.len() > 2_048
    {
        return Err(());
    }
    let session = SessionId(authority.session_id.clone());
    if !session.is_trigger_managed() {
        return Err(());
    }
    let launch = LaunchId::parse(authority.launch_id.clone()).map_err(|_| ())?;
    let capability = InertGrantCapability {
        grant_id: SponsoredGrantId::parse(authority.grant_id).map_err(|_| ())?,
        grant_generation: authority.grant_generation,
        policy_generation: authority.policy_generation,
        launch_id: launch.clone(),
        session_id: session.clone(),
        expires_at: authority.expires_at,
        fingerprint: authority.capability_fingerprint,
    };
    capability.validate().map_err(|_| ())?;
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    if layer2_core::seconds_from_rfc3339(&capability.expires_at)
        .is_none_or(|expires_at| expires_at <= now)
    {
        return Err(());
    }
    let request = SponsoredModelRequest {
        stable_request_id: envelope.stable_request_id,
        request_digest: envelope.request_digest,
        maximum_quota: envelope.maximum_quota.into(),
        model: envelope.model,
        resource: envelope.resource,
        request: envelope.request,
    };
    request.validate().map_err(|_| ())?;
    Ok((
        capability,
        request,
        session,
        launch,
        authority.runtime_audience,
        authority.session_credential_generation,
    ))
}

fn has_exact_json_content_type(headers: &HeaderMap) -> bool {
    headers.get_all(header::CONTENT_TYPE).iter().count() == 1
        && headers
            .get(header::CONTENT_TYPE)
            .is_some_and(|value| value.as_bytes() == b"application/json")
        && !headers.contains_key(header::CONTENT_ENCODING)
}

fn exact_bearer(headers: &HeaderMap) -> Option<Zeroizing<String>> {
    if headers.get_all(header::AUTHORIZATION).iter().count() != 1 {
        return None;
    }
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let bearer = value.strip_prefix("Bearer ")?;
    if bearer.is_empty() || bearer.len() > MAX_BEARER_BYTES {
        return None;
    }
    Some(Zeroizing::new(bearer.to_string()))
}

fn private_json(status: StatusCode, value: serde_json::Value) -> Response {
    let mut response = (status, axum::Json(value)).into_response();
    apply_private_headers(&mut response);
    response
}

fn private_error(status: StatusCode, code: ErrorCode) -> Response {
    let mut response = (
        status,
        axum::Json(ErrorEnvelope {
            error: ErrorBody {
                code,
                message: code.message(),
            },
        }),
    )
        .into_response();
    apply_private_headers(&mut response);
    response
}

fn apply_private_headers(response: &mut Response<Body>) {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum ErrorCode {
    InvalidRequest,
    AuthorityRefused,
    RequestRefused,
    Unavailable,
}

impl ErrorCode {
    fn message(self) -> &'static str {
        match self {
            Self::InvalidRequest => "The sponsored model request is invalid.",
            Self::AuthorityRefused => "Sponsored model authority was refused.",
            Self::RequestRefused => "The sponsored model request was refused.",
            Self::Unavailable => "The sponsored model service is unavailable.",
        }
    }
}

#[derive(Serialize)]
struct ErrorEnvelope {
    error: ErrorBody,
}

#[derive(Serialize)]
struct ErrorBody {
    code: ErrorCode,
    message: &'static str,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GatewayEnvelope {
    version: u32,
    authority: SponsoredModelRuntimeAuthority,
    stable_request_id: String,
    request_digest: String,
    maximum_quota: GatewayQuota,
    model: String,
    resource: String,
    request: serde_json::Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GatewayQuota {
    requests: u64,
    input_tokens: u64,
    output_tokens: u64,
}

impl From<GatewayQuota> for SponsoredQuota {
    fn from(value: GatewayQuota) -> Self {
        Self {
            requests: value.requests,
            input_tokens: value.input_tokens,
            output_tokens: value.output_tokens,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use axum::body::{to_bytes, Bytes};
    use axum::http::Request;
    use layer2_core::SponsoredModelProviderResponse;
    use sha2::{Digest, Sha256};
    use tower::ServiceExt as _;

    use super::*;

    struct FixtureAuth;

    #[async_trait]
    impl SessionCredentialAuthenticator for FixtureAuth {
        async fn authenticates(
            &self,
            _session: &SessionId,
            custody_generation: u64,
            bearer: &str,
        ) -> Result<bool, ()> {
            Ok(custody_generation == 7 && bearer == "exact-session-credential")
        }
    }

    struct FixtureRuntime;

    struct RotatedFixtureAuth;

    struct BlockingAuth {
        started: Semaphore,
        release: Semaphore,
    }

    impl BlockingAuth {
        fn new() -> Self {
            Self {
                started: Semaphore::new(0),
                release: Semaphore::new(0),
            }
        }
    }

    #[async_trait]
    impl SessionCredentialAuthenticator for BlockingAuth {
        async fn authenticates(
            &self,
            _session: &SessionId,
            _custody_generation: u64,
            _bearer: &str,
        ) -> Result<bool, ()> {
            self.started.add_permits(1);
            self.release
                .acquire()
                .await
                .expect("the blocking authentication fixture remains open")
                .forget();
            Ok(false)
        }
    }

    #[async_trait]
    impl SessionCredentialAuthenticator for RotatedFixtureAuth {
        async fn authenticates(
            &self,
            _session: &SessionId,
            custody_generation: u64,
            bearer: &str,
        ) -> Result<bool, ()> {
            Ok(custody_generation == 8 && bearer == "replacement-session-credential")
        }
    }

    #[async_trait]
    impl SponsoredRuntimeVerifier for FixtureRuntime {
        async fn verifies(&self, _session: &SessionId, _launch: &LaunchId, audience: &str) -> bool {
            audience == "runtime-current"
        }
    }

    #[derive(Default)]
    struct FixtureExecutor {
        calls: AtomicUsize,
    }

    #[async_trait]
    impl SponsoredModelExecutor for FixtureExecutor {
        async fn execute(
            &self,
            _capability: &InertGrantCapability,
            _request: SponsoredModelRequest,
        ) -> Result<SponsoredModelProviderResponse, SponsoredModelDataPlaneError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(SponsoredModelProviderResponse {
                response: serde_json::json!({"id": "response-1", "object": "response"}),
                actual_quota: SponsoredQuota {
                    requests: 1,
                    input_tokens: 4,
                    output_tokens: 5,
                },
            })
        }
    }

    struct BlockingExecutor {
        started: Semaphore,
        release: Semaphore,
    }

    impl BlockingExecutor {
        fn new() -> Self {
            Self {
                started: Semaphore::new(0),
                release: Semaphore::new(0),
            }
        }
    }

    #[async_trait]
    impl SponsoredModelExecutor for BlockingExecutor {
        async fn execute(
            &self,
            _capability: &InertGrantCapability,
            _request: SponsoredModelRequest,
        ) -> Result<SponsoredModelProviderResponse, SponsoredModelDataPlaneError> {
            self.started.add_permits(1);
            self.release
                .acquire()
                .await
                .expect("the blocking gateway fixture remains open")
                .forget();
            Ok(SponsoredModelProviderResponse {
                response: serde_json::json!({"id": "response-1", "object": "response"}),
                actual_quota: SponsoredQuota {
                    requests: 1,
                    input_tokens: 4,
                    output_tokens: 5,
                },
            })
        }
    }

    fn gateway(executor: Arc<FixtureExecutor>) -> Router {
        gateway_with_executor(executor)
    }

    fn gateway_with_executor(executor: Arc<dyn SponsoredModelExecutor>) -> Router {
        gateway_with_executor_and_limit(executor, MAX_CONCURRENT_PROVIDER_REQUESTS).router()
    }

    fn gateway_with_executor_and_limit(
        executor: Arc<dyn SponsoredModelExecutor>,
        maximum_concurrent_requests: usize,
    ) -> SponsoredModelGateway {
        SponsoredModelGateway::with_ports(
            Arc::new(FixtureAuth),
            Arc::new(FixtureRuntime),
            executor,
            SponsoredModelGatewayOrigin::parse("https://cma.internal.example").unwrap(),
            MAX_CONCURRENT_INGRESS_REQUESTS,
            maximum_concurrent_requests,
        )
    }

    fn request(audience: &str, bearer: &str) -> Request<Body> {
        request_with_expiry(audience, bearer, "2099-01-02T03:04:05Z")
    }

    fn request_with_expiry(audience: &str, bearer: &str, expires_at: &str) -> Request<Body> {
        let model_request =
            serde_json::json!({"max_output_tokens": 16, "model": "gpt-5", "stream": false});
        request_with_model_request(audience, bearer, expires_at, model_request)
    }

    fn request_with_model_request(
        audience: &str,
        bearer: &str,
        expires_at: &str,
        model_request: serde_json::Value,
    ) -> Request<Body> {
        request_with_model_request_and_generation(audience, bearer, expires_at, 7, model_request)
    }

    fn request_with_model_request_and_generation(
        audience: &str,
        bearer: &str,
        expires_at: &str,
        custody_generation: u64,
        model_request: serde_json::Value,
    ) -> Request<Body> {
        let canonical = serde_json::to_vec(&model_request).unwrap();
        let digest = lower_hex(Sha256::digest(&canonical));
        let body = serde_json::json!({
            "version": SPONSORED_MODEL_GATEWAY_REQUEST_VERSION,
            "authority": {
                "version": SPONSORED_MODEL_RUNTIME_AUTHORITY_VERSION,
                "gateway_origin": "https://cma.internal.example",
                "runtime_audience": audience,
                "session_id": "sesn_trg_123456781234123412341234567890ab",
                "session_credential_generation": custody_generation,
                "launch_id": "11111111-1111-4111-8111-111111111111",
                "grant_id": "22222222-2222-4222-8222-222222222222",
                "grant_generation": 3,
                "policy_generation": 4,
                "maximum_output_tokens_per_request": 16,
                "expires_at": expires_at,
                "capability_fingerprint": "a".repeat(64),
            },
            "stable_request_id": format!("smr-b-{}-{digest}", "1".repeat(64)),
            "request_digest": digest,
            "maximum_quota": {
                "requests": 1,
                "input_tokens": canonical.len(),
                "output_tokens": 16,
            },
            "model": "gpt-5",
            "resource": SPONSORED_MODEL_RESOURCE,
            "request": model_request,
        });
        Request::builder()
            .method(Method::POST)
            .uri(GATEWAY_PATH)
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::AUTHORIZATION, format!("Bearer {bearer}"))
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap()
    }

    async fn body(response: Response) -> Vec<u8> {
        to_bytes(response.into_body(), 64 * 1024)
            .await
            .unwrap()
            .to_vec()
    }

    fn assert_private(response: &Response) {
        assert_eq!(
            response.headers().get(header::CACHE_CONTROL),
            Some(&HeaderValue::from_static("no-store"))
        );
        assert_eq!(
            response.headers().get(header::REFERRER_POLICY),
            Some(&HeaderValue::from_static("no-referrer"))
        );
    }

    #[tokio::test]
    async fn invalid_bearer_and_audience_are_indistinguishable_and_never_execute() {
        let executor = Arc::new(FixtureExecutor::default());
        let invalid_bearer = gateway(Arc::clone(&executor))
            .oneshot(request("runtime-current", "wrong"))
            .await
            .unwrap();
        assert_eq!(invalid_bearer.status(), StatusCode::FORBIDDEN);
        assert_private(&invalid_bearer);
        let invalid_bearer_body = body(invalid_bearer).await;

        let invalid_audience = gateway(Arc::clone(&executor))
            .oneshot(request("runtime-retired", "exact-session-credential"))
            .await
            .unwrap();
        assert_eq!(invalid_audience.status(), StatusCode::FORBIDDEN);
        assert_private(&invalid_audience);
        assert_eq!(invalid_bearer_body, body(invalid_audience).await);
        assert_eq!(executor.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn unauthenticated_ingress_is_bounded_without_consuming_provider_capacity() {
        let auth = Arc::new(BlockingAuth::new());
        let executor = Arc::new(FixtureExecutor::default());
        let sponsored_gateway = SponsoredModelGateway::with_ports(
            auth.clone(),
            Arc::new(FixtureRuntime),
            executor.clone(),
            SponsoredModelGatewayOrigin::parse("https://cma.internal.example").unwrap(),
            1,
            1,
        );
        let ingress = Arc::clone(&sponsored_gateway.state.ingress_concurrency);
        let providers = Arc::clone(&sponsored_gateway.state.provider_concurrency);
        let gateway = sponsored_gateway.router();
        let blocked = tokio::spawn({
            let gateway = gateway.clone();
            async move {
                gateway
                    .oneshot(request("runtime-current", "untrusted-credential"))
                    .await
                    .unwrap()
            }
        });
        auth.started.acquire().await.unwrap().forget();

        assert!(Arc::clone(&ingress).try_acquire_owned().is_err());
        let provider_capacity = Arc::clone(&providers).try_acquire_owned().unwrap();
        let overflow = gateway
            .oneshot(request("runtime-current", "another-untrusted-credential"))
            .await
            .unwrap();
        assert_eq!(overflow.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_private(&overflow);
        drop(provider_capacity);

        auth.release.add_permits(1);
        let refused = blocked.await.unwrap();
        assert_eq!(refused.status(), StatusCode::FORBIDDEN);
        assert_private(&refused);
        assert_eq!(executor.calls.load(Ordering::SeqCst), 0);
        let returned = ingress.try_acquire_owned().unwrap();
        drop(returned);
    }

    #[tokio::test]
    async fn valid_request_returns_only_provider_json_with_private_headers() {
        let executor = Arc::new(FixtureExecutor::default());
        let response = gateway(Arc::clone(&executor))
            .oneshot(request("runtime-current", "exact-session-credential"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_private(&response);
        let value: serde_json::Value = serde_json::from_slice(&body(response).await).unwrap();
        assert_eq!(
            value,
            serde_json::json!({"id": "response-1", "object": "response"})
        );
        assert_eq!(executor.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn replacement_bearer_authenticates_only_with_its_exact_forward_generation() {
        let executor = Arc::new(FixtureExecutor::default());
        let gateway = SponsoredModelGateway::with_ports(
            Arc::new(RotatedFixtureAuth),
            Arc::new(FixtureRuntime),
            executor.clone(),
            SponsoredModelGatewayOrigin::parse("https://cma.internal.example").unwrap(),
            2,
            1,
        )
        .router();
        let model_request =
            serde_json::json!({"max_output_tokens": 16, "model": "gpt-5", "stream": false});
        let stale = request_with_model_request_and_generation(
            "runtime-current",
            "replacement-session-credential",
            "2099-01-02T03:04:05Z",
            7,
            model_request.clone(),
        );
        assert_eq!(
            gateway.clone().oneshot(stale).await.unwrap().status(),
            StatusCode::FORBIDDEN
        );
        let replacement = request_with_model_request_and_generation(
            "runtime-current",
            "replacement-session-credential",
            "2099-01-02T03:04:05Z",
            8,
            model_request,
        );

        let response = gateway.oneshot(replacement).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(executor.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn query_and_non_post_requests_are_locally_refused() {
        let executor = Arc::new(FixtureExecutor::default());
        let mut query = request("runtime-current", "exact-session-credential");
        *query.uri_mut() = format!("{GATEWAY_PATH}?redirect=true").parse().unwrap();
        let query_response = gateway(Arc::clone(&executor)).oneshot(query).await.unwrap();
        assert_eq!(query_response.status(), StatusCode::BAD_REQUEST);
        assert_private(&query_response);

        let mut get = request("runtime-current", "exact-session-credential");
        *get.method_mut() = Method::GET;
        let get_response = gateway(Arc::clone(&executor)).oneshot(get).await.unwrap();
        assert_eq!(get_response.status(), StatusCode::BAD_REQUEST);
        assert_private(&get_response);
        assert_eq!(executor.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn expired_capability_is_an_authority_refusal_before_data_plane_execution() {
        let executor = Arc::new(FixtureExecutor::default());
        let response = gateway(Arc::clone(&executor))
            .oneshot(request_with_expiry(
                "runtime-current",
                "exact-session-credential",
                "1970-01-01T00:00:01Z",
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_private(&response);
        assert_eq!(executor.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn media_type_encoding_and_body_bounds_are_enforced_before_execution() {
        let executor = Arc::new(FixtureExecutor::default());
        let mut missing = request("runtime-current", "exact-session-credential");
        missing.headers_mut().remove(header::CONTENT_TYPE);
        let mut parameterized = request("runtime-current", "exact-session-credential");
        parameterized.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json; charset=utf-8"),
        );
        let mut encoded = request("runtime-current", "exact-session-credential");
        encoded
            .headers_mut()
            .insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
        for invalid in [missing, parameterized, encoded] {
            let response = gateway(Arc::clone(&executor))
                .oneshot(invalid)
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
            assert_private(&response);
        }

        let oversized = Request::builder()
            .method(Method::POST)
            .uri(GATEWAY_PATH)
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::AUTHORIZATION, "Bearer exact-session-credential")
            .body(Body::from(vec![
                b'x';
                MAX_SPONSORED_MODEL_GATEWAY_ENVELOPE_BYTES
                    + 1
            ]))
            .unwrap();
        let response = gateway(Arc::clone(&executor))
            .oneshot(oversized)
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert_private(&response);
        assert_eq!(executor.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn one_mib_canonical_model_request_fits_inside_the_gateway_envelope_allowance() {
        let executor = Arc::new(FixtureExecutor::default());
        let mut model_request = serde_json::json!({
            "input": "",
            "max_output_tokens": 16,
            "model": "gpt-5",
            "stream": false,
        });
        let empty_length = serde_json::to_vec(&model_request).unwrap().len();
        model_request["input"] =
            serde_json::Value::String("x".repeat(MAX_SPONSORED_MODEL_REQUEST_BYTES - empty_length));
        assert_eq!(
            serde_json::to_vec(&model_request).unwrap().len(),
            MAX_SPONSORED_MODEL_REQUEST_BYTES
        );
        let request = request_with_model_request(
            "runtime-current",
            "exact-session-credential",
            "2099-01-02T03:04:05Z",
            model_request,
        );

        let response = gateway(Arc::clone(&executor))
            .oneshot(request)
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_private(&response);
        assert_eq!(executor.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn thirty_two_calls_are_admitted_and_the_next_call_is_refused_without_queueing() {
        let executor = Arc::new(BlockingExecutor::new());
        let gateway = gateway_with_executor(executor.clone());
        let mut in_flight = Vec::new();
        for _ in 0..MAX_CONCURRENT_PROVIDER_REQUESTS {
            let gateway = gateway.clone();
            in_flight.push(tokio::spawn(async move {
                gateway
                    .oneshot(request("runtime-current", "exact-session-credential"))
                    .await
                    .unwrap()
            }));
        }
        executor
            .started
            .acquire_many(u32::try_from(MAX_CONCURRENT_PROVIDER_REQUESTS).unwrap())
            .await
            .unwrap()
            .forget();

        let overflow = gateway
            .oneshot(request("runtime-current", "exact-session-credential"))
            .await
            .unwrap();
        assert_eq!(overflow.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_private(&overflow);

        executor
            .release
            .add_permits(MAX_CONCURRENT_PROVIDER_REQUESTS);
        for call in in_flight {
            assert_eq!(call.await.unwrap().status(), StatusCode::OK);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn provider_execution_timeout_is_private_and_does_not_cancel_admitted_work() {
        let executor = Arc::new(BlockingExecutor::new());
        let sponsored_gateway = gateway_with_executor_and_limit(executor.clone(), 1);
        let concurrency = Arc::clone(&sponsored_gateway.state.provider_concurrency);
        let gateway = sponsored_gateway.router();
        let response = gateway
            .clone()
            .oneshot(request("runtime-current", "exact-session-credential"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::GATEWAY_TIMEOUT);
        assert_private(&response);

        let overflow = gateway
            .clone()
            .oneshot(request("runtime-current", "exact-session-credential"))
            .await
            .unwrap();
        assert_eq!(overflow.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_private(&overflow);

        executor.release.add_permits(1);
        let returned = Arc::clone(&concurrency).acquire_owned().await.unwrap();
        drop(returned);

        executor.release.add_permits(1);
        let replacement = gateway.oneshot(request("runtime-current", "exact-session-credential"));
        tokio::pin!(replacement);
        tokio::select! {
            response = &mut replacement => {
                assert_eq!(response.unwrap().status(), StatusCode::OK);
            }
            _ = tokio::time::sleep(REQUEST_TIMEOUT + Duration::from_secs(1)) => {
                panic!("completed detached work must return its concurrency permit");
            }
        }
    }

    #[tokio::test(start_paused = true)]
    async fn stalled_predispatch_body_is_cancelled_and_returns_its_permit() {
        let executor = Arc::new(FixtureExecutor::default());
        let sponsored_gateway = gateway_with_executor_and_limit(executor.clone(), 1);
        let concurrency = Arc::clone(&sponsored_gateway.state.ingress_concurrency);
        let gateway = sponsored_gateway.router();
        let stalled = Request::builder()
            .method(Method::POST)
            .uri(GATEWAY_PATH)
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::AUTHORIZATION, "Bearer exact-session-credential")
            .body(Body::from_stream(futures_util::stream::pending::<
                Result<Bytes, std::convert::Infallible>,
            >()))
            .unwrap();

        let response = gateway.clone().oneshot(stalled).await.unwrap();

        assert_eq!(response.status(), StatusCode::GATEWAY_TIMEOUT);
        assert_private(&response);
        assert_eq!(executor.calls.load(Ordering::SeqCst), 0);
        let returned = Arc::clone(&concurrency).try_acquire_owned().unwrap();
        drop(returned);

        let replacement = gateway
            .oneshot(request("runtime-current", "exact-session-credential"))
            .await
            .unwrap();
        assert_eq!(replacement.status(), StatusCode::OK);
        assert_eq!(executor.calls.load(Ordering::SeqCst), 1);
    }

    fn lower_hex(bytes: impl AsRef<[u8]>) -> String {
        bytes
            .as_ref()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }
}
