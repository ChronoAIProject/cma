//! Versioned HTTP adapters for sponsorship-owned external services.
//!
//! URLs are composed exclusively from a configuration-time canonical origin
//! and the fixed paths below. Request projections are deliberately explicit:
//! provider credential material and raw provider authority never enter the
//! billing or pricing contracts.

use std::collections::BTreeSet;
use std::fmt;
use std::fmt::Write as _;
use std::time::Duration;

use async_trait::async_trait;
use futures_util::StreamExt;
use layer2_core::{
    seconds_from_rfc3339, BillingAuthorization, LaunchId, PendingProviderOperation,
    ProviderCredentialEvidence, ProviderCredentialSecret, ProviderGrantReceipt,
    ProviderOperationKind, ProviderOperationPhase, ProviderOperationResolution,
    SensitiveProviderValue, SessionId, SponsorBillingOwner, SponsoredBillingError,
    SponsoredBillingProvider, SponsoredCredentialMaterialState, SponsoredCredentialProvider,
    SponsoredCredentialRecord, SponsoredCredentialState, SponsoredGrant, SponsoredGrantState,
    SponsoredPriceQuote, SponsoredPricingError, SponsoredPricingProvider, SponsoredProviderError,
    SponsoredQuota, SponsoredUsageSummary, SponsorshipPolicy, SponsorshipPolicyState,
};
use reqwest::header::{
    HeaderMap, HeaderName, HeaderValue, ACCEPT, ACCEPT_ENCODING, CONTENT_ENCODING, CONTENT_TYPE,
};
use reqwest::{StatusCode, Url};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};
use sha2::{Digest, Sha256};

const PROTOCOL_VERSION: u32 = 1;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_REQUEST_BODY_BYTES: usize = 256 * 1024;
const MAX_RESPONSE_BODY_BYTES: usize = 64 * 1024;
const MAX_ADAPTER_ID_BYTES: usize = 128;
const MAX_ORIGIN_BYTES: usize = 2_048;

const ADAPTER_ID_HEADER: HeaderName = HeaderName::from_static("x-cma-sponsorship-adapter-id");
const REQUEST_ID_HEADER: HeaderName = HeaderName::from_static("x-cma-request-id");
const IDEMPOTENCY_KEY_HEADER: HeaderName = HeaderName::from_static("idempotency-key");

const OWNERSHIP_PATH: &str = "/v1/sponsorship/credentials/ownership:validate";
const ISSUE_GRANT_PATH: &str = "/v1/sponsorship/grants:issue";
const RECONCILE_GRANT_ISSUE_PATH: &str = "/v1/sponsorship/grants:reconcile-issue";
const REVOKE_UNCOMMITTED_GRANT_PATH: &str = "/v1/sponsorship/grants:revoke-uncommitted";
const RECONCILE_UNCOMMITTED_GRANT_REVOCATION_PATH: &str =
    "/v1/sponsorship/grants:reconcile-uncommitted-revocation";
const ROTATE_CREDENTIAL_PATH: &str = "/v1/sponsorship/credentials:rotate";
const RECONCILE_CREDENTIAL_ROTATION_PATH: &str = "/v1/sponsorship/credentials:reconcile-rotation";
const REVOKE_CREDENTIAL_PATH: &str = "/v1/sponsorship/credentials:revoke";
const RECONCILE_CREDENTIAL_REVOCATION_PATH: &str =
    "/v1/sponsorship/credentials:reconcile-revocation";
const REVOKE_GRANT_PATH: &str = "/v1/sponsorship/grants:revoke";
const RECONCILE_GRANT_REVOCATION_PATH: &str = "/v1/sponsorship/grants:reconcile-revocation";
const BILLING_CONFIRM_PATH: &str = "/v1/sponsorship/billing/grants:confirm";
const BILLING_RECONCILE_PATH: &str = "/v1/sponsorship/billing/grants:reconcile";
const PRICING_QUOTE_PATH: &str = "/v1/sponsorship/pricing:quote";
const PRICING_ACTUAL_PATH: &str = "/v1/sponsorship/pricing:price-actual";

const OWNERSHIP_KIND: &str = "credential.validate_ownership";
const ISSUE_GRANT_KIND: &str = "credential.issue_grant";
const RECONCILE_GRANT_ISSUE_KIND: &str = "credential.reconcile_grant_issue";
const REVOKE_UNCOMMITTED_GRANT_KIND: &str = "credential.revoke_uncommitted_grant";
const RECONCILE_UNCOMMITTED_GRANT_REVOCATION_KIND: &str =
    "credential.reconcile_uncommitted_grant_revocation";
const ROTATE_CREDENTIAL_KIND: &str = "credential.rotate";
const RECONCILE_CREDENTIAL_ROTATION_KIND: &str = "credential.reconcile_rotation";
const REVOKE_CREDENTIAL_KIND: &str = "credential.revoke";
const RECONCILE_CREDENTIAL_REVOCATION_KIND: &str = "credential.reconcile_revocation";
const REVOKE_GRANT_KIND: &str = "credential.revoke_grant";
const RECONCILE_GRANT_REVOCATION_KIND: &str = "credential.reconcile_grant_revocation";
const BILLING_CONFIRM_KIND: &str = "billing.confirm_grant";
const BILLING_RECONCILE_KIND: &str = "billing.reconcile_grant";
const PRICING_QUOTE_KIND: &str = "pricing.quote";
const PRICING_ACTUAL_KIND: &str = "pricing.price_actual";

#[derive(Clone)]
struct HttpAdapter {
    adapter_id: String,
    origin: String,
    client: reqwest::Client,
    maximum_request_body: usize,
    maximum_response_body: usize,
}

impl fmt::Debug for HttpAdapter {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HttpAdapter")
            .field("adapter_id", &self.adapter_id)
            .field("origin", &self.origin)
            .field("maximum_request_body", &self.maximum_request_body)
            .field("maximum_response_body", &self.maximum_response_body)
            .finish_non_exhaustive()
    }
}

impl HttpAdapter {
    fn production(adapter_id: String, origin: String) -> Result<Self, String> {
        Self::build(
            adapter_id,
            origin,
            false,
            CONNECT_TIMEOUT,
            REQUEST_TIMEOUT,
            MAX_REQUEST_BODY_BYTES,
            MAX_RESPONSE_BODY_BYTES,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn build(
        adapter_id: String,
        origin: String,
        allow_loopback_http: bool,
        connect_timeout: Duration,
        request_timeout: Duration,
        maximum_request_body: usize,
        maximum_response_body: usize,
    ) -> Result<Self, String> {
        validate_adapter_id(&adapter_id)?;
        let origin = validate_origin(&origin, allow_loopback_http)?;
        if connect_timeout.is_zero()
            || request_timeout.is_zero()
            || connect_timeout > request_timeout
            || maximum_request_body == 0
            || maximum_response_body == 0
        {
            return Err("sponsorship adapter transport bounds are invalid".into());
        }
        let client = reqwest::Client::builder()
            .https_only(!allow_loopback_http)
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(connect_timeout)
            .timeout(request_timeout)
            .build()
            .map_err(|_| "sponsorship adapter HTTP client could not be constructed".to_string())?;
        Ok(Self {
            adapter_id,
            origin,
            client,
            maximum_request_body,
            maximum_response_body,
        })
    }

    async fn execute<Q: Serialize>(
        &self,
        path: &'static str,
        kind: &'static str,
        request_id: &str,
        request: Q,
    ) -> Result<HttpResponse, HttpFailure> {
        let envelope = RequestEnvelope {
            version: PROTOCOL_VERSION,
            adapter_id: &self.adapter_id,
            kind,
            request_id,
            request,
        };
        let body = serde_json::to_vec(&envelope).map_err(|_| HttpFailure::InvalidRequest)?;
        if body.len() > self.maximum_request_body {
            return Err(HttpFailure::InvalidRequest);
        }

        let adapter_id =
            HeaderValue::from_str(&self.adapter_id).map_err(|_| HttpFailure::InvalidRequest)?;
        let request_id =
            HeaderValue::from_str(request_id).map_err(|_| HttpFailure::InvalidRequest)?;
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("identity"));
        headers.insert(ADAPTER_ID_HEADER, adapter_id);
        headers.insert(REQUEST_ID_HEADER, request_id.clone());
        headers.insert(IDEMPOTENCY_KEY_HEADER, request_id);

        let url = format!("{}{path}", self.origin);
        let response = self
            .client
            .post(url)
            .headers(headers)
            .body(body)
            .send()
            .await
            .map_err(classify_request_error)?;
        let status = response.status();
        let exact_json_content_type = response.headers().get_all(CONTENT_TYPE).iter().count() == 1
            && response
                .headers()
                .get(CONTENT_TYPE)
                .is_some_and(|value| value.as_bytes() == b"application/json")
            && !response.headers().contains_key(CONTENT_ENCODING);

        let mut body = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| HttpFailure::ResponseBody(status))?;
            let next_size = body
                .len()
                .checked_add(chunk.len())
                .ok_or(HttpFailure::ResponseBody(status))?;
            if next_size > self.maximum_response_body {
                return Err(HttpFailure::ResponseBody(status));
            }
            body.extend_from_slice(&chunk);
        }
        Ok(HttpResponse {
            status,
            exact_json_content_type,
            body,
        })
    }

    fn decode<T: DeserializeOwned>(
        &self,
        response: HttpResponse,
        expected_kind: &'static str,
    ) -> Result<WireOutcome<T>, ()> {
        if !response.exact_json_content_type {
            return Err(());
        }
        let envelope: ResponseEnvelope<T> =
            serde_json::from_slice(&response.body).map_err(|_| ())?;
        if envelope.version != PROTOCOL_VERSION
            || envelope.adapter_id != self.adapter_id
            || envelope.kind != expected_kind
        {
            return Err(());
        }
        match (response.status, envelope.status, envelope.result) {
            (StatusCode::OK, ResponseStatus::Confirmed, ResultField::Present(result)) => {
                Ok(WireOutcome::Confirmed(result))
            }
            (StatusCode::ACCEPTED, ResponseStatus::Pending, ResultField::Missing) => {
                Ok(WireOutcome::Pending)
            }
            (status, ResponseStatus::Rejected, ResultField::Missing)
                if status.is_client_error() && status != StatusCode::TOO_MANY_REQUESTS =>
            {
                Ok(WireOutcome::Rejected)
            }
            (
                StatusCode::NOT_FOUND | StatusCode::CONFLICT | StatusCode::UNPROCESSABLE_ENTITY,
                ResponseStatus::Unsupported,
                ResultField::Missing,
            ) => Ok(WireOutcome::Unsupported),
            (StatusCode::UNPROCESSABLE_ENTITY, ResponseStatus::Malformed, ResultField::Missing) => {
                Ok(WireOutcome::Malformed)
            }
            _ => Err(()),
        }
    }

    async fn provider_call<Q: Serialize, R: DeserializeOwned>(
        &self,
        path: &'static str,
        kind: &'static str,
        request_id: &str,
        request: Q,
        mutation: bool,
    ) -> Result<WireOutcome<R>, SponsoredProviderError> {
        let response = match self.execute(path, kind, request_id, request).await {
            Ok(response) => response,
            Err(HttpFailure::InvalidRequest) => return Err(SponsoredProviderError::Rejected),
            Err(HttpFailure::Connect) => return Err(SponsoredProviderError::Unavailable),
            Err(HttpFailure::Timeout | HttpFailure::Transport) => {
                return Err(if mutation {
                    SponsoredProviderError::Ambiguous
                } else {
                    SponsoredProviderError::Unavailable
                })
            }
            Err(HttpFailure::ResponseBody(status)) => {
                if status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
                    return Err(if mutation {
                        SponsoredProviderError::Ambiguous
                    } else {
                        SponsoredProviderError::Unavailable
                    });
                }
                return Err(SponsoredProviderError::MalformedEvidence);
            }
        };
        if response.status == StatusCode::TOO_MANY_REQUESTS || response.status.is_server_error() {
            return Err(if mutation {
                SponsoredProviderError::Ambiguous
            } else {
                SponsoredProviderError::Unavailable
            });
        }
        if response.status.is_redirection() {
            return Err(SponsoredProviderError::MalformedEvidence);
        }
        self.decode(response, kind)
            .map_err(|_| SponsoredProviderError::MalformedEvidence)
    }

    async fn billing_call<Q: Serialize, R: DeserializeOwned>(
        &self,
        path: &'static str,
        kind: &'static str,
        request_id: &str,
        request: Q,
    ) -> Result<WireOutcome<R>, SponsoredBillingError> {
        let response = match self.execute(path, kind, request_id, request).await {
            Ok(response) => response,
            Err(HttpFailure::Connect | HttpFailure::InvalidRequest) => {
                return Err(SponsoredBillingError::Unavailable)
            }
            Err(HttpFailure::Timeout | HttpFailure::Transport) => {
                return Err(SponsoredBillingError::Ambiguous)
            }
            Err(HttpFailure::ResponseBody(status)) => {
                return Err(
                    if status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
                        SponsoredBillingError::Unavailable
                    } else {
                        SponsoredBillingError::Ambiguous
                    },
                )
            }
        };
        if response.status == StatusCode::TOO_MANY_REQUESTS || response.status.is_server_error() {
            return Err(SponsoredBillingError::Unavailable);
        }
        self.decode(response, kind)
            .map_err(|_| SponsoredBillingError::Unavailable)
    }

    async fn pricing_call<Q: Serialize, R: DeserializeOwned>(
        &self,
        path: &'static str,
        kind: &'static str,
        request_id: &str,
        request: Q,
    ) -> Result<WireOutcome<R>, SponsoredPricingError> {
        let response = match self.execute(path, kind, request_id, request).await {
            Ok(response) => response,
            Err(HttpFailure::Connect | HttpFailure::Timeout | HttpFailure::Transport) => {
                return Err(SponsoredPricingError::Unavailable)
            }
            Err(HttpFailure::InvalidRequest) => return Err(SponsoredPricingError::Malformed),
            Err(HttpFailure::ResponseBody(status)) => {
                return Err(
                    if status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
                        SponsoredPricingError::Unavailable
                    } else {
                        SponsoredPricingError::Malformed
                    },
                )
            }
        };
        if response.status == StatusCode::TOO_MANY_REQUESTS || response.status.is_server_error() {
            return Err(SponsoredPricingError::Unavailable);
        }
        self.decode(response, kind)
            .map_err(|_| SponsoredPricingError::Malformed)
    }
}

fn classify_request_error(error: reqwest::Error) -> HttpFailure {
    if error.is_connect() {
        HttpFailure::Connect
    } else if error.is_timeout() {
        HttpFailure::Timeout
    } else {
        HttpFailure::Transport
    }
}

fn validate_adapter_id(adapter_id: &str) -> Result<(), String> {
    if adapter_id.is_empty()
        || adapter_id.len() > MAX_ADAPTER_ID_BYTES
        || !adapter_id.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':' | b'/')
        })
    {
        return Err("sponsorship adapter ID must contain 1 to 128 safe ASCII bytes".into());
    }
    Ok(())
}

fn validate_origin(raw: &str, allow_loopback_http: bool) -> Result<String, String> {
    let parsed = Url::parse(raw)
        .map_err(|_| "sponsorship adapter origin must be one canonical absolute origin")?;
    let canonical = parsed.origin().ascii_serialization();
    let production_https = parsed.scheme() == "https";
    let test_loopback_http = allow_loopback_http
        && parsed.scheme() == "http"
        && parsed.host_str().is_some_and(|host| {
            host == "localhost"
                || host
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|address| address.is_loopback())
        });
    if raw.len() > MAX_ORIGIN_BYTES
        || (!production_https && !test_loopback_http)
        || parsed.host_str().is_none()
        || parsed.username() != ""
        || parsed.password().is_some()
        || parsed.path() != "/"
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || raw != canonical
    {
        return Err(
            "sponsorship adapter origin must be canonical and contain no credentials, path, query, or fragment"
                .into(),
        );
    }
    Ok(canonical)
}

#[derive(Debug)]
enum HttpFailure {
    InvalidRequest,
    Connect,
    Timeout,
    Transport,
    ResponseBody(StatusCode),
}

struct HttpResponse {
    status: StatusCode,
    exact_json_content_type: bool,
    body: Vec<u8>,
}

#[derive(Serialize)]
struct RequestEnvelope<'a, Q> {
    version: u32,
    adapter_id: &'a str,
    kind: &'static str,
    request_id: &'a str,
    request: Q,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResponseEnvelope<T> {
    version: u32,
    adapter_id: String,
    kind: String,
    status: ResponseStatus,
    #[serde(default = "missing_result")]
    result: ResultField<T>,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum ResponseStatus {
    Confirmed,
    Rejected,
    Pending,
    Unsupported,
    Malformed,
}

enum ResultField<T> {
    Missing,
    Present(T),
}

fn missing_result<T>() -> ResultField<T> {
    ResultField::Missing
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for ResultField<T> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        T::deserialize(deserializer).map(Self::Present)
    }
}

enum WireOutcome<T> {
    Confirmed(T),
    Rejected,
    Pending,
    Unsupported,
    Malformed,
}

fn derived_request_id<T: Serialize>(domain: &'static str, request: &T) -> Result<String, ()> {
    let canonical = serde_json::to_vec(&(domain, request)).map_err(|_| ())?;
    let digest = Sha256::digest(canonical);
    let mut request_id = String::with_capacity(19 + digest.len() * 2);
    request_id.push_str("cma-sponsorship-v1-");
    for byte in digest {
        write!(&mut request_id, "{byte:02x}").map_err(|_| ())?;
    }
    Ok(request_id)
}

fn quota_is_positive(quota: SponsoredQuota) -> bool {
    quota.requests > 0 && quota.input_tokens > 0 && quota.output_tokens > 0
}

fn quota_is_nonzero(quota: SponsoredQuota) -> bool {
    quota.requests > 0 || quota.input_tokens > 0 || quota.output_tokens > 0
}

fn valid_timestamp(value: &str) -> bool {
    seconds_from_rfc3339(value).is_some()
}

fn state_credential(state: SponsoredCredentialState) -> &'static str {
    match state {
        SponsoredCredentialState::Active => "active",
        SponsoredCredentialState::RotationPending => "rotation_pending",
        SponsoredCredentialState::RevocationPending => "revocation_pending",
        SponsoredCredentialState::Revoked => "revoked",
    }
}

fn state_material(state: SponsoredCredentialMaterialState) -> &'static str {
    match state {
        SponsoredCredentialMaterialState::Retained => "retained",
        SponsoredCredentialMaterialState::Retired => "retired",
    }
}

fn state_policy(state: SponsorshipPolicyState) -> &'static str {
    match state {
        SponsorshipPolicyState::Active => "active",
        SponsorshipPolicyState::Suspended => "suspended",
        SponsorshipPolicyState::Revoked => "revoked",
    }
}

fn state_grant(state: SponsoredGrantState) -> &'static str {
    match state {
        SponsoredGrantState::Active => "active",
        SponsoredGrantState::Exhausted => "exhausted",
        SponsoredGrantState::Expired => "expired",
        SponsoredGrantState::RevocationPending => "revocation_pending",
        SponsoredGrantState::Revoked => "revoked",
        SponsoredGrantState::Closed => "closed",
        SponsoredGrantState::Suspended => "suspended",
    }
}

fn operation_kind(kind: ProviderOperationKind) -> &'static str {
    match kind {
        ProviderOperationKind::RotateCredential => "rotate_credential",
        ProviderOperationKind::RevokeCredential => "revoke_credential",
        ProviderOperationKind::RevokeGrant => "revoke_grant",
        ProviderOperationKind::CloseGrant => "close_grant",
        ProviderOperationKind::RevokeUncommittedGrant => "revoke_uncommitted_grant",
    }
}

fn operation_phase(phase: ProviderOperationPhase) -> &'static str {
    match phase {
        ProviderOperationPhase::Prepared => "prepared",
        ProviderOperationPhase::Attempting => "attempting",
        ProviderOperationPhase::Ambiguous => "ambiguous",
        ProviderOperationPhase::Rejected => "rejected",
    }
}

#[derive(Serialize)]
struct PendingOperationWire<'a> {
    kind: &'static str,
    phase: &'static str,
    provider_request_id: &'a str,
    client_request_id_digest: &'a str,
    payload_digest: &'a str,
    requested_at: &'a str,
    attempted_at: Option<&'a str>,
}

impl<'a> From<&'a PendingProviderOperation> for PendingOperationWire<'a> {
    fn from(operation: &'a PendingProviderOperation) -> Self {
        Self {
            kind: operation_kind(operation.kind),
            phase: operation_phase(operation.phase),
            provider_request_id: &operation.provider_request_id,
            client_request_id_digest: &operation.client_request_id_digest,
            payload_digest: &operation.payload_digest,
            requested_at: &operation.requested_at,
            attempted_at: operation.attempted_at.as_deref(),
        }
    }
}

#[derive(Serialize)]
struct CredentialWire<'a> {
    schema_version: u32,
    credential_id: &'a str,
    owner: &'a SponsorBillingOwner,
    provider: &'a str,
    provider_policy_key: &'a str,
    provider_policy_handle: &'a str,
    provider_credential_id_digest: &'a str,
    provider_state_version: u64,
    ownership_verified_at: &'a str,
    supports_expiry: bool,
    supports_revocation: bool,
    state: &'static str,
    material_state: &'static str,
    generation: u64,
    created_at: &'a str,
    updated_at: &'a str,
    revoked_at: Option<&'a str>,
    pending_operation: Option<PendingOperationWire<'a>>,
    superseded_operation: Option<PendingOperationWire<'a>>,
}

impl<'a> From<&'a SponsoredCredentialRecord> for CredentialWire<'a> {
    fn from(credential: &'a SponsoredCredentialRecord) -> Self {
        Self {
            schema_version: credential.schema_version,
            credential_id: &credential.id.0,
            owner: &credential.owner,
            provider: &credential.authority.provider,
            provider_policy_key: &credential.authority.provider_policy_key,
            provider_policy_handle: credential
                .authority
                .provider_policy_handle
                .expose_for_provider_or_encryption(),
            provider_credential_id_digest: &credential.authority.provider_credential_id_digest,
            provider_state_version: credential.authority.state_version,
            ownership_verified_at: &credential.authority.ownership_verified_at,
            supports_expiry: credential.authority.supports_expiry,
            supports_revocation: credential.authority.supports_revocation,
            state: state_credential(credential.state),
            material_state: state_material(credential.material_state),
            generation: credential.generation,
            created_at: &credential.created_at,
            updated_at: &credential.updated_at,
            revoked_at: credential.revoked_at.as_deref(),
            pending_operation: credential.pending_operation.as_ref().map(Into::into),
            superseded_operation: credential.superseded_operation.as_ref().map(Into::into),
        }
    }
}

#[derive(Serialize)]
struct PolicyWire<'a> {
    schema_version: u32,
    policy_id: &'a str,
    tenant_id: &'a str,
    owner: &'a SponsorBillingOwner,
    credential_id: &'a str,
    provider: &'a str,
    provider_policy_key: &'a str,
    allowed_models: &'a BTreeSet<String>,
    allowed_resources: &'a BTreeSet<String>,
    per_launch_quota: SponsoredQuota,
    aggregate_quota: SponsoredQuota,
    per_launch_budget_micros: u64,
    aggregate_budget_micros: u64,
    max_requests_per_minute: u32,
    max_concurrent_launches: u32,
    expires_at: &'a str,
    state: &'static str,
    generation: u64,
    created_at: &'a str,
    updated_at: &'a str,
    revoked_at: Option<&'a str>,
}

impl<'a> From<&'a SponsorshipPolicy> for PolicyWire<'a> {
    fn from(policy: &'a SponsorshipPolicy) -> Self {
        Self {
            schema_version: policy.schema_version,
            policy_id: &policy.id.0,
            tenant_id: &policy.tenant_id,
            owner: &policy.owner,
            credential_id: &policy.credential_id.0,
            provider: &policy.provider_policy.provider,
            provider_policy_key: &policy.provider_policy.provider_policy_key,
            allowed_models: &policy.provider_policy.allowed_models,
            allowed_resources: &policy.provider_policy.allowed_resources,
            per_launch_quota: policy.per_launch_quota,
            aggregate_quota: policy.aggregate_quota,
            per_launch_budget_micros: policy.per_launch_budget_micros,
            aggregate_budget_micros: policy.aggregate_budget_micros,
            max_requests_per_minute: policy.max_requests_per_minute,
            max_concurrent_launches: policy.max_concurrent_launches,
            expires_at: &policy.expires_at,
            state: state_policy(policy.state),
            generation: policy.generation,
            created_at: &policy.created_at,
            updated_at: &policy.updated_at,
            revoked_at: policy.revoked_at.as_deref(),
        }
    }
}

#[derive(Serialize)]
struct GrantWire<'a> {
    schema_version: u32,
    grant_id: &'a str,
    tenant_id: &'a str,
    policy_id: &'a str,
    policy_generation: u64,
    credential_generation: u64,
    trigger_revision_id: &'a str,
    trigger_revision_digest: &'a str,
    launch_id: &'a str,
    session_id: &'a str,
    contributor: &'a str,
    sponsor_subject: &'a str,
    terms_digest: &'a str,
    state: &'static str,
    allocation: &'a SponsoredUsageSummary,
    usage: &'a SponsoredUsageSummary,
    generation: u64,
    expires_at: &'a str,
    created_at: &'a str,
    updated_at: &'a str,
    revoked_at: Option<&'a str>,
}

impl<'a> From<&'a SponsoredGrant> for GrantWire<'a> {
    fn from(grant: &'a SponsoredGrant) -> Self {
        Self {
            schema_version: grant.schema_version,
            grant_id: &grant.id.0,
            tenant_id: &grant.tenant_id,
            policy_id: &grant.policy_id.0,
            policy_generation: grant.policy_generation,
            credential_generation: grant.credential_generation,
            trigger_revision_id: &grant.trigger_revision_id.0,
            trigger_revision_digest: &grant.trigger_revision_digest,
            launch_id: &grant.launch_id.0,
            session_id: &grant.session_id.0,
            contributor: &grant.contributor.0,
            sponsor_subject: &grant.sponsor_subject.0,
            terms_digest: &grant.terms_digest,
            state: state_grant(grant.state),
            allocation: &grant.allocation,
            usage: &grant.usage,
            generation: grant.generation,
            expires_at: &grant.authority.expires_at,
            created_at: &grant.created_at,
            updated_at: &grant.updated_at,
            revoked_at: grant.revoked_at.as_deref(),
        }
    }
}

#[derive(Serialize)]
struct ReceiptWire<'a> {
    provider_grant_id: &'a str,
    provider_state_version: u64,
    expires_at: &'a str,
    revocation_handle: &'a str,
    issued_at: &'a str,
}

impl<'a> From<&'a ProviderGrantReceipt> for ReceiptWire<'a> {
    fn from(receipt: &'a ProviderGrantReceipt) -> Self {
        Self {
            provider_grant_id: receipt
                .provider_grant_id
                .expose_for_provider_or_encryption(),
            provider_state_version: receipt.provider_state_version,
            expires_at: &receipt.expires_at,
            revocation_handle: receipt
                .revocation_handle
                .expose_for_provider_or_encryption(),
            issued_at: &receipt.issued_at,
        }
    }
}

#[derive(Serialize)]
struct OwnershipRequest<'a> {
    owner: &'a SponsorBillingOwner,
    provider: &'a str,
    provider_policy_key: &'a str,
    credential_secret: &'a str,
}

#[derive(Serialize)]
struct OwnershipRequestIdentity<'a> {
    owner: &'a SponsorBillingOwner,
    provider: &'a str,
    provider_policy_key: &'a str,
}

#[derive(Serialize)]
struct GrantIssueRequest<'a> {
    credential: CredentialWire<'a>,
    credential_secret: &'a str,
    policy: PolicyWire<'a>,
    launch_id: &'a str,
    session_id: &'a str,
}

#[derive(Serialize)]
struct UncommittedGrantRevocationRequest<'a> {
    receipt: ReceiptWire<'a>,
    credential: CredentialWire<'a>,
    credential_secret: &'a str,
}

#[derive(Serialize)]
struct CredentialRotationRequest<'a> {
    credential: CredentialWire<'a>,
    current_credential_secret: &'a str,
    replacement_credential_secret: &'a str,
}

#[derive(Serialize)]
struct CredentialRevocationRequest<'a> {
    credential: CredentialWire<'a>,
    current_credential_secret: &'a str,
    staged_replacement_credential_secret: Option<&'a str>,
    superseded_rotation: Option<PendingOperationWire<'a>>,
}

#[derive(Serialize)]
struct GrantRevocationRequest<'a> {
    grant: GrantWire<'a>,
    receipt: ReceiptWire<'a>,
    credential: CredentialWire<'a>,
    credential_secret: &'a str,
}

#[derive(Serialize)]
struct BillingEligibilityRequest<'a> {
    policy: PolicyWire<'a>,
    launch_id: &'a str,
}

#[derive(Serialize)]
struct QuoteRequest<'a> {
    provider: &'a str,
    model: &'a str,
    resource: &'a str,
    maximum_quota: SponsoredQuota,
    observed_at: &'a str,
}

#[derive(Serialize)]
struct ActualPriceRequest<'a> {
    quote_digest: &'a str,
    pricing_generation: u64,
    provider: &'a str,
    model: &'a str,
    resource: &'a str,
    maximum: &'a SponsoredUsageSummary,
    actual_quota: SponsoredQuota,
    observed_at: &'a str,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnershipResult {
    evidence: EvidenceResult,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RotationResult {
    credential_id: String,
    previous_generation: u64,
    evidence: EvidenceResult,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EvidenceResult {
    provider: String,
    provider_policy_key: String,
    provider_policy_handle: String,
    provider_credential_id: String,
    state_version: u64,
    ownership_verified_at: String,
    supports_expiry: bool,
    supports_revocation: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GrantReceiptResult {
    credential_id: String,
    credential_generation: u64,
    policy_id: String,
    policy_generation: u64,
    launch_id: String,
    session_id: String,
    receipt: ReceiptResult,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReceiptResult {
    provider_grant_id: String,
    provider_state_version: u64,
    expires_at: String,
    revocation_handle: String,
    issued_at: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EmptyResult {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BillingResult {
    policy_id: String,
    policy_generation: u64,
    launch_id: String,
    authorization: BillingResultAuthorization,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum BillingResultAuthorization {
    Confirmed,
    Rejected,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct QuotaResult {
    requests: u64,
    input_tokens: u64,
    output_tokens: u64,
}

impl From<QuotaResult> for SponsoredQuota {
    fn from(quota: QuotaResult) -> Self {
        Self {
            requests: quota.requests,
            input_tokens: quota.input_tokens,
            output_tokens: quota.output_tokens,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UsageResult {
    quota: QuotaResult,
    spend_micros: u64,
}

impl From<UsageResult> for SponsoredUsageSummary {
    fn from(usage: UsageResult) -> Self {
        Self {
            quota: usage.quota.into(),
            spend_micros: usage.spend_micros,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct QuoteResult {
    provider: String,
    model: String,
    resource: String,
    maximum: UsageResult,
    pricing_generation: u64,
    digest: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ActualPriceResult {
    quote_digest: String,
    pricing_generation: u64,
    actual: UsageResult,
}

fn evidence_from_result(
    result: EvidenceResult,
    provider: &str,
    provider_policy_key: &str,
    previous_state_version: Option<u64>,
) -> Result<ProviderCredentialEvidence, SponsoredProviderError> {
    if result.provider != provider
        || result.provider_policy_key != provider_policy_key
        || result.state_version == 0
        || previous_state_version.is_some_and(|version| result.state_version <= version)
        || !valid_timestamp(&result.ownership_verified_at)
        || !result.supports_expiry
        || !result.supports_revocation
    {
        return Err(SponsoredProviderError::MalformedEvidence);
    }
    let provider_policy_handle = SensitiveProviderValue::new(result.provider_policy_handle)
        .map_err(|_| SponsoredProviderError::MalformedEvidence)?;
    let provider_credential_id = SensitiveProviderValue::new(result.provider_credential_id)
        .map_err(|_| SponsoredProviderError::MalformedEvidence)?;
    Ok(ProviderCredentialEvidence {
        provider: result.provider,
        provider_policy_key: result.provider_policy_key,
        provider_policy_handle,
        provider_credential_id,
        state_version: result.state_version,
        ownership_verified_at: result.ownership_verified_at,
        supports_expiry: result.supports_expiry,
        supports_revocation: result.supports_revocation,
    })
}

fn receipt_from_result(
    result: GrantReceiptResult,
    credential: &SponsoredCredentialRecord,
    policy: &SponsorshipPolicy,
    launch_id: &LaunchId,
    session_id: &SessionId,
) -> Result<ProviderGrantReceipt, SponsoredProviderError> {
    if result.credential_id != credential.id.0
        || result.credential_generation != credential.generation
        || result.policy_id != policy.id.0
        || result.policy_generation != policy.generation
        || result.launch_id != launch_id.0
        || result.session_id != session_id.0
        || result.receipt.provider_state_version == 0
        || !valid_timestamp(&result.receipt.issued_at)
        || !valid_timestamp(&result.receipt.expires_at)
        || seconds_from_rfc3339(&result.receipt.issued_at)
            .zip(seconds_from_rfc3339(&result.receipt.expires_at))
            .is_none_or(|(issued, expires)| issued >= expires)
        || seconds_from_rfc3339(&result.receipt.expires_at)
            .zip(seconds_from_rfc3339(&policy.expires_at))
            .is_none_or(|(expires, policy_expires)| expires > policy_expires)
    {
        return Err(SponsoredProviderError::MalformedEvidence);
    }
    let provider_grant_id = SensitiveProviderValue::new(result.receipt.provider_grant_id)
        .map_err(|_| SponsoredProviderError::MalformedEvidence)?;
    let revocation_handle = SensitiveProviderValue::new(result.receipt.revocation_handle)
        .map_err(|_| SponsoredProviderError::MalformedEvidence)?;
    Ok(ProviderGrantReceipt {
        provider_grant_id,
        provider_state_version: result.receipt.provider_state_version,
        expires_at: result.receipt.expires_at,
        revocation_handle,
        issued_at: result.receipt.issued_at,
    })
}

fn billing_from_result(
    result: BillingResult,
    policy: &SponsorshipPolicy,
    launch_id: &LaunchId,
) -> Result<BillingAuthorization, SponsoredBillingError> {
    if result.policy_id != policy.id.0
        || result.policy_generation != policy.generation
        || result.launch_id != launch_id.0
    {
        return Err(SponsoredBillingError::Unavailable);
    }
    Ok(match result.authorization {
        BillingResultAuthorization::Confirmed => BillingAuthorization::Confirmed,
        BillingResultAuthorization::Rejected => BillingAuthorization::Rejected,
    })
}

fn valid_policy_token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':' | b'/')
        })
}

fn valid_receipt_input(receipt: &ProviderGrantReceipt) -> bool {
    receipt.provider_state_version > 0
        && valid_timestamp(&receipt.issued_at)
        && valid_timestamp(&receipt.expires_at)
        && seconds_from_rfc3339(&receipt.issued_at)
            .zip(seconds_from_rfc3339(&receipt.expires_at))
            .is_some_and(|(issued, expires)| issued < expires)
}

fn provider_confirmed_unit(
    outcome: WireOutcome<EmptyResult>,
    reconciliation: bool,
) -> Result<ProviderOperationResolution<()>, SponsoredProviderError> {
    match outcome {
        WireOutcome::Confirmed(_) => Ok(ProviderOperationResolution::Confirmed(())),
        WireOutcome::Rejected => Ok(ProviderOperationResolution::Rejected),
        WireOutcome::Pending if reconciliation => Ok(ProviderOperationResolution::Pending),
        WireOutcome::Pending => Err(SponsoredProviderError::Ambiguous),
        WireOutcome::Unsupported | WireOutcome::Malformed => {
            Err(SponsoredProviderError::MalformedEvidence)
        }
    }
}

/// Credential-provider adapter using the sponsorship HTTP v1 contract.
#[derive(Clone, Debug)]
pub struct HttpSponsoredCredentialProvider {
    http: HttpAdapter,
}

impl HttpSponsoredCredentialProvider {
    pub fn new(adapter_id: String, origin: String) -> Result<Self, String> {
        Ok(Self {
            http: HttpAdapter::production(adapter_id, origin)?,
        })
    }

    #[cfg(test)]
    fn fixture(
        adapter_id: &str,
        origin: String,
        timeout: Duration,
        maximum_request_body: usize,
        maximum_response_body: usize,
    ) -> Self {
        Self {
            http: HttpAdapter::build(
                adapter_id.into(),
                origin,
                true,
                timeout.min(Duration::from_millis(50)),
                timeout,
                maximum_request_body,
                maximum_response_body,
            )
            .expect("fixture adapter configuration is valid"),
        }
    }
}

#[async_trait]
impl SponsoredCredentialProvider for HttpSponsoredCredentialProvider {
    async fn validate_ownership(
        &self,
        owner: &SponsorBillingOwner,
        provider: &str,
        provider_policy_key: &str,
        credential: &ProviderCredentialSecret,
    ) -> Result<ProviderCredentialEvidence, SponsoredProviderError> {
        if !valid_policy_token(provider) || !valid_policy_token(provider_policy_key) {
            return Err(SponsoredProviderError::Rejected);
        }
        let identity = OwnershipRequestIdentity {
            owner,
            provider,
            provider_policy_key,
        };
        let request_id = derived_request_id("credential.validate_ownership", &identity)
            .map_err(|_| SponsoredProviderError::Rejected)?;
        let request = OwnershipRequest {
            owner,
            provider,
            provider_policy_key,
            credential_secret: credential.expose(),
        };
        match self
            .http
            .provider_call::<_, OwnershipResult>(
                OWNERSHIP_PATH,
                OWNERSHIP_KIND,
                &request_id,
                request,
                false,
            )
            .await?
        {
            WireOutcome::Confirmed(result) => {
                evidence_from_result(result.evidence, provider, provider_policy_key, None)
            }
            WireOutcome::Rejected => Err(SponsoredProviderError::Rejected),
            WireOutcome::Pending | WireOutcome::Unsupported | WireOutcome::Malformed => {
                Err(SponsoredProviderError::MalformedEvidence)
            }
        }
    }

    async fn issue_grant(
        &self,
        stable_request_id: &str,
        credential: &SponsoredCredentialRecord,
        secret: &ProviderCredentialSecret,
        policy: &SponsorshipPolicy,
        launch_id: &LaunchId,
        session_id: &SessionId,
    ) -> Result<ProviderGrantReceipt, SponsoredProviderError> {
        credential
            .validate()
            .and_then(|_| policy.validate())
            .map_err(|_| SponsoredProviderError::Rejected)?;
        let request = GrantIssueRequest {
            credential: credential.into(),
            credential_secret: secret.expose(),
            policy: policy.into(),
            launch_id: &launch_id.0,
            session_id: &session_id.0,
        };
        match self
            .http
            .provider_call::<_, GrantReceiptResult>(
                ISSUE_GRANT_PATH,
                ISSUE_GRANT_KIND,
                stable_request_id,
                request,
                true,
            )
            .await?
        {
            WireOutcome::Confirmed(result) => {
                receipt_from_result(result, credential, policy, launch_id, session_id)
            }
            WireOutcome::Rejected => Err(SponsoredProviderError::Rejected),
            WireOutcome::Pending => Err(SponsoredProviderError::Ambiguous),
            WireOutcome::Unsupported | WireOutcome::Malformed => {
                Err(SponsoredProviderError::MalformedEvidence)
            }
        }
    }

    async fn reconcile_grant_issue(
        &self,
        stable_request_id: &str,
        credential: &SponsoredCredentialRecord,
        secret: &ProviderCredentialSecret,
        policy: &SponsorshipPolicy,
        launch_id: &LaunchId,
        session_id: &SessionId,
    ) -> Result<ProviderOperationResolution<ProviderGrantReceipt>, SponsoredProviderError> {
        credential
            .validate()
            .and_then(|_| policy.validate())
            .map_err(|_| SponsoredProviderError::Rejected)?;
        let request = GrantIssueRequest {
            credential: credential.into(),
            credential_secret: secret.expose(),
            policy: policy.into(),
            launch_id: &launch_id.0,
            session_id: &session_id.0,
        };
        match self
            .http
            .provider_call::<_, GrantReceiptResult>(
                RECONCILE_GRANT_ISSUE_PATH,
                RECONCILE_GRANT_ISSUE_KIND,
                stable_request_id,
                request,
                false,
            )
            .await?
        {
            WireOutcome::Confirmed(result) => Ok(ProviderOperationResolution::Confirmed(
                receipt_from_result(result, credential, policy, launch_id, session_id)?,
            )),
            WireOutcome::Rejected => Ok(ProviderOperationResolution::Rejected),
            WireOutcome::Pending => Ok(ProviderOperationResolution::Pending),
            WireOutcome::Unsupported | WireOutcome::Malformed => {
                Err(SponsoredProviderError::MalformedEvidence)
            }
        }
    }

    async fn revoke_uncommitted_grant(
        &self,
        stable_request_id: &str,
        receipt: &ProviderGrantReceipt,
        credential: &SponsoredCredentialRecord,
        secret: &ProviderCredentialSecret,
    ) -> Result<(), SponsoredProviderError> {
        credential
            .validate()
            .map_err(|_| SponsoredProviderError::Rejected)?;
        if !valid_receipt_input(receipt) {
            return Err(SponsoredProviderError::Rejected);
        }
        let request = UncommittedGrantRevocationRequest {
            receipt: receipt.into(),
            credential: credential.into(),
            credential_secret: secret.expose(),
        };
        match provider_confirmed_unit(
            self.http
                .provider_call(
                    REVOKE_UNCOMMITTED_GRANT_PATH,
                    REVOKE_UNCOMMITTED_GRANT_KIND,
                    stable_request_id,
                    request,
                    true,
                )
                .await?,
            false,
        )? {
            ProviderOperationResolution::Confirmed(()) => Ok(()),
            ProviderOperationResolution::Rejected => Err(SponsoredProviderError::Rejected),
            ProviderOperationResolution::Pending => Err(SponsoredProviderError::Ambiguous),
        }
    }

    async fn reconcile_uncommitted_grant_revocation(
        &self,
        stable_request_id: &str,
        receipt: &ProviderGrantReceipt,
        credential: &SponsoredCredentialRecord,
        secret: &ProviderCredentialSecret,
    ) -> Result<ProviderOperationResolution<()>, SponsoredProviderError> {
        credential
            .validate()
            .map_err(|_| SponsoredProviderError::Rejected)?;
        if !valid_receipt_input(receipt) {
            return Err(SponsoredProviderError::Rejected);
        }
        let request = UncommittedGrantRevocationRequest {
            receipt: receipt.into(),
            credential: credential.into(),
            credential_secret: secret.expose(),
        };
        provider_confirmed_unit(
            self.http
                .provider_call(
                    RECONCILE_UNCOMMITTED_GRANT_REVOCATION_PATH,
                    RECONCILE_UNCOMMITTED_GRANT_REVOCATION_KIND,
                    stable_request_id,
                    request,
                    false,
                )
                .await?,
            true,
        )
    }

    async fn rotate_credential(
        &self,
        stable_request_id: &str,
        credential: &SponsoredCredentialRecord,
        current_secret: &ProviderCredentialSecret,
        replacement_secret: &ProviderCredentialSecret,
    ) -> Result<ProviderCredentialEvidence, SponsoredProviderError> {
        credential
            .validate()
            .map_err(|_| SponsoredProviderError::Rejected)?;
        let request = CredentialRotationRequest {
            credential: credential.into(),
            current_credential_secret: current_secret.expose(),
            replacement_credential_secret: replacement_secret.expose(),
        };
        match self
            .http
            .provider_call::<_, RotationResult>(
                ROTATE_CREDENTIAL_PATH,
                ROTATE_CREDENTIAL_KIND,
                stable_request_id,
                request,
                true,
            )
            .await?
        {
            WireOutcome::Confirmed(result)
                if result.credential_id == credential.id.0
                    && result.previous_generation == credential.generation =>
            {
                evidence_from_result(
                    result.evidence,
                    &credential.authority.provider,
                    &credential.authority.provider_policy_key,
                    Some(credential.authority.state_version),
                )
            }
            WireOutcome::Confirmed(_) => Err(SponsoredProviderError::MalformedEvidence),
            WireOutcome::Rejected => Err(SponsoredProviderError::Rejected),
            WireOutcome::Pending => Err(SponsoredProviderError::Ambiguous),
            WireOutcome::Unsupported | WireOutcome::Malformed => {
                Err(SponsoredProviderError::MalformedEvidence)
            }
        }
    }

    async fn reconcile_credential_rotation(
        &self,
        stable_request_id: &str,
        credential: &SponsoredCredentialRecord,
        current_secret: &ProviderCredentialSecret,
        replacement_secret: &ProviderCredentialSecret,
    ) -> Result<ProviderOperationResolution<ProviderCredentialEvidence>, SponsoredProviderError>
    {
        credential
            .validate()
            .map_err(|_| SponsoredProviderError::Rejected)?;
        let request = CredentialRotationRequest {
            credential: credential.into(),
            current_credential_secret: current_secret.expose(),
            replacement_credential_secret: replacement_secret.expose(),
        };
        match self
            .http
            .provider_call::<_, RotationResult>(
                RECONCILE_CREDENTIAL_ROTATION_PATH,
                RECONCILE_CREDENTIAL_ROTATION_KIND,
                stable_request_id,
                request,
                false,
            )
            .await?
        {
            WireOutcome::Confirmed(result)
                if result.credential_id == credential.id.0
                    && result.previous_generation == credential.generation =>
            {
                Ok(ProviderOperationResolution::Confirmed(
                    evidence_from_result(
                        result.evidence,
                        &credential.authority.provider,
                        &credential.authority.provider_policy_key,
                        Some(credential.authority.state_version),
                    )?,
                ))
            }
            WireOutcome::Confirmed(_) => Err(SponsoredProviderError::MalformedEvidence),
            WireOutcome::Rejected => Ok(ProviderOperationResolution::Rejected),
            WireOutcome::Pending => Ok(ProviderOperationResolution::Pending),
            WireOutcome::Unsupported | WireOutcome::Malformed => {
                Err(SponsoredProviderError::MalformedEvidence)
            }
        }
    }

    async fn revoke_credential(
        &self,
        stable_request_id: &str,
        credential: &SponsoredCredentialRecord,
        secret: &ProviderCredentialSecret,
        staged_replacement: Option<&ProviderCredentialSecret>,
        superseded_rotation: Option<&PendingProviderOperation>,
    ) -> Result<(), SponsoredProviderError> {
        credential
            .validate()
            .map_err(|_| SponsoredProviderError::Rejected)?;
        if superseded_rotation.is_some_and(|operation| operation.validate().is_err()) {
            return Err(SponsoredProviderError::Rejected);
        }
        let request = CredentialRevocationRequest {
            credential: credential.into(),
            current_credential_secret: secret.expose(),
            staged_replacement_credential_secret: staged_replacement.map(|secret| secret.expose()),
            superseded_rotation: superseded_rotation.map(Into::into),
        };
        match provider_confirmed_unit(
            self.http
                .provider_call(
                    REVOKE_CREDENTIAL_PATH,
                    REVOKE_CREDENTIAL_KIND,
                    stable_request_id,
                    request,
                    true,
                )
                .await?,
            false,
        )? {
            ProviderOperationResolution::Confirmed(()) => Ok(()),
            ProviderOperationResolution::Rejected => Err(SponsoredProviderError::Rejected),
            ProviderOperationResolution::Pending => Err(SponsoredProviderError::Ambiguous),
        }
    }

    async fn reconcile_credential_revocation(
        &self,
        stable_request_id: &str,
        credential: &SponsoredCredentialRecord,
        secret: &ProviderCredentialSecret,
        staged_replacement: Option<&ProviderCredentialSecret>,
        superseded_rotation: Option<&PendingProviderOperation>,
    ) -> Result<ProviderOperationResolution<()>, SponsoredProviderError> {
        credential
            .validate()
            .map_err(|_| SponsoredProviderError::Rejected)?;
        if superseded_rotation.is_some_and(|operation| operation.validate().is_err()) {
            return Err(SponsoredProviderError::Rejected);
        }
        let request = CredentialRevocationRequest {
            credential: credential.into(),
            current_credential_secret: secret.expose(),
            staged_replacement_credential_secret: staged_replacement.map(|secret| secret.expose()),
            superseded_rotation: superseded_rotation.map(Into::into),
        };
        provider_confirmed_unit(
            self.http
                .provider_call(
                    RECONCILE_CREDENTIAL_REVOCATION_PATH,
                    RECONCILE_CREDENTIAL_REVOCATION_KIND,
                    stable_request_id,
                    request,
                    false,
                )
                .await?,
            true,
        )
    }

    async fn revoke_grant(
        &self,
        stable_request_id: &str,
        grant: &SponsoredGrant,
        provider_receipt: &ProviderGrantReceipt,
        credential: &SponsoredCredentialRecord,
        secret: &ProviderCredentialSecret,
    ) -> Result<(), SponsoredProviderError> {
        grant
            .validate()
            .and_then(|_| credential.validate())
            .map_err(|_| SponsoredProviderError::Rejected)?;
        if !valid_receipt_input(provider_receipt) {
            return Err(SponsoredProviderError::Rejected);
        }
        let request = GrantRevocationRequest {
            grant: grant.into(),
            receipt: provider_receipt.into(),
            credential: credential.into(),
            credential_secret: secret.expose(),
        };
        match provider_confirmed_unit(
            self.http
                .provider_call(
                    REVOKE_GRANT_PATH,
                    REVOKE_GRANT_KIND,
                    stable_request_id,
                    request,
                    true,
                )
                .await?,
            false,
        )? {
            ProviderOperationResolution::Confirmed(()) => Ok(()),
            ProviderOperationResolution::Rejected => Err(SponsoredProviderError::Rejected),
            ProviderOperationResolution::Pending => Err(SponsoredProviderError::Ambiguous),
        }
    }

    async fn reconcile_grant_revocation(
        &self,
        stable_request_id: &str,
        grant: &SponsoredGrant,
        provider_receipt: &ProviderGrantReceipt,
        credential: &SponsoredCredentialRecord,
        secret: &ProviderCredentialSecret,
    ) -> Result<ProviderOperationResolution<()>, SponsoredProviderError> {
        grant
            .validate()
            .and_then(|_| credential.validate())
            .map_err(|_| SponsoredProviderError::Rejected)?;
        if !valid_receipt_input(provider_receipt) {
            return Err(SponsoredProviderError::Rejected);
        }
        let request = GrantRevocationRequest {
            grant: grant.into(),
            receipt: provider_receipt.into(),
            credential: credential.into(),
            credential_secret: secret.expose(),
        };
        provider_confirmed_unit(
            self.http
                .provider_call(
                    RECONCILE_GRANT_REVOCATION_PATH,
                    RECONCILE_GRANT_REVOCATION_KIND,
                    stable_request_id,
                    request,
                    false,
                )
                .await?,
            true,
        )
    }
}

/// Read-only billing eligibility adapter using the sponsorship HTTP v1 contract.
#[derive(Clone, Debug)]
pub struct HttpSponsoredBillingProvider {
    http: HttpAdapter,
}

impl HttpSponsoredBillingProvider {
    pub fn new(adapter_id: String, origin: String) -> Result<Self, String> {
        Ok(Self {
            http: HttpAdapter::production(adapter_id, origin)?,
        })
    }

    #[cfg(test)]
    fn fixture(adapter_id: &str, origin: String, timeout: Duration) -> Self {
        Self {
            http: HttpAdapter::build(
                adapter_id.into(),
                origin,
                true,
                timeout.min(Duration::from_millis(50)),
                timeout,
                MAX_REQUEST_BODY_BYTES,
                MAX_RESPONSE_BODY_BYTES,
            )
            .expect("fixture adapter configuration is valid"),
        }
    }
}

#[async_trait]
impl SponsoredBillingProvider for HttpSponsoredBillingProvider {
    async fn confirm_grant(
        &self,
        stable_request_id: &str,
        policy: &SponsorshipPolicy,
        launch_id: &LaunchId,
    ) -> Result<BillingAuthorization, SponsoredBillingError> {
        policy
            .validate()
            .map_err(|_| SponsoredBillingError::Unavailable)?;
        let request = BillingEligibilityRequest {
            policy: policy.into(),
            launch_id: &launch_id.0,
        };
        match self
            .http
            .billing_call::<_, BillingResult>(
                BILLING_CONFIRM_PATH,
                BILLING_CONFIRM_KIND,
                stable_request_id,
                request,
            )
            .await?
        {
            WireOutcome::Confirmed(result) => billing_from_result(result, policy, launch_id),
            WireOutcome::Pending => Err(SponsoredBillingError::Ambiguous),
            WireOutcome::Rejected | WireOutcome::Unsupported | WireOutcome::Malformed => {
                Err(SponsoredBillingError::Unavailable)
            }
        }
    }

    async fn reconcile_grant(
        &self,
        stable_request_id: &str,
        policy: &SponsorshipPolicy,
        launch_id: &LaunchId,
    ) -> Result<ProviderOperationResolution<BillingAuthorization>, SponsoredBillingError> {
        policy
            .validate()
            .map_err(|_| SponsoredBillingError::Unavailable)?;
        let request = BillingEligibilityRequest {
            policy: policy.into(),
            launch_id: &launch_id.0,
        };
        match self
            .http
            .billing_call::<_, BillingResult>(
                BILLING_RECONCILE_PATH,
                BILLING_RECONCILE_KIND,
                stable_request_id,
                request,
            )
            .await?
        {
            WireOutcome::Confirmed(result) => Ok(ProviderOperationResolution::Confirmed(
                billing_from_result(result, policy, launch_id)?,
            )),
            WireOutcome::Pending => Ok(ProviderOperationResolution::Pending),
            WireOutcome::Rejected | WireOutcome::Unsupported | WireOutcome::Malformed => {
                Err(SponsoredBillingError::Unavailable)
            }
        }
    }
}

/// Trusted pricing adapter using the sponsorship HTTP v1 contract.
#[derive(Clone, Debug)]
pub struct HttpSponsoredPricingProvider {
    http: HttpAdapter,
}

impl HttpSponsoredPricingProvider {
    pub fn new(adapter_id: String, origin: String) -> Result<Self, String> {
        Ok(Self {
            http: HttpAdapter::production(adapter_id, origin)?,
        })
    }

    #[cfg(test)]
    fn fixture(adapter_id: &str, origin: String, timeout: Duration) -> Self {
        Self {
            http: HttpAdapter::build(
                adapter_id.into(),
                origin,
                true,
                timeout.min(Duration::from_millis(50)),
                timeout,
                MAX_REQUEST_BODY_BYTES,
                MAX_RESPONSE_BODY_BYTES,
            )
            .expect("fixture adapter configuration is valid"),
        }
    }
}

#[async_trait]
impl SponsoredPricingProvider for HttpSponsoredPricingProvider {
    async fn quote(
        &self,
        provider: &str,
        model: &str,
        resource: &str,
        maximum_quota: SponsoredQuota,
        observed_at: &str,
    ) -> Result<SponsoredPriceQuote, SponsoredPricingError> {
        if !valid_policy_token(provider)
            || !valid_policy_token(model)
            || !valid_policy_token(resource)
            || !quota_is_positive(maximum_quota)
            || !valid_timestamp(observed_at)
        {
            return Err(SponsoredPricingError::Malformed);
        }
        let request = QuoteRequest {
            provider,
            model,
            resource,
            maximum_quota,
            observed_at,
        };
        let request_id = derived_request_id("pricing.quote", &request)
            .map_err(|_| SponsoredPricingError::Malformed)?;
        let outcome = self
            .http
            .pricing_call::<_, QuoteResult>(
                PRICING_QUOTE_PATH,
                PRICING_QUOTE_KIND,
                &request_id,
                request,
            )
            .await?;
        let result = match outcome {
            WireOutcome::Confirmed(result) => result,
            WireOutcome::Unsupported => return Err(SponsoredPricingError::Unsupported),
            WireOutcome::Rejected | WireOutcome::Pending | WireOutcome::Malformed => {
                return Err(SponsoredPricingError::Malformed)
            }
        };
        let quote = SponsoredPriceQuote {
            provider: result.provider,
            model: result.model,
            resource: result.resource,
            maximum: result.maximum.into(),
            pricing_generation: result.pricing_generation,
            digest: result.digest,
        };
        if quote.provider != provider
            || quote.model != model
            || quote.resource != resource
            || quote.maximum.quota != maximum_quota
            || quote.validate().is_err()
        {
            return Err(SponsoredPricingError::Malformed);
        }
        Ok(quote)
    }

    async fn price_actual(
        &self,
        quote: &SponsoredPriceQuote,
        actual_quota: SponsoredQuota,
        observed_at: &str,
    ) -> Result<SponsoredUsageSummary, SponsoredPricingError> {
        if quote.validate().is_err()
            || !actual_quota.fits_within(quote.maximum.quota)
            || !valid_timestamp(observed_at)
        {
            return Err(SponsoredPricingError::Malformed);
        }
        let request = ActualPriceRequest {
            quote_digest: &quote.digest,
            pricing_generation: quote.pricing_generation,
            provider: &quote.provider,
            model: &quote.model,
            resource: &quote.resource,
            maximum: &quote.maximum,
            actual_quota,
            observed_at,
        };
        let request_id = derived_request_id("pricing.price_actual", &request)
            .map_err(|_| SponsoredPricingError::Malformed)?;
        let outcome = self
            .http
            .pricing_call::<_, ActualPriceResult>(
                PRICING_ACTUAL_PATH,
                PRICING_ACTUAL_KIND,
                &request_id,
                request,
            )
            .await?;
        let result = match outcome {
            WireOutcome::Confirmed(result) => result,
            WireOutcome::Unsupported => return Err(SponsoredPricingError::Unsupported),
            WireOutcome::Rejected | WireOutcome::Pending | WireOutcome::Malformed => {
                return Err(SponsoredPricingError::Malformed)
            }
        };
        let actual: SponsoredUsageSummary = result.actual.into();
        let nonzero_quota = quota_is_nonzero(actual_quota);
        if result.quote_digest != quote.digest
            || result.pricing_generation != quote.pricing_generation
            || actual.quota != actual_quota
            || actual.spend_micros > quote.maximum.spend_micros
            || (nonzero_quota && actual.spend_micros == 0)
            || (!nonzero_quota && actual.spend_micros != 0)
        {
            return Err(SponsoredPricingError::Malformed);
        }
        Ok(actual)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use layer2_core::{
        InertGrantCapability, ProviderCredentialAuthority, ProviderGrantAuthority,
        SponsoredCredentialId, SponsoredGrantAcceptance, SponsoredGrantId, SponsoredProviderPolicy,
        SponsorshipPolicyId, SubjectId, TriggerRevisionId, SPONSORSHIP_SCHEMA_VERSION,
    };
    use serde_json::{json, Value};
    use sha2::{Digest, Sha256};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::sync::mpsc;

    use super::*;

    const ADAPTER_ID: &str = "fixture-adapter-v1";
    const NOW: &str = "2026-08-29T00:00:00Z";
    const ISSUED_AT: &str = "2026-08-29T00:01:00Z";
    const CREATED_AT: &str = "2026-08-29T00:02:00Z";
    const GRANT_EXPIRY: &str = "2026-08-29T01:00:00Z";
    const POLICY_EXPIRY: &str = "2026-08-30T00:00:00Z";
    const PROVIDER: &str = "provider-a";
    const POLICY_KEY: &str = "provider-a:least-privilege-v1";
    const CURRENT_SECRET: &str = "provider-current-secret-fixture";
    const REPLACEMENT_SECRET: &str = "provider-replacement-secret-fixture";
    const POLICY_HANDLE: &str = "provider-policy-handle-fixture";
    const CREDENTIAL_HANDLE: &str = "provider-credential-id-fixture";
    const GRANT_HANDLE: &str = "provider-grant-id-fixture";
    const REVOCATION_HANDLE: &str = "provider-revocation-handle-fixture";

    #[derive(Clone)]
    struct ResponseSpec {
        status: u16,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
        delay: Duration,
    }

    impl ResponseSpec {
        fn json(status: u16, body: Vec<u8>) -> Self {
            Self {
                status,
                headers: vec![("Content-Type".into(), "application/json".into())],
                body,
                delay: Duration::ZERO,
            }
        }

        fn delayed(mut self, delay: Duration) -> Self {
            self.delay = delay;
            self
        }

        fn with_header(mut self, name: &str, value: &str) -> Self {
            self.headers.push((name.into(), value.into()));
            self
        }
    }

    #[derive(Debug)]
    struct CapturedRequest {
        method: String,
        path: String,
        headers: BTreeMap<String, String>,
        body: Vec<u8>,
    }

    async fn fixture(
        responses: Vec<ResponseSpec>,
    ) -> (
        String,
        mpsc::Receiver<CapturedRequest>,
        tokio::task::JoinHandle<()>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (captured_sender, captured_receiver) = mpsc::channel(responses.len().max(1));
        let task = tokio::spawn(async move {
            for response in responses {
                let (mut stream, _) = listener.accept().await.unwrap();
                let captured = read_request(&mut stream).await;
                captured_sender.send(captured).await.unwrap();
                if !response.delay.is_zero() {
                    tokio::time::sleep(response.delay).await;
                }
                let reason = match response.status {
                    200 => "OK",
                    202 => "Accepted",
                    302 => "Found",
                    400 => "Bad Request",
                    404 => "Not Found",
                    409 => "Conflict",
                    422 => "Unprocessable Entity",
                    429 => "Too Many Requests",
                    500 => "Internal Server Error",
                    503 => "Service Unavailable",
                    _ => "Fixture",
                };
                let mut head = format!(
                    "HTTP/1.1 {} {}\r\nContent-Length: {}\r\nConnection: close\r\n",
                    response.status,
                    reason,
                    response.body.len()
                );
                for (name, value) in response.headers {
                    head.push_str(&format!("{name}: {value}\r\n"));
                }
                head.push_str("\r\n");
                if stream.write_all(head.as_bytes()).await.is_err() {
                    continue;
                }
                let _ = stream.write_all(&response.body).await;
            }
        });
        (format!("http://{address}"), captured_receiver, task)
    }

    async fn read_request(stream: &mut tokio::net::TcpStream) -> CapturedRequest {
        let mut bytes = Vec::new();
        let (header_end, content_length) = loop {
            if let Some(header_end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                let header_end = header_end + 4;
                let headers = std::str::from_utf8(&bytes[..header_end]).unwrap();
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                break (header_end, content_length);
            }
            assert!(bytes.len() < MAX_REQUEST_BODY_BYTES + 32 * 1024);
            let mut chunk = [0_u8; 4096];
            let read = stream.read(&mut chunk).await.unwrap();
            assert_ne!(read, 0, "request ended before its headers");
            bytes.extend_from_slice(&chunk[..read]);
        };
        while bytes.len() < header_end + content_length {
            let mut chunk = [0_u8; 4096];
            let read = stream.read(&mut chunk).await.unwrap();
            assert_ne!(read, 0, "request ended before its declared body");
            bytes.extend_from_slice(&chunk[..read]);
        }
        let head = std::str::from_utf8(&bytes[..header_end - 4]).unwrap();
        let mut lines = head.split("\r\n");
        let mut request_line = lines.next().unwrap().split_whitespace();
        let method = request_line.next().unwrap().to_string();
        let path = request_line.next().unwrap().to_string();
        assert_eq!(request_line.next(), Some("HTTP/1.1"));
        let mut headers = BTreeMap::new();
        for line in lines {
            let (name, value) = line.split_once(':').unwrap();
            assert!(
                headers
                    .insert(name.to_ascii_lowercase(), value.trim().to_string())
                    .is_none(),
                "fixture received a duplicate header"
            );
        }
        CapturedRequest {
            method,
            path,
            headers,
            body: bytes[header_end..header_end + content_length].to_vec(),
        }
    }

    fn envelope(kind: &str, status: &str, result: Option<Value>) -> Vec<u8> {
        let mut value = json!({
            "version": PROTOCOL_VERSION,
            "adapter_id": ADAPTER_ID,
            "kind": kind,
            "status": status,
        });
        if let Some(result) = result {
            value
                .as_object_mut()
                .unwrap()
                .insert("result".into(), result);
        }
        serde_json::to_vec(&value).unwrap()
    }

    fn digest(value: &str) -> String {
        Sha256::digest(value.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    fn operational_digest() -> String {
        format!("sp1.fixture.{}", "a".repeat(64))
    }

    fn owner() -> SponsorBillingOwner {
        SponsorBillingOwner::Developer {
            subject: SubjectId("sponsor-owner".into()),
            display_name: "Example Sponsor".into(),
        }
    }

    fn credential() -> SponsoredCredentialRecord {
        SponsoredCredentialRecord {
            schema_version: SPONSORSHIP_SCHEMA_VERSION,
            id: SponsoredCredentialId::new(),
            owner: owner(),
            authority: ProviderCredentialAuthority {
                provider: PROVIDER.into(),
                provider_policy_key: POLICY_KEY.into(),
                provider_policy_handle: SensitiveProviderValue::new(POLICY_HANDLE).unwrap(),
                provider_credential_id_digest: operational_digest(),
                state_version: 1,
                ownership_verified_at: NOW.into(),
                supports_expiry: true,
                supports_revocation: true,
            },
            state: SponsoredCredentialState::Active,
            material_state: SponsoredCredentialMaterialState::Retained,
            generation: 1,
            created_at: NOW.into(),
            updated_at: NOW.into(),
            revoked_at: None,
            pending_operation: None,
            superseded_operation: None,
            last_completed_operation: None,
        }
    }

    fn policy(credential: &SponsoredCredentialRecord) -> SponsorshipPolicy {
        SponsorshipPolicy {
            schema_version: SPONSORSHIP_SCHEMA_VERSION,
            id: SponsorshipPolicyId::new(),
            tenant_id: "tenant-a".into(),
            owner: owner(),
            credential_id: credential.id.clone(),
            provider_policy: SponsoredProviderPolicy {
                provider: PROVIDER.into(),
                provider_policy_key: POLICY_KEY.into(),
                allowed_models: BTreeSet::from(["model-a".into()]),
                allowed_resources: BTreeSet::from(["tokens".into()]),
            },
            per_launch_quota: SponsoredQuota {
                requests: 4,
                input_tokens: 1_000,
                output_tokens: 1_000,
            },
            aggregate_quota: SponsoredQuota {
                requests: 8,
                input_tokens: 2_000,
                output_tokens: 2_000,
            },
            per_launch_budget_micros: 400,
            aggregate_budget_micros: 800,
            max_requests_per_minute: 10,
            max_concurrent_launches: 4,
            expires_at: POLICY_EXPIRY.into(),
            state: SponsorshipPolicyState::Active,
            generation: 1,
            created_at: NOW.into(),
            updated_at: NOW.into(),
            revoked_at: None,
        }
    }

    fn grant(
        policy: &SponsorshipPolicy,
        credential: &SponsoredCredentialRecord,
        receipt: &ProviderGrantReceipt,
        launch_id: &LaunchId,
        session_id: &SessionId,
    ) -> SponsoredGrant {
        let revision_id = TriggerRevisionId::new();
        let mut acceptance = SponsoredGrantAcceptance {
            launch_id: launch_id.clone(),
            contributor: SubjectId("launch-contributor".into()),
            revision_id: revision_id.clone(),
            revision_digest: digest("revision"),
            policy_id: policy.id.clone(),
            policy_generation: policy.generation,
            terms_digest: digest("terms"),
            launch_snapshot_digest: digest("snapshot"),
            accepted_at: NOW.into(),
            idempotency_key_hash: operational_digest(),
            digest: String::new(),
        };
        acceptance.digest = acceptance.calculate_digest().unwrap();
        let id = SponsoredGrantId::new();
        let allocation = SponsoredUsageSummary {
            quota: policy.per_launch_quota,
            spend_micros: policy.per_launch_budget_micros,
        };
        let mut grant = SponsoredGrant {
            schema_version: SPONSORSHIP_SCHEMA_VERSION,
            id: id.clone(),
            tenant_id: policy.tenant_id.clone(),
            policy_id: policy.id.clone(),
            policy_generation: policy.generation,
            credential_generation: credential.generation,
            trigger_revision_id: revision_id,
            trigger_revision_digest: acceptance.revision_digest.clone(),
            launch_id: launch_id.clone(),
            session_id: session_id.clone(),
            contributor: acceptance.contributor.clone(),
            sponsor_subject: policy.owner.controller().clone(),
            terms_digest: acceptance.terms_digest.clone(),
            acceptance,
            authority: ProviderGrantAuthority {
                provider_grant_id_digest: operational_digest(),
                provider_state_version: receipt.provider_state_version,
                expires_at: receipt.expires_at.clone(),
                revocation_handle_digest: operational_digest(),
                issued_at: receipt.issued_at.clone(),
            },
            capability: InertGrantCapability {
                grant_id: id,
                grant_generation: 1,
                policy_generation: policy.generation,
                launch_id: launch_id.clone(),
                session_id: session_id.clone(),
                expires_at: receipt.expires_at.clone(),
                fingerprint: digest("placeholder"),
            },
            state: SponsoredGrantState::Active,
            allocation,
            usage: SponsoredUsageSummary::default(),
            generation: 1,
            created_at: CREATED_AT.into(),
            updated_at: CREATED_AT.into(),
            revoked_at: None,
            pending_revocation: None,
        };
        grant.refresh_capability().unwrap();
        grant.validate().unwrap();
        grant
    }

    fn evidence_result() -> Value {
        json!({
            "evidence": {
                "provider": PROVIDER,
                "provider_policy_key": POLICY_KEY,
                "provider_policy_handle": POLICY_HANDLE,
                "provider_credential_id": CREDENTIAL_HANDLE,
                "state_version": 1,
                "ownership_verified_at": NOW,
                "supports_expiry": true,
                "supports_revocation": true
            }
        })
    }

    fn receipt_result(
        credential: &SponsoredCredentialRecord,
        policy: &SponsorshipPolicy,
        launch_id: &LaunchId,
        session_id: &SessionId,
    ) -> Value {
        json!({
            "credential_id": credential.id.0,
            "credential_generation": credential.generation,
            "policy_id": policy.id.0,
            "policy_generation": policy.generation,
            "launch_id": launch_id.0,
            "session_id": session_id.0,
            "receipt": {
                "provider_grant_id": GRANT_HANDLE,
                "provider_state_version": 2,
                "expires_at": GRANT_EXPIRY,
                "revocation_handle": REVOCATION_HANDLE,
                "issued_at": ISSUED_AT
            }
        })
    }

    fn rotation_result(credential: &SponsoredCredentialRecord) -> Value {
        json!({
            "credential_id": credential.id.0,
            "previous_generation": credential.generation,
            "evidence": {
                "provider": PROVIDER,
                "provider_policy_key": POLICY_KEY,
                "provider_policy_handle": POLICY_HANDLE,
                "provider_credential_id": "replacement-provider-credential-id",
                "state_version": 2,
                "ownership_verified_at": ISSUED_AT,
                "supports_expiry": true,
                "supports_revocation": true
            }
        })
    }

    fn assert_request_contract(request: &CapturedRequest, expected_path: &str) -> Value {
        assert_eq!(request.method, "POST");
        assert_eq!(request.path, expected_path);
        assert_eq!(request.headers["content-type"], "application/json");
        assert_eq!(request.headers["accept"], "application/json");
        assert_eq!(request.headers["accept-encoding"], "identity");
        assert_eq!(request.headers["x-cma-sponsorship-adapter-id"], ADAPTER_ID);
        assert_eq!(
            request.headers["x-cma-request-id"],
            request.headers["idempotency-key"]
        );
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        assert_eq!(body["version"], PROTOCOL_VERSION);
        assert_eq!(body["adapter_id"], ADAPTER_ID);
        assert_eq!(body["request_id"], request.headers["x-cma-request-id"]);
        body
    }

    #[test]
    fn constructors_require_exact_adapter_ids_and_canonical_https_origins() {
        assert!(HttpSponsoredCredentialProvider::new(
            "provider-adapter-v1".into(),
            "https://provider.example".into()
        )
        .is_ok());
        assert!(HttpSponsoredBillingProvider::new(
            "billing/adapter:v1".into(),
            "https://billing.example:8443".into()
        )
        .is_ok());
        assert!(HttpSponsoredPricingProvider::new(
            "pricing.adapter_v1".into(),
            "https://pricing.example".into()
        )
        .is_ok());

        for origin in [
            "http://provider.example",
            "https://provider.example/",
            "https://PROVIDER.example",
            "https://user@provider.example",
            "https://provider.example/path",
            "https://provider.example?query",
            "https://provider.example#fragment",
            "https://provider.example:443",
        ] {
            assert!(
                HttpSponsoredCredentialProvider::new("adapter-v1".into(), origin.into()).is_err(),
                "accepted noncanonical origin {origin}"
            );
        }
        for adapter_id in ["", "contains space", "contains@sign"] {
            assert!(HttpSponsoredCredentialProvider::new(
                adapter_id.into(),
                "https://provider.example".into()
            )
            .is_err());
        }
    }

    #[tokio::test]
    async fn credential_provider_contract_covers_every_operation_family_and_redacts_secrets() {
        let credential = credential();
        let policy = policy(&credential);
        let launch_id = LaunchId::new();
        let session_id = SessionId::generate();
        let responses = vec![
            ResponseSpec::json(
                200,
                envelope(OWNERSHIP_KIND, "confirmed", Some(evidence_result())),
            ),
            ResponseSpec::json(
                200,
                envelope(
                    ISSUE_GRANT_KIND,
                    "confirmed",
                    Some(receipt_result(
                        &credential,
                        &policy,
                        &launch_id,
                        &session_id,
                    )),
                ),
            ),
            ResponseSpec::json(202, envelope(RECONCILE_GRANT_ISSUE_KIND, "pending", None)),
            ResponseSpec::json(
                200,
                envelope(REVOKE_UNCOMMITTED_GRANT_KIND, "confirmed", Some(json!({}))),
            ),
            ResponseSpec::json(
                202,
                envelope(RECONCILE_UNCOMMITTED_GRANT_REVOCATION_KIND, "pending", None),
            ),
            ResponseSpec::json(
                200,
                envelope(
                    ROTATE_CREDENTIAL_KIND,
                    "confirmed",
                    Some(rotation_result(&credential)),
                ),
            ),
            ResponseSpec::json(
                202,
                envelope(RECONCILE_CREDENTIAL_ROTATION_KIND, "pending", None),
            ),
            ResponseSpec::json(
                200,
                envelope(REVOKE_CREDENTIAL_KIND, "confirmed", Some(json!({}))),
            ),
            ResponseSpec::json(
                202,
                envelope(RECONCILE_CREDENTIAL_REVOCATION_KIND, "pending", None),
            ),
            ResponseSpec::json(
                200,
                envelope(REVOKE_GRANT_KIND, "confirmed", Some(json!({}))),
            ),
            ResponseSpec::json(
                202,
                envelope(RECONCILE_GRANT_REVOCATION_KIND, "pending", None),
            ),
        ];
        let (origin, mut captured, task) = fixture(responses).await;
        let adapter = HttpSponsoredCredentialProvider::fixture(
            ADAPTER_ID,
            origin,
            Duration::from_secs(2),
            MAX_REQUEST_BODY_BYTES,
            MAX_RESPONSE_BODY_BYTES,
        );
        let current_secret = ProviderCredentialSecret::new(CURRENT_SECRET).unwrap();
        let replacement_secret = ProviderCredentialSecret::new(REPLACEMENT_SECRET).unwrap();

        let evidence = adapter
            .validate_ownership(&owner(), PROVIDER, POLICY_KEY, &current_secret)
            .await
            .unwrap();
        assert_eq!(evidence.provider, PROVIDER);
        assert!(!format!("{evidence:?}").contains(POLICY_HANDLE));
        let issued_receipt = adapter
            .issue_grant(
                "issue-grant-stable-1",
                &credential,
                &current_secret,
                &policy,
                &launch_id,
                &session_id,
            )
            .await
            .unwrap();
        assert!(matches!(
            adapter
                .reconcile_grant_issue(
                    "reconcile-issue-stable-1",
                    &credential,
                    &current_secret,
                    &policy,
                    &launch_id,
                    &session_id,
                )
                .await
                .unwrap(),
            ProviderOperationResolution::Pending
        ));
        adapter
            .revoke_uncommitted_grant(
                "revoke-uncommitted-1",
                &issued_receipt,
                &credential,
                &current_secret,
            )
            .await
            .unwrap();
        assert!(matches!(
            adapter
                .reconcile_uncommitted_grant_revocation(
                    "reconcile-uncommitted-1",
                    &issued_receipt,
                    &credential,
                    &current_secret,
                )
                .await
                .unwrap(),
            ProviderOperationResolution::Pending
        ));
        adapter
            .rotate_credential(
                "rotate-credential-1",
                &credential,
                &current_secret,
                &replacement_secret,
            )
            .await
            .unwrap();
        assert!(matches!(
            adapter
                .reconcile_credential_rotation(
                    "reconcile-rotation-1",
                    &credential,
                    &current_secret,
                    &replacement_secret,
                )
                .await
                .unwrap(),
            ProviderOperationResolution::Pending
        ));
        adapter
            .revoke_credential(
                "revoke-credential-1",
                &credential,
                &current_secret,
                None,
                None,
            )
            .await
            .unwrap();
        assert!(matches!(
            adapter
                .reconcile_credential_revocation(
                    "reconcile-credential-revocation-1",
                    &credential,
                    &current_secret,
                    None,
                    None,
                )
                .await
                .unwrap(),
            ProviderOperationResolution::Pending
        ));
        let grant = grant(
            &policy,
            &credential,
            &issued_receipt,
            &launch_id,
            &session_id,
        );
        adapter
            .revoke_grant(
                "revoke-grant-1",
                &grant,
                &issued_receipt,
                &credential,
                &current_secret,
            )
            .await
            .unwrap();
        assert!(matches!(
            adapter
                .reconcile_grant_revocation(
                    "reconcile-grant-revocation-1",
                    &grant,
                    &issued_receipt,
                    &credential,
                    &current_secret,
                )
                .await
                .unwrap(),
            ProviderOperationResolution::Pending
        ));

        let expected_paths = [
            OWNERSHIP_PATH,
            ISSUE_GRANT_PATH,
            RECONCILE_GRANT_ISSUE_PATH,
            REVOKE_UNCOMMITTED_GRANT_PATH,
            RECONCILE_UNCOMMITTED_GRANT_REVOCATION_PATH,
            ROTATE_CREDENTIAL_PATH,
            RECONCILE_CREDENTIAL_ROTATION_PATH,
            REVOKE_CREDENTIAL_PATH,
            RECONCILE_CREDENTIAL_REVOCATION_PATH,
            REVOKE_GRANT_PATH,
            RECONCILE_GRANT_REVOCATION_PATH,
        ];
        for (index, expected_path) in expected_paths.into_iter().enumerate() {
            let request = captured.recv().await.unwrap();
            let body = assert_request_contract(&request, expected_path);
            let body_text = String::from_utf8(request.body).unwrap();
            assert!(!request.path.contains(CURRENT_SECRET));
            assert!(request
                .headers
                .values()
                .all(|value| !value.contains(CURRENT_SECRET)));
            assert!(body_text.contains(CURRENT_SECRET));
            if index == 0 {
                assert!(!request.headers["x-cma-request-id"].contains(CURRENT_SECRET));
                assert_eq!(body["kind"], OWNERSHIP_KIND);
            } else {
                assert!(body_text.contains(POLICY_HANDLE));
            }
        }
        task.await.unwrap();
        let debug = format!("{adapter:?}");
        assert!(!debug.contains(CURRENT_SECRET));
        assert!(!debug.contains(REPLACEMENT_SECRET));
        assert!(!debug.contains(POLICY_HANDLE));
        assert!(!format!("{:?}", SponsoredProviderError::Rejected).contains(CURRENT_SECRET));
    }

    #[tokio::test]
    async fn billing_contract_is_observation_only_and_reconciliation_can_remain_pending() {
        let credential = credential();
        let policy = policy(&credential);
        let launch_id = LaunchId::new();
        let confirmed = json!({
            "policy_id": policy.id.0,
            "policy_generation": policy.generation,
            "launch_id": launch_id.0,
            "authorization": "confirmed"
        });
        let responses = vec![
            ResponseSpec::json(
                200,
                envelope(BILLING_CONFIRM_KIND, "confirmed", Some(confirmed)),
            ),
            ResponseSpec::json(202, envelope(BILLING_RECONCILE_KIND, "pending", None)),
        ];
        let (origin, mut captured, task) = fixture(responses).await;
        let adapter =
            HttpSponsoredBillingProvider::fixture(ADAPTER_ID, origin, Duration::from_secs(2));
        assert_eq!(
            adapter
                .confirm_grant("billing-observation-1", &policy, &launch_id)
                .await
                .unwrap(),
            BillingAuthorization::Confirmed
        );
        assert!(matches!(
            adapter
                .reconcile_grant("billing-observation-1", &policy, &launch_id)
                .await
                .unwrap(),
            ProviderOperationResolution::Pending
        ));

        for path in [BILLING_CONFIRM_PATH, BILLING_RECONCILE_PATH] {
            let request = captured.recv().await.unwrap();
            let body = assert_request_contract(&request, path);
            assert_eq!(request.headers["x-cma-request-id"], "billing-observation-1");
            let mut keys = Vec::new();
            collect_json_keys(&body["request"], &mut keys);
            for forbidden in [
                "hold",
                "authorization",
                "charge",
                "invoice",
                "reservation",
                "mutation",
                "compensation",
                "receipt",
                "operation",
            ] {
                assert!(!keys.contains(&forbidden), "{keys:?}");
            }
            let text = String::from_utf8(request.body).unwrap();
            assert!(!text.contains(CURRENT_SECRET));
            assert!(!text.contains(POLICY_HANDLE));
            assert!(!text.contains(GRANT_HANDLE));
            assert!(!text.contains(REVOCATION_HANDLE));
        }
        task.await.unwrap();
    }

    fn collect_json_keys<'a>(value: &'a Value, keys: &mut Vec<&'a str>) {
        match value {
            Value::Object(object) => {
                for (key, value) in object {
                    keys.push(key);
                    collect_json_keys(value, keys);
                }
            }
            Value::Array(array) => {
                for value in array {
                    collect_json_keys(value, keys);
                }
            }
            _ => {}
        }
    }

    #[tokio::test]
    async fn pricing_contract_validates_quote_identity_digest_generation_quota_and_actual_spend() {
        let maximum_quota = SponsoredQuota {
            requests: 4,
            input_tokens: 1_000,
            output_tokens: 500,
        };
        let mut expected_quote = SponsoredPriceQuote {
            provider: PROVIDER.into(),
            model: "model-a".into(),
            resource: "tokens".into(),
            maximum: SponsoredUsageSummary {
                quota: maximum_quota,
                spend_micros: 900,
            },
            pricing_generation: 7,
            digest: String::new(),
        };
        expected_quote.digest = expected_quote.calculate_digest().unwrap();
        let quote_result = json!({
            "provider": expected_quote.provider,
            "model": expected_quote.model,
            "resource": expected_quote.resource,
            "maximum": {
                "quota": maximum_quota,
                "spend_micros": expected_quote.maximum.spend_micros
            },
            "pricing_generation": expected_quote.pricing_generation,
            "digest": expected_quote.digest
        });
        let actual_quota = SponsoredQuota {
            requests: 2,
            input_tokens: 400,
            output_tokens: 200,
        };
        let actual_result = json!({
            "quote_digest": expected_quote.digest,
            "pricing_generation": expected_quote.pricing_generation,
            "actual": {
                "quota": actual_quota,
                "spend_micros": 350
            }
        });
        let responses = vec![
            ResponseSpec::json(
                200,
                envelope(PRICING_QUOTE_KIND, "confirmed", Some(quote_result)),
            ),
            ResponseSpec::json(
                200,
                envelope(PRICING_ACTUAL_KIND, "confirmed", Some(actual_result)),
            ),
        ];
        let (origin, mut captured, task) = fixture(responses).await;
        let adapter =
            HttpSponsoredPricingProvider::fixture(ADAPTER_ID, origin, Duration::from_secs(2));
        let quote = adapter
            .quote(PROVIDER, "model-a", "tokens", maximum_quota, NOW)
            .await
            .unwrap();
        assert_eq!(quote, expected_quote);
        let actual = adapter
            .price_actual(&quote, actual_quota, ISSUED_AT)
            .await
            .unwrap();
        assert_eq!(actual.quota, actual_quota);
        assert_eq!(actual.spend_micros, 350);

        let quote_request = captured.recv().await.unwrap();
        let quote_body = assert_request_contract(&quote_request, PRICING_QUOTE_PATH);
        let actual_request = captured.recv().await.unwrap();
        let actual_body = assert_request_contract(&actual_request, PRICING_ACTUAL_PATH);
        assert_eq!(quote_body["request"]["maximum_quota"]["requests"], 4);
        assert_eq!(actual_body["request"]["quote_digest"], quote.digest);
        assert_ne!(
            quote_request.headers["x-cma-request-id"],
            actual_request.headers["x-cma-request-id"]
        );
        for request in [quote_request, actual_request] {
            let text = String::from_utf8(request.body).unwrap();
            assert!(!text.contains(CURRENT_SECRET));
            assert!(!text.contains(POLICY_HANDLE));
            assert!(!text.contains(GRANT_HANDLE));
            assert!(!text.contains(REVOCATION_HANDLE));
        }
        task.await.unwrap();
    }

    #[tokio::test]
    async fn pricing_rejects_unsupported_and_malformed_quotes() {
        let quota = SponsoredQuota {
            requests: 1,
            input_tokens: 1,
            output_tokens: 1,
        };
        let malformed = json!({
            "provider": PROVIDER,
            "model": "model-a",
            "resource": "tokens",
            "maximum": { "quota": quota, "spend_micros": 1 },
            "pricing_generation": 1,
            "digest": "not-a-digest"
        });
        let responses = vec![
            ResponseSpec::json(404, envelope(PRICING_QUOTE_KIND, "unsupported", None)),
            ResponseSpec::json(
                200,
                envelope(PRICING_QUOTE_KIND, "confirmed", Some(malformed)),
            ),
        ];
        let (origin, _captured, task) = fixture(responses).await;
        let adapter =
            HttpSponsoredPricingProvider::fixture(ADAPTER_ID, origin, Duration::from_secs(2));
        assert_eq!(
            adapter
                .quote(PROVIDER, "model-a", "tokens", quota, NOW)
                .await,
            Err(SponsoredPricingError::Unsupported)
        );
        assert_eq!(
            adapter
                .quote(PROVIDER, "model-a", "tokens", quota, NOW)
                .await,
            Err(SponsoredPricingError::Malformed)
        );
        task.await.unwrap();
    }

    #[tokio::test]
    async fn strict_envelopes_redirect_policy_statuses_and_response_bounds_are_enforced() {
        let wrong_adapter = json!({
            "version": PROTOCOL_VERSION,
            "adapter_id": "different-adapter",
            "kind": REVOKE_CREDENTIAL_KIND,
            "status": "confirmed",
            "result": {}
        });
        let unknown_result = json!({
            "version": PROTOCOL_VERSION,
            "adapter_id": ADAPTER_ID,
            "kind": REVOKE_CREDENTIAL_KIND,
            "status": "confirmed",
            "result": { "unexpected": true }
        });
        let responses = vec![
            ResponseSpec::json(
                302,
                envelope(REVOKE_CREDENTIAL_KIND, "confirmed", Some(json!({}))),
            )
            .with_header("Location", "http://127.0.0.1:9/redirect-target"),
            ResponseSpec::json(503, b"service unavailable".to_vec()),
            ResponseSpec::json(503, b"service unavailable".to_vec()),
            ResponseSpec::json(429, b"rate limited".to_vec()),
            ResponseSpec::json(200, serde_json::to_vec(&wrong_adapter).unwrap()),
            ResponseSpec::json(200, serde_json::to_vec(&unknown_result).unwrap()),
            ResponseSpec {
                status: 200,
                headers: vec![(
                    "Content-Type".into(),
                    "application/json; charset=utf-8".into(),
                )],
                body: envelope(REVOKE_CREDENTIAL_KIND, "confirmed", Some(json!({}))),
                delay: Duration::ZERO,
            },
            ResponseSpec::json(200, vec![b'x'; 512]),
            ResponseSpec::json(400, envelope(REVOKE_CREDENTIAL_KIND, "rejected", None)),
        ];
        let (origin, mut captured, task) = fixture(responses).await;
        let adapter = HttpSponsoredCredentialProvider::fixture(
            ADAPTER_ID,
            origin,
            Duration::from_secs(2),
            MAX_REQUEST_BODY_BYTES,
            256,
        );

        let redirect: Result<WireOutcome<EmptyResult>, _> = adapter
            .http
            .provider_call(
                REVOKE_CREDENTIAL_PATH,
                REVOKE_CREDENTIAL_KIND,
                "strict-response-1",
                json!({}),
                true,
            )
            .await;
        assert_eq!(
            redirect.err(),
            Some(SponsoredProviderError::MalformedEvidence)
        );

        for (request_id, mutation, expected) in [
            ("strict-response-2", true, SponsoredProviderError::Ambiguous),
            (
                "strict-response-3",
                false,
                SponsoredProviderError::Unavailable,
            ),
            ("strict-response-4", true, SponsoredProviderError::Ambiguous),
        ] {
            let result: Result<WireOutcome<EmptyResult>, _> = adapter
                .http
                .provider_call(
                    REVOKE_CREDENTIAL_PATH,
                    REVOKE_CREDENTIAL_KIND,
                    request_id,
                    json!({}),
                    mutation,
                )
                .await;
            assert_eq!(result.err(), Some(expected));
        }
        for request_id in [
            "strict-response-5",
            "strict-response-6",
            "strict-response-7",
            "strict-response-8",
        ] {
            let result: Result<WireOutcome<EmptyResult>, _> = adapter
                .http
                .provider_call(
                    REVOKE_CREDENTIAL_PATH,
                    REVOKE_CREDENTIAL_KIND,
                    request_id,
                    json!({}),
                    true,
                )
                .await;
            assert_eq!(
                result.err(),
                Some(SponsoredProviderError::MalformedEvidence)
            );
        }
        let rejected: Result<WireOutcome<EmptyResult>, _> = adapter
            .http
            .provider_call(
                REVOKE_CREDENTIAL_PATH,
                REVOKE_CREDENTIAL_KIND,
                "strict-response-9",
                json!({}),
                true,
            )
            .await;
        assert!(matches!(rejected, Ok(WireOutcome::Rejected)));

        for _ in 0..9 {
            let request = captured.recv().await.unwrap();
            assert_eq!(request.path, REVOKE_CREDENTIAL_PATH);
        }
        task.await.unwrap();
    }

    #[tokio::test]
    async fn timeout_connect_and_request_bound_failures_have_deliberate_error_mappings() {
        let delayed = ResponseSpec::json(
            200,
            envelope(REVOKE_CREDENTIAL_KIND, "confirmed", Some(json!({}))),
        )
        .delayed(Duration::from_millis(200));
        let (origin, mut captured, _task) = fixture(vec![delayed]).await;
        let adapter = HttpSponsoredCredentialProvider::fixture(
            ADAPTER_ID,
            origin,
            Duration::from_millis(50),
            MAX_REQUEST_BODY_BYTES,
            MAX_RESPONSE_BODY_BYTES,
        );
        let timed_out: Result<WireOutcome<EmptyResult>, _> = adapter
            .http
            .provider_call(
                REVOKE_CREDENTIAL_PATH,
                REVOKE_CREDENTIAL_KIND,
                "timeout-mutation-1",
                json!({}),
                true,
            )
            .await;
        assert_eq!(timed_out.err(), Some(SponsoredProviderError::Ambiguous));
        captured.recv().await.unwrap();

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let unused_origin = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        let disconnected = HttpSponsoredCredentialProvider::fixture(
            ADAPTER_ID,
            unused_origin.clone(),
            Duration::from_millis(100),
            MAX_REQUEST_BODY_BYTES,
            MAX_RESPONSE_BODY_BYTES,
        );
        let connect: Result<WireOutcome<EmptyResult>, _> = disconnected
            .http
            .provider_call(
                REVOKE_CREDENTIAL_PATH,
                REVOKE_CREDENTIAL_KIND,
                "connect-failure-1",
                json!({}),
                true,
            )
            .await;
        assert_eq!(connect.err(), Some(SponsoredProviderError::Unavailable));

        let bounded = HttpSponsoredCredentialProvider::fixture(
            ADAPTER_ID,
            unused_origin,
            Duration::from_millis(100),
            64,
            MAX_RESPONSE_BODY_BYTES,
        );
        let oversized: Result<WireOutcome<EmptyResult>, _> = bounded
            .http
            .provider_call(
                REVOKE_CREDENTIAL_PATH,
                REVOKE_CREDENTIAL_KIND,
                "oversized-request-1",
                json!({ "credential_secret": "s".repeat(1_000) }),
                true,
            )
            .await;
        let oversized_error = oversized.err().unwrap();
        assert_eq!(oversized_error, SponsoredProviderError::Rejected);
        assert!(!format!("{oversized_error:?}").contains(&"s".repeat(64)));
    }

    #[tokio::test]
    async fn billing_and_pricing_timeout_mappings_match_their_domain_contracts() {
        let billing_response = ResponseSpec::json(
            200,
            envelope(BILLING_CONFIRM_KIND, "confirmed", Some(json!({}))),
        )
        .delayed(Duration::from_millis(200));
        let (billing_origin, _captured, _task) = fixture(vec![billing_response]).await;
        let billing = HttpSponsoredBillingProvider::fixture(
            ADAPTER_ID,
            billing_origin,
            Duration::from_millis(50),
        );
        let billing_result: Result<WireOutcome<BillingResult>, _> = billing
            .http
            .billing_call(
                BILLING_CONFIRM_PATH,
                BILLING_CONFIRM_KIND,
                "billing-timeout-1",
                json!({}),
            )
            .await;
        assert_eq!(billing_result.err(), Some(SponsoredBillingError::Ambiguous));

        let pricing_response = ResponseSpec::json(
            200,
            envelope(PRICING_QUOTE_KIND, "confirmed", Some(json!({}))),
        )
        .delayed(Duration::from_millis(200));
        let (pricing_origin, _captured, _task) = fixture(vec![pricing_response]).await;
        let pricing = HttpSponsoredPricingProvider::fixture(
            ADAPTER_ID,
            pricing_origin,
            Duration::from_millis(50),
        );
        let pricing_result: Result<WireOutcome<QuoteResult>, _> = pricing
            .http
            .pricing_call(
                PRICING_QUOTE_PATH,
                PRICING_QUOTE_KIND,
                "pricing-timeout-1",
                json!({}),
            )
            .await;
        assert_eq!(
            pricing_result.err(),
            Some(SponsoredPricingError::Unavailable)
        );
    }
}
