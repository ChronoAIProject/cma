//! Authenticated sponsored credential, policy, terms, grant, and metering APIs.

use std::collections::BTreeSet;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{FromRequest, Path, Query, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use layer2_core::{
    AcceptSponsoredTerms, CreateSponsorshipPolicy, InertGrantCapability, LaunchId,
    ProviderCredentialSecret, RegisterSponsoredCredential, ReportSponsoredUsage,
    RequestSponsoredUsage, ReservationDisposition, RotateSponsoredCredential, SponsorBillingOwner,
    SponsoredAuditEvent, SponsoredCredentialId, SponsoredCredentialView, SponsoredGrantAcceptance,
    SponsoredGrantId, SponsoredGrantTerms, SponsoredGrantView, SponsoredProviderPolicy,
    SponsoredQuota, SponsoredTermsReview, SponsoredUsageReservation, SponsoredUsageReservationId,
    SponsoredUsageReservationOutcome, SponsoredUsageState, SponsorshipError, SponsorshipGateFence,
    SponsorshipPolicyId, SponsorshipPolicyUsage, SponsorshipPolicyView, SponsorshipService,
    SubjectId,
};
use serde::{de::DeserializeOwned, Deserialize, Deserializer, Serialize};
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};
use zeroize::Zeroizing;

use super::pagination::{SponsorshipPageCollection, SponsorshipPageParams};
use super::{
    json_problem_response, problem_document, problem_response, HttpProblem, JsonProblemKind,
    TriggersState,
};
use crate::auth::AuthenticatedCaller;

const DEFAULT_AUDIT_LIMIT: usize = 100;
const RATE_RETRY_AFTER_SECONDS: u64 = 60;

pub(super) fn documented_router(state: TriggersState) -> OpenApiRouter {
    OpenApiRouter::new()
        .routes(routes!(register_credential, list_credentials))
        .routes(routes!(read_credential))
        .routes(routes!(rotate_credential))
        .routes(routes!(revoke_credential))
        .routes(routes!(create_policy, list_policies))
        .routes(routes!(read_policy))
        .routes(routes!(suspend_policy))
        .routes(routes!(revoke_policy))
        .routes(routes!(review_terms))
        .routes(routes!(accept_terms))
        .routes(routes!(read_launch_grant))
        .routes(routes!(list_grants))
        .routes(routes!(revoke_grant))
        .routes(routes!(close_grant))
        .routes(routes!(reserve_usage))
        .routes(routes!(reconcile_usage))
        .routes(routes!(read_actor_audit))
        .routes(routes!(engage_kill_switch))
        .with_state(state)
}

fn service(state: &TriggersState) -> Result<Arc<SponsorshipService>, SponsorshipError> {
    state.sponsorships.clone().ok_or(SponsorshipError::Disabled)
}

#[derive(Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum BillingOwnerRequest {
    Developer {
        display_name: String,
    },
    Organization {
        organization_id: String,
        display_name: String,
    },
}

impl BillingOwnerRequest {
    fn resolve(self, caller: &SubjectId) -> Result<SponsorBillingOwner, SponsorshipError> {
        match self {
            Self::Developer { display_name } => Ok(SponsorBillingOwner::Developer {
                subject: caller.clone(),
                display_name,
            }),
            // Public authentication currently proves only the developer
            // subject. Until an organization-authority port is composed, a
            // submitted organization identifier is never ownership evidence.
            Self::Organization {
                organization_id,
                display_name,
            } => {
                drop((organization_id, display_name));
                Err(SponsorshipError::Forbidden)
            }
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
enum GrantCollectionRole {
    Contributor,
    Sponsor,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct GrantPageParams {
    role: GrantCollectionRole,
    #[serde(default)]
    limit: Option<u32>,
    #[serde(default)]
    cursor: Option<String>,
}

impl GrantPageParams {
    fn collection(&self) -> SponsorshipPageCollection {
        match self.role {
            GrantCollectionRole::Contributor => SponsorshipPageCollection::ContributorGrants,
            GrantCollectionRole::Sponsor => SponsorshipPageCollection::SponsorGrants,
        }
    }

    fn page(&self) -> SponsorshipPageParams {
        SponsorshipPageParams::new(self.limit, self.cursor.clone())
    }
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct RegisterCredentialRequest {
    stable_request_id: String,
    owner: BillingOwnerRequest,
    provider: String,
    provider_policy_key: String,
    #[serde(deserialize_with = "zeroizing_string")]
    #[schema(value_type = String, format = Password)]
    credential: Zeroizing<String>,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct RotateCredentialRequest {
    stable_request_id: String,
    #[serde(deserialize_with = "zeroizing_string")]
    #[schema(value_type = String, format = Password)]
    replacement: Zeroizing<String>,
}

fn zeroizing_string<'de, D>(deserializer: D) -> Result<Zeroizing<String>, D::Error>
where
    D: Deserializer<'de>,
{
    String::deserialize(deserializer).map(Zeroizing::new)
}

fn provider_credential_secret(
    mut value: Zeroizing<String>,
) -> Result<ProviderCredentialSecret, SponsorshipError> {
    ProviderCredentialSecret::new(std::mem::take(&mut *value)).map_err(Into::into)
}

struct SponsorshipJson<T>(T);

impl<T, S> FromRequest<S> for SponsorshipJson<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = SponsorshipBodyProblem;

    async fn from_request(request: Request, state: &S) -> Result<Self, Self::Rejection> {
        if !is_json_content_type(request.headers()) {
            return Err(SponsorshipBodyProblem::MissingContentType);
        }

        let bytes = Bytes::from_request(request, state).await.map_err(|error| {
            if error.status() == StatusCode::PAYLOAD_TOO_LARGE {
                SponsorshipBodyProblem::TooLarge
            } else {
                SponsorshipBodyProblem::Unreadable
            }
        })?;
        // Axum's immutable transport allocation cannot be wiped. Copy it into
        // application-owned zeroizing storage, then release the transport
        // allocation immediately. Parsed credential strings use the same
        // zeroizing custody; this does not claim upstream transport buffers
        // are erased.
        let zeroizing_bytes = Zeroizing::new(bytes.to_vec());
        drop(bytes);
        let value = serde_json::from_slice(zeroizing_bytes.as_slice()).map_err(|error| {
            use serde_json::error::Category;

            match error.classify() {
                Category::Data => SponsorshipBodyProblem::Data,
                Category::Syntax | Category::Eof => SponsorshipBodyProblem::Syntax,
                Category::Io => SponsorshipBodyProblem::Unreadable,
            }
        })?;
        Ok(Self(value))
    }
}

fn is_json_content_type(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<mime::Mime>().ok())
        .is_some_and(|mime| {
            mime.type_() == "application"
                && (mime.subtype() == "json" || mime.suffix().is_some_and(|name| name == "json"))
        })
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct StableRequest {
    stable_request_id: String,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct QuotaRequest {
    requests: u64,
    input_tokens: u64,
    output_tokens: u64,
}

impl From<QuotaRequest> for SponsoredQuota {
    fn from(value: QuotaRequest) -> Self {
        Self {
            requests: value.requests,
            input_tokens: value.input_tokens,
            output_tokens: value.output_tokens,
        }
    }
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct CreatePolicyRequest {
    tenant_id: String,
    owner: BillingOwnerRequest,
    credential_id: String,
    provider: String,
    provider_policy_key: String,
    allowed_models: Vec<String>,
    allowed_resources: Vec<String>,
    per_launch_quota: QuotaRequest,
    aggregate_quota: QuotaRequest,
    per_launch_budget_micros: u64,
    aggregate_budget_micros: u64,
    max_requests_per_minute: u32,
    max_concurrent_launches: u32,
    expires_at: String,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct AcceptTermsRequest {
    policy_id: String,
    terms_digest: String,
    launch_snapshot_digest: String,
    idempotency_key: String,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct ReserveUsageRequest {
    stable_request_id: String,
    grant_id: String,
    provider: String,
    model: String,
    resource: String,
    maximum_quota: QuotaRequest,
    capability: CapabilityRequest,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct CapabilityRequest {
    grant_id: String,
    grant_generation: u64,
    policy_generation: u64,
    launch_id: String,
    session_id: String,
    expires_at: String,
    fingerprint: String,
}

impl CapabilityRequest {
    fn resolve(self) -> Result<InertGrantCapability, SponsorshipError> {
        Ok(InertGrantCapability {
            grant_id: SponsoredGrantId::parse(self.grant_id)?,
            grant_generation: self.grant_generation,
            policy_generation: self.policy_generation,
            launch_id: LaunchId::parse(self.launch_id)?,
            session_id: layer2_core::SessionId(self.session_id),
            expires_at: self.expires_at,
            fingerprint: self.fingerprint,
        })
    }
}

#[derive(Debug, Clone, Copy, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
enum UsageOutcomeRequest {
    Ambiguous,
    Settled,
    Released,
}

impl From<UsageOutcomeRequest> for SponsoredUsageState {
    fn from(value: UsageOutcomeRequest) -> Self {
        match value {
            UsageOutcomeRequest::Ambiguous => Self::Ambiguous,
            UsageOutcomeRequest::Settled => Self::Settled,
            UsageOutcomeRequest::Released => Self::Released,
        }
    }
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct ReconcileUsageRequest {
    reservation_id: String,
    request_fingerprint: String,
    actual_quota: Option<QuotaRequest>,
    outcome: UsageOutcomeRequest,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct AuditParams {
    #[serde(default = "default_audit_limit")]
    limit: usize,
}

fn default_audit_limit() -> usize {
    DEFAULT_AUDIT_LIMIT
}

#[derive(Serialize, ToSchema)]
struct QuotaView {
    requests: u64,
    input_tokens: u64,
    output_tokens: u64,
}

impl From<SponsoredQuota> for QuotaView {
    fn from(value: SponsoredQuota) -> Self {
        Self {
            requests: value.requests,
            input_tokens: value.input_tokens,
            output_tokens: value.output_tokens,
        }
    }
}

#[derive(Serialize, ToSchema)]
struct UsageSummaryView {
    quota: QuotaView,
    spend_micros: u64,
}

impl From<layer2_core::SponsoredUsageSummary> for UsageSummaryView {
    fn from(value: layer2_core::SponsoredUsageSummary) -> Self {
        Self {
            quota: value.quota.into(),
            spend_micros: value.spend_micros,
        }
    }
}

#[derive(Serialize, ToSchema)]
struct CredentialView {
    id: String,
    provider: String,
    provider_policy_key: String,
    state: String,
    generation: u64,
    ownership_verified_at: String,
    updated_at: String,
}

impl From<SponsoredCredentialView> for CredentialView {
    fn from(value: SponsoredCredentialView) -> Self {
        Self {
            id: value.id.0,
            provider: value.provider,
            provider_policy_key: value.provider_policy_key,
            state: enum_code(value.state),
            generation: value.generation,
            ownership_verified_at: value.ownership_verified_at,
            updated_at: value.updated_at,
        }
    }
}

#[derive(Serialize, ToSchema)]
struct PolicyUsageView {
    consumed: UsageSummaryView,
    reserved: UsageSummaryView,
    active_launches: u32,
}

impl From<SponsorshipPolicyUsage> for PolicyUsageView {
    fn from(value: SponsorshipPolicyUsage) -> Self {
        Self {
            consumed: value.consumed.into(),
            reserved: value.reserved.into(),
            active_launches: value.active_launches,
        }
    }
}

#[derive(Serialize, ToSchema)]
struct PolicyView {
    id: String,
    tenant_id: String,
    sponsor_display_name: String,
    provider: String,
    provider_policy_key: String,
    allowed_models: Vec<String>,
    allowed_resources: Vec<String>,
    per_launch_quota: QuotaView,
    aggregate_quota: QuotaView,
    per_launch_budget_micros: u64,
    aggregate_budget_micros: u64,
    max_requests_per_minute: u32,
    max_concurrent_launches: u32,
    expires_at: String,
    state: String,
    generation: u64,
    usage: PolicyUsageView,
    updated_at: String,
}

impl From<SponsorshipPolicyView> for PolicyView {
    fn from(value: SponsorshipPolicyView) -> Self {
        Self {
            id: value.id.0,
            tenant_id: value.tenant_id,
            sponsor_display_name: value.sponsor_display_name,
            provider: value.provider,
            provider_policy_key: value.provider_policy_key,
            allowed_models: value.allowed_models.into_iter().collect(),
            allowed_resources: value.allowed_resources.into_iter().collect(),
            per_launch_quota: value.per_launch_quota.into(),
            aggregate_quota: value.aggregate_quota.into(),
            per_launch_budget_micros: value.per_launch_budget_micros,
            aggregate_budget_micros: value.aggregate_budget_micros,
            max_requests_per_minute: value.max_requests_per_minute,
            max_concurrent_launches: value.max_concurrent_launches,
            expires_at: value.expires_at,
            state: enum_code(value.state),
            generation: value.generation,
            usage: value.usage.into(),
            updated_at: value.updated_at,
        }
    }
}

#[derive(Serialize, ToSchema)]
struct TermsView {
    policy_id: String,
    policy_generation: u64,
    sponsor_display_name: String,
    provider: String,
    allowed_models: Vec<String>,
    allowed_resources: Vec<String>,
    per_launch_quota: QuotaView,
    aggregate_remaining_quota: QuotaView,
    per_launch_budget_micros: u64,
    aggregate_remaining_budget_micros: u64,
    max_requests_per_minute: u32,
    max_concurrent_launches: u32,
    available_launches: u32,
    expires_at: String,
    github_credential_route: String,
    model_credential_route: String,
    digest: String,
}

impl From<SponsoredGrantTerms> for TermsView {
    fn from(value: SponsoredGrantTerms) -> Self {
        Self {
            policy_id: value.policy_id.0,
            policy_generation: value.policy_generation,
            sponsor_display_name: value.sponsor_display_name,
            provider: value.provider,
            allowed_models: value.allowed_models.into_iter().collect(),
            allowed_resources: value.allowed_resources.into_iter().collect(),
            per_launch_quota: value.per_launch_quota.into(),
            aggregate_remaining_quota: value.aggregate_remaining_quota.into(),
            per_launch_budget_micros: value.per_launch_budget_micros,
            aggregate_remaining_budget_micros: value.aggregate_remaining_budget_micros,
            max_requests_per_minute: value.max_requests_per_minute,
            max_concurrent_launches: value.max_concurrent_launches,
            available_launches: value.available_launches,
            expires_at: value.expires_at,
            github_credential_route: enum_code(value.credential_route.github),
            model_credential_route: enum_code(value.credential_route.model_provider),
            digest: value.digest,
        }
    }
}

#[derive(Serialize, ToSchema)]
struct TermsReviewView {
    terms: TermsView,
    launch_snapshot_digest: String,
}

impl From<SponsoredTermsReview> for TermsReviewView {
    fn from(value: SponsoredTermsReview) -> Self {
        Self {
            terms: value.terms.into(),
            launch_snapshot_digest: value.launch_snapshot_digest,
        }
    }
}

#[derive(Serialize, ToSchema)]
struct AcceptanceView {
    launch_id: String,
    policy_id: String,
    policy_generation: u64,
    terms_digest: String,
    launch_snapshot_digest: String,
    accepted_at: String,
    digest: String,
}

impl From<SponsoredGrantAcceptance> for AcceptanceView {
    fn from(value: SponsoredGrantAcceptance) -> Self {
        Self {
            launch_id: value.launch_id.0,
            policy_id: value.policy_id.0,
            policy_generation: value.policy_generation,
            terms_digest: value.terms_digest,
            launch_snapshot_digest: value.launch_snapshot_digest,
            accepted_at: value.accepted_at,
            digest: value.digest,
        }
    }
}

#[derive(Serialize, ToSchema)]
struct CapabilityView {
    grant_id: String,
    grant_generation: u64,
    policy_generation: u64,
    launch_id: String,
    session_id: String,
    expires_at: String,
    fingerprint: String,
}

impl From<InertGrantCapability> for CapabilityView {
    fn from(value: InertGrantCapability) -> Self {
        Self {
            grant_id: value.grant_id.0,
            grant_generation: value.grant_generation,
            policy_generation: value.policy_generation,
            launch_id: value.launch_id.0,
            session_id: value.session_id.0,
            expires_at: value.expires_at,
            fingerprint: value.fingerprint,
        }
    }
}

#[derive(Serialize, ToSchema)]
struct GrantView {
    id: String,
    policy_id: String,
    launch_id: String,
    session_id: String,
    contributor: String,
    state: String,
    allocation: UsageSummaryView,
    usage: UsageSummaryView,
    capability: CapabilityView,
    expires_at: String,
    updated_at: String,
    provider_revocation_pending: bool,
}

impl From<SponsoredGrantView> for GrantView {
    fn from(value: SponsoredGrantView) -> Self {
        Self {
            id: value.id.0,
            policy_id: value.policy_id.0,
            launch_id: value.launch_id.0,
            session_id: value.session_id.0,
            contributor: value.contributor.0,
            state: enum_code(value.state),
            allocation: value.allocation.into(),
            usage: value.usage.into(),
            capability: value.capability.into(),
            expires_at: value.expires_at,
            updated_at: value.updated_at,
            provider_revocation_pending: value.provider_revocation_pending,
        }
    }
}

#[derive(Serialize, ToSchema)]
struct ReservationView {
    id: String,
    request_fingerprint: String,
    grant_id: String,
    provider: String,
    model: String,
    resource: String,
    reserved: UsageSummaryView,
    state: String,
    actual: Option<UsageSummaryView>,
    created_at: String,
    updated_at: String,
}

impl From<SponsoredUsageReservation> for ReservationView {
    fn from(value: SponsoredUsageReservation) -> Self {
        Self {
            id: value.id.0,
            request_fingerprint: value.request_fingerprint,
            grant_id: value.grant_id.0,
            provider: value.price_quote.provider,
            model: value.price_quote.model,
            resource: value.price_quote.resource,
            reserved: value.reserved.into(),
            state: enum_code(value.state),
            actual: value.actual.map(Into::into),
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

#[derive(Serialize, ToSchema)]
struct ReservationOutcomeView {
    disposition: String,
    reservation: ReservationView,
}

impl From<SponsoredUsageReservationOutcome> for ReservationOutcomeView {
    fn from(value: SponsoredUsageReservationOutcome) -> Self {
        let disposition = match value.disposition {
            ReservationDisposition::Created => "created",
            ReservationDisposition::Existing => "existing",
        };
        Self {
            disposition: disposition.into(),
            reservation: value.reservation.into(),
        }
    }
}

#[derive(Serialize, ToSchema)]
struct AuditEventView {
    event_id: String,
    action: String,
    actor_digest: String,
    policy_id: Option<String>,
    grant_id: Option<String>,
    stable_request_id_digest: Option<String>,
    outcome_code: String,
    occurred_at: String,
}

impl From<SponsoredAuditEvent> for AuditEventView {
    fn from(value: SponsoredAuditEvent) -> Self {
        Self {
            event_id: value.event_id,
            action: enum_code(value.action),
            actor_digest: value.actor_digest,
            policy_id: value.policy_id.map(|id| id.0),
            grant_id: value.grant_id.map(|id| id.0),
            stable_request_id_digest: value.stable_request_id_digest,
            outcome_code: value.outcome_code,
            occurred_at: value.occurred_at,
        }
    }
}

#[derive(Serialize, ToSchema)]
struct GateFenceView {
    configuration_digest: String,
    epoch: u64,
    kill_switch_engaged: bool,
}

impl From<SponsorshipGateFence> for GateFenceView {
    fn from(value: SponsorshipGateFence) -> Self {
        Self {
            configuration_digest: value.configuration_digest,
            epoch: value.epoch,
            kill_switch_engaged: value.kill_switch_engaged,
        }
    }
}

fn enum_code(value: impl Serialize) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .expect("sponsorship enums serialize as strings")
}

fn distinct(
    values: Vec<String>,
    field: &'static str,
) -> Result<BTreeSet<String>, SponsorshipError> {
    let count = values.len();
    let values: BTreeSet<_> = values.into_iter().collect();
    if values.len() != count {
        return Err(layer2_core::TriggerValidationError::field(
            field,
            "contains a duplicate value",
        )
        .into());
    }
    Ok(values)
}

fn parse_credential_id(value: String) -> Result<SponsoredCredentialId, SponsorshipError> {
    SponsoredCredentialId::parse(value).map_err(Into::into)
}

fn parse_policy_id(value: String) -> Result<SponsorshipPolicyId, SponsorshipError> {
    SponsorshipPolicyId::parse(value).map_err(Into::into)
}

fn parse_grant_id(value: String) -> Result<SponsoredGrantId, SponsorshipError> {
    SponsoredGrantId::parse(value).map_err(Into::into)
}

fn parse_reservation_id(value: String) -> Result<SponsoredUsageReservationId, SponsorshipError> {
    SponsoredUsageReservationId::parse(value).map_err(Into::into)
}

fn parse_launch_id(value: String) -> Result<LaunchId, SponsorshipError> {
    LaunchId::parse(value).map_err(Into::into)
}

#[utoipa::path(
    post,
    path = "/api/v1/sponsorships/credentials",
    tag = "sponsorships",
    request_body = RegisterCredentialRequest,
    responses(
        (status = 201, description = "Provider credential registered", body = CredentialView),
        (status = 422, description = "Credential or provider policy is invalid", body = super::ProblemDocument, content_type = "application/problem+json"),
        (status = 503, description = "Sponsorship or its provider is unavailable", body = super::ProblemDocument, content_type = "application/problem+json")
    )
)]
async fn register_credential(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    body: Result<SponsorshipJson<RegisterCredentialRequest>, SponsorshipBodyProblem>,
) -> Result<Response, SponsorshipRequestFailure> {
    let SponsorshipJson(body) = body?;
    let credential = provider_credential_secret(body.credential)?;
    let owner = body.owner.resolve(&caller.subject)?;
    let registered = service(&state)?
        .register_credential(
            &caller,
            RegisterSponsoredCredential {
                stable_request_id: body.stable_request_id,
                owner,
                provider: body.provider,
                provider_policy_key: body.provider_policy_key,
                credential,
            },
        )
        .await?;
    Ok(private_json(
        StatusCode::CREATED,
        CredentialView::from(registered),
    ))
}

#[utoipa::path(
    get,
    path = "/api/v1/sponsorships/credentials",
    tag = "sponsorships",
    params(("limit" = Option<u32>, Query), ("cursor" = Option<String>, Query)),
    responses(
        (status = 200, description = "Newest-first secret-free owner credential page", body = [CredentialView]),
        (status = 422, description = "Continuation is invalid for this collection", body = super::ProblemDocument, content_type = "application/problem+json"),
        (status = 503, description = "Sponsorship persistence is unavailable", body = super::ProblemDocument, content_type = "application/problem+json")
    )
)]
async fn list_credentials(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    params: Result<Query<SponsorshipPageParams>, axum::extract::rejection::QueryRejection>,
) -> Result<Response, SponsorshipRequestFailure> {
    let Query(params) = params.map_err(SponsorshipQueryProblem::from)?;
    let request = params
        .store_request(SponsorshipPageCollection::Credentials)
        .map_err(SponsorshipError::from)?;
    let page = service(&state)?.credential_views(&caller, &request).await?;
    let headers = params.next_headers(
        "/api/v1/sponsorships/credentials",
        SponsorshipPageCollection::Credentials,
        page.next.as_ref(),
    );
    Ok(private_page_json(
        page.items
            .into_iter()
            .map(CredentialView::from)
            .collect::<Vec<_>>(),
        headers,
    ))
}

#[utoipa::path(
    get,
    path = "/api/v1/sponsorships/credentials/{credential_id}",
    tag = "sponsorships",
    params(("credential_id" = String, Path)),
    responses(
        (status = 200, description = "Secret-free provider credential metadata", body = CredentialView),
        (status = 404, description = "Credential absent or not controlled by the caller", body = super::ProblemDocument, content_type = "application/problem+json"),
        (status = 503, description = "Sponsorship persistence is unavailable", body = super::ProblemDocument, content_type = "application/problem+json")
    )
)]
async fn read_credential(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(credential_id): Path<String>,
) -> Result<Response, SponsorshipRequestFailure> {
    let credential_id = parse_credential_id(credential_id)?;
    let credential = service(&state)?
        .credential_view(&caller, &credential_id)
        .await?;
    Ok(private_json(
        StatusCode::OK,
        CredentialView::from(credential),
    ))
}

#[utoipa::path(
    post,
    path = "/api/v1/sponsorships/credentials/{credential_id}/rotate",
    tag = "sponsorships",
    params(("credential_id" = String, Path)),
    request_body = RotateCredentialRequest,
    responses(
        (status = 200, description = "Credential rotated or an identical request replayed", body = CredentialView),
        (status = 404, description = "Credential absent or not controlled by the caller", body = super::ProblemDocument, content_type = "application/problem+json"),
        (status = 409, description = "Credential or provider operation is not ready", body = super::ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "Rotation request is invalid", body = super::ProblemDocument, content_type = "application/problem+json"),
        (status = 503, description = "Sponsorship or its provider is unavailable", body = super::ProblemDocument, content_type = "application/problem+json")
    )
)]
async fn rotate_credential(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(credential_id): Path<String>,
    body: Result<SponsorshipJson<RotateCredentialRequest>, SponsorshipBodyProblem>,
) -> Result<Response, SponsorshipRequestFailure> {
    let SponsorshipJson(body) = body?;
    let replacement = provider_credential_secret(body.replacement)?;
    let credential = service(&state)?
        .rotate_credential(
            &caller,
            RotateSponsoredCredential {
                credential_id: parse_credential_id(credential_id)?,
                stable_request_id: body.stable_request_id,
                replacement,
            },
        )
        .await?;
    Ok(private_json(
        StatusCode::OK,
        CredentialView::from(credential),
    ))
}

#[utoipa::path(
    post,
    path = "/api/v1/sponsorships/credentials/{credential_id}/revoke",
    tag = "sponsorships",
    params(("credential_id" = String, Path)),
    request_body = StableRequest,
    responses(
        (status = 200, description = "Credential revoked or an identical request replayed", body = CredentialView),
        (status = 404, description = "Credential absent or not controlled by the caller", body = super::ProblemDocument, content_type = "application/problem+json"),
        (status = 409, description = "Credential or provider operation is not ready", body = super::ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "Revocation request is invalid", body = super::ProblemDocument, content_type = "application/problem+json"),
        (status = 503, description = "Provider or persistence is unavailable", body = super::ProblemDocument, content_type = "application/problem+json")
    )
)]
async fn revoke_credential(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(credential_id): Path<String>,
    body: Result<SponsorshipJson<StableRequest>, SponsorshipBodyProblem>,
) -> Result<Response, SponsorshipRequestFailure> {
    let SponsorshipJson(body) = body?;
    let credential = service(&state)?
        .revoke_credential(
            &caller,
            &parse_credential_id(credential_id)?,
            &body.stable_request_id,
        )
        .await?;
    Ok(private_json(
        StatusCode::OK,
        CredentialView::from(credential),
    ))
}

#[utoipa::path(
    post,
    path = "/api/v1/sponsorships/policies",
    tag = "sponsorships",
    request_body = CreatePolicyRequest,
    responses(
        (status = 201, description = "Sponsorship policy created", body = PolicyView),
        (status = 404, description = "Credential absent or not controlled by the caller", body = super::ProblemDocument, content_type = "application/problem+json"),
        (status = 409, description = "Credential or policy is not available", body = super::ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "Policy is invalid or exceeds deployment ceilings", body = super::ProblemDocument, content_type = "application/problem+json"),
        (status = 503, description = "Sponsorship persistence is unavailable", body = super::ProblemDocument, content_type = "application/problem+json")
    )
)]
async fn create_policy(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    body: Result<SponsorshipJson<CreatePolicyRequest>, SponsorshipBodyProblem>,
) -> Result<Response, SponsorshipRequestFailure> {
    let SponsorshipJson(body) = body?;
    let CreatePolicyRequest {
        tenant_id,
        owner,
        credential_id,
        provider,
        provider_policy_key,
        allowed_models,
        allowed_resources,
        per_launch_quota,
        aggregate_quota,
        per_launch_budget_micros,
        aggregate_budget_micros,
        max_requests_per_minute,
        max_concurrent_launches,
        expires_at,
    } = body;
    let owner = owner.resolve(&caller.subject)?;
    let sponsorships = service(&state)?;
    let policy = sponsorships
        .create_policy(
            &caller,
            CreateSponsorshipPolicy {
                tenant_id,
                owner,
                credential_id: parse_credential_id(credential_id)?,
                provider_policy: SponsoredProviderPolicy {
                    provider,
                    provider_policy_key,
                    allowed_models: distinct(allowed_models, "provider_policy.allowed_models")?,
                    allowed_resources: distinct(
                        allowed_resources,
                        "provider_policy.allowed_resources",
                    )?,
                },
                per_launch_quota: per_launch_quota.into(),
                aggregate_quota: aggregate_quota.into(),
                per_launch_budget_micros,
                aggregate_budget_micros,
                max_requests_per_minute,
                max_concurrent_launches,
                expires_at,
            },
        )
        .await?;
    let policy = sponsorships.policy_view(&caller, &policy.id).await?;
    Ok(private_json(StatusCode::CREATED, PolicyView::from(policy)))
}

#[utoipa::path(
    get,
    path = "/api/v1/sponsorships/policies",
    tag = "sponsorships",
    params(("limit" = Option<u32>, Query), ("cursor" = Option<String>, Query)),
    responses(
        (status = 200, description = "Newest-first owner policy page with durable accounting", body = [PolicyView]),
        (status = 422, description = "Continuation is invalid for this collection", body = super::ProblemDocument, content_type = "application/problem+json"),
        (status = 503, description = "Sponsorship persistence is unavailable", body = super::ProblemDocument, content_type = "application/problem+json")
    )
)]
async fn list_policies(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    params: Result<Query<SponsorshipPageParams>, axum::extract::rejection::QueryRejection>,
) -> Result<Response, SponsorshipRequestFailure> {
    let Query(params) = params.map_err(SponsorshipQueryProblem::from)?;
    let request = params
        .store_request(SponsorshipPageCollection::Policies)
        .map_err(SponsorshipError::from)?;
    let page = service(&state)?.policy_views(&caller, &request).await?;
    let headers = params.next_headers(
        "/api/v1/sponsorships/policies",
        SponsorshipPageCollection::Policies,
        page.next.as_ref(),
    );
    Ok(private_page_json(
        page.items
            .into_iter()
            .map(PolicyView::from)
            .collect::<Vec<_>>(),
        headers,
    ))
}

#[utoipa::path(
    get,
    path = "/api/v1/sponsorships/policies/{policy_id}",
    tag = "sponsorships",
    params(("policy_id" = String, Path)),
    responses(
        (status = 200, description = "Owner-safe sponsorship policy and usage", body = PolicyView),
        (status = 404, description = "Policy absent or not controlled by the caller", body = super::ProblemDocument, content_type = "application/problem+json"),
        (status = 503, description = "Sponsorship persistence is unavailable", body = super::ProblemDocument, content_type = "application/problem+json")
    )
)]
async fn read_policy(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(policy_id): Path<String>,
) -> Result<Response, SponsorshipRequestFailure> {
    let policy = service(&state)?
        .policy_view(&caller, &parse_policy_id(policy_id)?)
        .await?;
    Ok(private_json(StatusCode::OK, PolicyView::from(policy)))
}

#[utoipa::path(
    post,
    path = "/api/v1/sponsorships/policies/{policy_id}/suspend",
    tag = "sponsorships",
    params(("policy_id" = String, Path)),
    responses(
        (status = 200, description = "Policy suspended", body = PolicyView),
        (status = 404, description = "Policy absent or not controlled by the caller", body = super::ProblemDocument, content_type = "application/problem+json"),
        (status = 409, description = "Policy changed concurrently", body = super::ProblemDocument, content_type = "application/problem+json"),
        (status = 503, description = "Sponsorship persistence is unavailable", body = super::ProblemDocument, content_type = "application/problem+json")
    )
)]
async fn suspend_policy(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(policy_id): Path<String>,
) -> Result<Response, SponsorshipRequestFailure> {
    let sponsorships = service(&state)?;
    let policy_id = parse_policy_id(policy_id)?;
    sponsorships.suspend_policy(&caller, &policy_id).await?;
    let policy = sponsorships.policy_view(&caller, &policy_id).await?;
    Ok(private_json(StatusCode::OK, PolicyView::from(policy)))
}

#[utoipa::path(
    post,
    path = "/api/v1/sponsorships/policies/{policy_id}/revoke",
    tag = "sponsorships",
    params(("policy_id" = String, Path)),
    responses(
        (status = 200, description = "Policy revoked", body = PolicyView),
        (status = 404, description = "Policy absent or not controlled by the caller", body = super::ProblemDocument, content_type = "application/problem+json"),
        (status = 409, description = "Policy changed concurrently", body = super::ProblemDocument, content_type = "application/problem+json"),
        (status = 503, description = "Sponsorship persistence is unavailable", body = super::ProblemDocument, content_type = "application/problem+json")
    )
)]
async fn revoke_policy(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(policy_id): Path<String>,
) -> Result<Response, SponsorshipRequestFailure> {
    let sponsorships = service(&state)?;
    let policy_id = parse_policy_id(policy_id)?;
    sponsorships.revoke_policy(&caller, &policy_id).await?;
    let policy = sponsorships.policy_view(&caller, &policy_id).await?;
    Ok(private_json(StatusCode::OK, PolicyView::from(policy)))
}

#[utoipa::path(
    get,
    path = "/api/v1/launches/{launch_id}/sponsorship/terms",
    tag = "sponsorships",
    params(("launch_id" = String, Path)),
    responses(
        (status = 200, description = "Current sponsored terms bound to this Launch snapshot", body = TermsReviewView),
        (status = 404, description = "Launch absent or not controlled by the caller", body = super::ProblemDocument, content_type = "application/problem+json"),
        (status = 409, description = "Policy or Launch is not ready for terms review", body = super::ProblemDocument, content_type = "application/problem+json"),
        (status = 503, description = "Sponsorship is unavailable", body = super::ProblemDocument, content_type = "application/problem+json")
    )
)]
async fn review_terms(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(launch_id): Path<String>,
) -> Result<Response, SponsorshipRequestFailure> {
    let review = service(&state)?
        .terms_review(&caller, &parse_launch_id(launch_id)?)
        .await?;
    Ok(private_json(StatusCode::OK, TermsReviewView::from(review)))
}

#[utoipa::path(
    post,
    path = "/api/v1/launches/{launch_id}/sponsorship/acceptance",
    tag = "sponsorships",
    params(("launch_id" = String, Path)),
    request_body = AcceptTermsRequest,
    responses(
        (status = 201, description = "Exact sponsored terms accepted or replayed", body = AcceptanceView),
        (status = 404, description = "Launch or policy absent or not controlled by the caller", body = super::ProblemDocument, content_type = "application/problem+json"),
        (status = 409, description = "Terms, Launch authority, or gate changed", body = super::ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "Acceptance is invalid", body = super::ProblemDocument, content_type = "application/problem+json"),
        (status = 503, description = "Sponsorship is unavailable", body = super::ProblemDocument, content_type = "application/problem+json")
    )
)]
async fn accept_terms(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(launch_id): Path<String>,
    body: Result<SponsorshipJson<AcceptTermsRequest>, SponsorshipBodyProblem>,
) -> Result<Response, SponsorshipRequestFailure> {
    let SponsorshipJson(body) = body?;
    let acceptance = service(&state)?
        .accept_terms(
            &caller,
            AcceptSponsoredTerms {
                launch_id: parse_launch_id(launch_id)?,
                policy_id: parse_policy_id(body.policy_id)?,
                terms_digest: body.terms_digest,
                launch_snapshot_digest: body.launch_snapshot_digest,
                idempotency_key: body.idempotency_key,
            },
        )
        .await?;
    Ok(private_json(
        StatusCode::CREATED,
        AcceptanceView::from(acceptance),
    ))
}

#[utoipa::path(
    get,
    path = "/api/v1/launches/{launch_id}/sponsorship/grant",
    tag = "sponsorships",
    params(("launch_id" = String, Path)),
    responses(
        (status = 200, description = "Secret-free grant for this Launch", body = GrantView),
        (status = 404, description = "Grant absent or not controlled by the caller", body = super::ProblemDocument, content_type = "application/problem+json"),
        (status = 503, description = "Sponsorship persistence is unavailable", body = super::ProblemDocument, content_type = "application/problem+json")
    )
)]
async fn read_launch_grant(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(launch_id): Path<String>,
) -> Result<Response, SponsorshipRequestFailure> {
    let grant = service(&state)?
        .grant_for_launch_view(&caller, &parse_launch_id(launch_id)?)
        .await?
        .ok_or(SponsorshipError::NotFound)?;
    Ok(private_json(StatusCode::OK, GrantView::from(grant)))
}

#[utoipa::path(
    get,
    path = "/api/v1/sponsorships/grants",
    tag = "sponsorships",
    params(
        ("role" = GrantCollectionRole, Query),
        ("limit" = Option<u32>, Query),
        ("cursor" = Option<String>, Query)
    ),
    responses(
        (status = 200, description = "Newest-first contributor or sponsor grant page", body = [GrantView]),
        (status = 422, description = "Role or continuation is invalid for this collection", body = super::ProblemDocument, content_type = "application/problem+json"),
        (status = 503, description = "Sponsorship persistence is unavailable", body = super::ProblemDocument, content_type = "application/problem+json")
    )
)]
async fn list_grants(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    params: Result<Query<GrantPageParams>, axum::extract::rejection::QueryRejection>,
) -> Result<Response, SponsorshipRequestFailure> {
    let Query(params) = params.map_err(SponsorshipQueryProblem::from)?;
    let collection = params.collection();
    let page_params = params.page();
    let request = page_params
        .store_request(collection)
        .map_err(SponsorshipError::from)?;
    let sponsorships = service(&state)?;
    let page = match params.role {
        GrantCollectionRole::Contributor => {
            sponsorships
                .contributor_grant_views(&caller, &request)
                .await?
        }
        GrantCollectionRole::Sponsor => sponsorships.sponsor_grant_views(&caller, &request).await?,
    };
    let headers = page_params.next_headers(
        "/api/v1/sponsorships/grants",
        collection,
        page.next.as_ref(),
    );
    Ok(private_page_json(
        page.items
            .into_iter()
            .map(GrantView::from)
            .collect::<Vec<_>>(),
        headers,
    ))
}

#[utoipa::path(
    post,
    path = "/api/v1/sponsorships/grants/{grant_id}/revoke",
    tag = "sponsorships",
    params(("grant_id" = String, Path)),
    request_body = StableRequest,
    responses(
        (status = 200, description = "Grant revoked or an identical request replayed", body = GrantView),
        (status = 404, description = "Grant absent or not controlled by the caller", body = super::ProblemDocument, content_type = "application/problem+json"),
        (status = 409, description = "Grant or provider operation is not ready", body = super::ProblemDocument, content_type = "application/problem+json"),
        (status = 503, description = "Provider or persistence is unavailable", body = super::ProblemDocument, content_type = "application/problem+json")
    )
)]
async fn revoke_grant(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(grant_id): Path<String>,
    body: Result<SponsorshipJson<StableRequest>, SponsorshipBodyProblem>,
) -> Result<Response, SponsorshipRequestFailure> {
    let SponsorshipJson(body) = body?;
    let sponsorships = service(&state)?;
    let grant_id = parse_grant_id(grant_id)?;
    sponsorships
        .revoke_grant(&caller, &grant_id, &body.stable_request_id)
        .await?;
    let grant = sponsorships.grant_view(&caller, &grant_id).await?;
    Ok(private_json(StatusCode::OK, GrantView::from(grant)))
}

#[utoipa::path(
    post,
    path = "/api/v1/sponsorships/grants/{grant_id}/close",
    tag = "sponsorships",
    params(("grant_id" = String, Path)),
    request_body = StableRequest,
    responses(
        (status = 200, description = "Terminal Launch grant closed or replayed", body = GrantView),
        (status = 404, description = "Grant absent or not controlled by the caller", body = super::ProblemDocument, content_type = "application/problem+json"),
        (status = 409, description = "Launch, grant, or provider operation is not ready", body = super::ProblemDocument, content_type = "application/problem+json"),
        (status = 503, description = "Provider or persistence is unavailable", body = super::ProblemDocument, content_type = "application/problem+json")
    )
)]
async fn close_grant(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(grant_id): Path<String>,
    body: Result<SponsorshipJson<StableRequest>, SponsorshipBodyProblem>,
) -> Result<Response, SponsorshipRequestFailure> {
    let SponsorshipJson(body) = body?;
    let sponsorships = service(&state)?;
    let grant_id = parse_grant_id(grant_id)?;
    sponsorships
        .close_grant(&caller, &grant_id, &body.stable_request_id)
        .await?;
    let grant = sponsorships.grant_view(&caller, &grant_id).await?;
    Ok(private_json(StatusCode::OK, GrantView::from(grant)))
}

#[utoipa::path(
    post,
    path = "/api/v1/sponsorships/usage/reservations",
    tag = "sponsorships",
    request_body = ReserveUsageRequest,
    responses(
        (status = 201, description = "Usage maximum reserved; exact replays return the durable reservation", body = ReservationOutcomeView),
        (status = 200, description = "Existing identical reservation replayed", body = ReservationOutcomeView),
        (status = 404, description = "Grant absent or not controlled by the caller", body = super::ProblemDocument, content_type = "application/problem+json"),
        (status = 409, description = "Grant, quota, budget, or idempotency state conflicts", body = super::ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "Usage scope or request is invalid", body = super::ProblemDocument, content_type = "application/problem+json"),
        (status = 429, description = "Sponsored request rate is exhausted", body = super::ProblemDocument, content_type = "application/problem+json"),
        (status = 503, description = "Sponsorship, pricing, or persistence is unavailable", body = super::ProblemDocument, content_type = "application/problem+json")
    )
)]
async fn reserve_usage(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    body: Result<SponsorshipJson<ReserveUsageRequest>, SponsorshipBodyProblem>,
) -> Result<Response, SponsorshipRequestFailure> {
    let SponsorshipJson(body) = body?;
    let outcome = service(&state)?
        .reserve_usage(
            &caller,
            RequestSponsoredUsage {
                stable_request_id: body.stable_request_id,
                grant_id: parse_grant_id(body.grant_id)?,
                provider: body.provider,
                model: body.model,
                resource: body.resource,
                maximum_quota: body.maximum_quota.into(),
                capability: body.capability.resolve()?,
            },
        )
        .await?;
    let status = match outcome.disposition {
        ReservationDisposition::Created => StatusCode::CREATED,
        ReservationDisposition::Existing => StatusCode::OK,
    };
    Ok(private_json(status, ReservationOutcomeView::from(outcome)))
}

#[utoipa::path(
    post,
    path = "/api/v1/sponsorships/usage/reconciliations",
    tag = "sponsorships",
    request_body = ReconcileUsageRequest,
    responses(
        (status = 200, description = "Usage reconciled by an authorized metering worker", body = ReservationView),
        (status = 404, description = "Reservation is absent", body = super::ProblemDocument, content_type = "application/problem+json"),
        (status = 409, description = "Reservation fingerprint, outcome, or accounting conflicts", body = super::ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "Usage report is invalid", body = super::ProblemDocument, content_type = "application/problem+json"),
        (status = 503, description = "Pricing or persistence is unavailable", body = super::ProblemDocument, content_type = "application/problem+json")
    )
)]
async fn reconcile_usage(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    body: Result<SponsorshipJson<ReconcileUsageRequest>, SponsorshipBodyProblem>,
) -> Result<Response, SponsorshipRequestFailure> {
    let SponsorshipJson(body) = body?;
    let reservation = service(&state)?
        .reconcile_usage(
            &caller,
            ReportSponsoredUsage {
                reservation_id: parse_reservation_id(body.reservation_id)?,
                request_fingerprint: body.request_fingerprint,
                actual_quota: body.actual_quota.map(Into::into),
                outcome: body.outcome.into(),
            },
        )
        .await?;
    Ok(private_json(
        StatusCode::OK,
        ReservationView::from(reservation),
    ))
}

#[utoipa::path(
    get,
    path = "/api/v1/sponsorships/audit",
    tag = "sponsorships",
    params(("limit" = Option<usize>, Query)),
    responses(
        (status = 200, description = "Authenticated actor's pseudonymized sponsorship audit events", body = [AuditEventView]),
        (status = 422, description = "Audit page request is invalid", body = super::ProblemDocument, content_type = "application/problem+json"),
        (status = 503, description = "Sponsorship persistence is unavailable", body = super::ProblemDocument, content_type = "application/problem+json")
    )
)]
async fn read_actor_audit(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    params: Result<Query<AuditParams>, axum::extract::rejection::QueryRejection>,
) -> Result<Response, SponsorshipRequestFailure> {
    let Query(params) = params.map_err(SponsorshipQueryProblem::from)?;
    let events = service(&state)?
        .actor_audit_events(&caller, params.limit)
        .await?
        .into_iter()
        .map(AuditEventView::from)
        .collect::<Vec<_>>();
    Ok(private_json(StatusCode::OK, events))
}

#[utoipa::path(
    post,
    path = "/api/v1/sponsorships/kill-switch",
    tag = "sponsorships",
    responses(
        (status = 200, description = "Durable sponsorship kill switch engaged", body = GateFenceView),
        (status = 404, description = "Operation is not authorized", body = super::ProblemDocument, content_type = "application/problem+json"),
        (status = 409, description = "Gate fence changed concurrently", body = super::ProblemDocument, content_type = "application/problem+json"),
        (status = 503, description = "Sponsorship persistence is unavailable", body = super::ProblemDocument, content_type = "application/problem+json")
    )
)]
async fn engage_kill_switch(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
) -> Result<Response, SponsorshipRequestFailure> {
    let fence = service(&state)?.engage_kill_switch(&caller).await?;
    Ok(private_json(StatusCode::OK, GateFenceView::from(fence)))
}

enum SponsorshipBodyProblem {
    Data,
    Syntax,
    MissingContentType,
    Unreadable,
    TooLarge,
}

impl IntoResponse for SponsorshipBodyProblem {
    fn into_response(self) -> Response {
        match self {
            Self::Data => {
                json_problem_response(StatusCode::UNPROCESSABLE_ENTITY, JsonProblemKind::Data)
            }
            Self::Syntax => json_problem_response(StatusCode::BAD_REQUEST, JsonProblemKind::Syntax),
            Self::MissingContentType => json_problem_response(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                JsonProblemKind::MissingContentType,
            ),
            Self::Unreadable => {
                json_problem_response(StatusCode::BAD_REQUEST, JsonProblemKind::Unreadable)
            }
            Self::TooLarge => HttpProblem::RequestTooLarge.into_response(),
        }
    }
}

struct SponsorshipQueryProblem(StatusCode);

impl From<axum::extract::rejection::QueryRejection> for SponsorshipQueryProblem {
    fn from(error: axum::extract::rejection::QueryRejection) -> Self {
        Self(error.status())
    }
}

impl IntoResponse for SponsorshipQueryProblem {
    fn into_response(self) -> Response {
        let body = problem_document(
            self.0,
            "invalid-query",
            "The query string does not match this operation's request schema.",
            "invalid_query",
        );
        problem_response(self.0, body)
    }
}

enum SponsorshipRequestFailure {
    Body(SponsorshipBodyProblem),
    Query(SponsorshipQueryProblem),
    Domain(SponsorshipProblem),
}

impl From<SponsorshipBodyProblem> for SponsorshipRequestFailure {
    fn from(value: SponsorshipBodyProblem) -> Self {
        Self::Body(value)
    }
}

impl From<SponsorshipQueryProblem> for SponsorshipRequestFailure {
    fn from(value: SponsorshipQueryProblem) -> Self {
        Self::Query(value)
    }
}

impl From<SponsorshipError> for SponsorshipRequestFailure {
    fn from(value: SponsorshipError) -> Self {
        Self::Domain(SponsorshipProblem(value))
    }
}

impl IntoResponse for SponsorshipRequestFailure {
    fn into_response(self) -> Response {
        let mut response = match self {
            Self::Body(problem) => problem.into_response(),
            Self::Query(problem) => problem.into_response(),
            Self::Domain(problem) => problem.into_response(),
        };
        add_private_headers(&mut response);
        response
    }
}

pub(crate) struct SponsorshipProblem(pub SponsorshipError);

impl IntoResponse for SponsorshipProblem {
    fn into_response(self) -> Response {
        use layer2_core::SponsorshipStoreError as Store;

        let validation_field = match &self.0 {
            SponsorshipError::Validation(error) => error.field.clone(),
            _ => None,
        };
        let (status, slug, code, detail, retry_after) = match self.0 {
            SponsorshipError::Disabled
            | SponsorshipError::KillSwitch
            | SponsorshipError::CapabilityEvidenceMissing
            | SponsorshipError::InvalidGate => (
                StatusCode::SERVICE_UNAVAILABLE,
                "sponsorship-unavailable",
                "sponsorship_unavailable",
                "Sponsored grants are not available in this deployment.",
                None,
            ),
            SponsorshipError::NotFound | SponsorshipError::Forbidden => (
                StatusCode::NOT_FOUND,
                "sponsorship-resource-not-found",
                "sponsorship_resource_not_found",
                "The requested sponsored resource does not exist.",
                None,
            ),
            SponsorshipError::PolicyUnavailable => (
                StatusCode::CONFLICT,
                "sponsorship-policy-unavailable",
                "sponsorship_policy_unavailable",
                "The sponsored policy is not available for this operation.",
                None,
            ),
            SponsorshipError::CredentialUnavailable => (
                StatusCode::CONFLICT,
                "sponsorship-credential-unavailable",
                "sponsorship_credential_unavailable",
                "The sponsored credential is not active for this operation.",
                None,
            ),
            SponsorshipError::ProviderPolicyDenied => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "sponsorship-provider-policy-denied",
                "sponsorship_provider_policy_denied",
                "The requested provider policy is not admitted by this deployment.",
                None,
            ),
            SponsorshipError::TermsNotAccepted => (
                StatusCode::CONFLICT,
                "sponsorship-terms-not-accepted",
                "sponsorship_terms_not_accepted",
                "The current sponsored terms have not been accepted for this Launch.",
                None,
            ),
            SponsorshipError::TermsChanged => (
                StatusCode::CONFLICT,
                "sponsorship-terms-changed",
                "sponsorship_terms_changed",
                "The sponsored terms changed; review and accept the current terms.",
                None,
            ),
            SponsorshipError::BillingRejected => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "sponsorship-billing-rejected",
                "sponsorship_billing_rejected",
                "Billing did not authorize this sponsored grant.",
                None,
            ),
            SponsorshipError::BillingAmbiguous | SponsorshipError::ProviderAmbiguous => (
                StatusCode::CONFLICT,
                "sponsorship-operation-ambiguous",
                "sponsorship_operation_ambiguous",
                "The external operation has no definitive result; reconciliation is required.",
                None,
            ),
            SponsorshipError::BillingUnavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "sponsorship-billing-unavailable",
                "sponsorship_billing_unavailable",
                "Sponsored billing is temporarily unavailable.",
                None,
            ),
            SponsorshipError::ProviderUnavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "sponsorship-provider-unavailable",
                "sponsorship_provider_unavailable",
                "The sponsored provider is temporarily unavailable.",
                None,
            ),
            SponsorshipError::ProviderRejected => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "sponsorship-provider-rejected",
                "sponsorship_provider_rejected",
                "The sponsored provider refused this operation.",
                None,
            ),
            SponsorshipError::PricingUnavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "sponsorship-pricing-unavailable",
                "sponsorship_pricing_unavailable",
                "Trusted sponsored pricing is temporarily unavailable.",
                None,
            ),
            SponsorshipError::PricingUnsupported => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "sponsorship-pricing-unsupported",
                "sponsorship_pricing_unsupported",
                "The requested sponsored usage has no admitted trusted price.",
                None,
            ),
            SponsorshipError::ProviderOperationInProgress
            | SponsorshipError::ActivationInProgress => (
                StatusCode::CONFLICT,
                "sponsorship-operation-in-progress",
                "sponsorship_operation_in_progress",
                "Another request owns this sponsored operation; read current state before retrying.",
                None,
            ),
            SponsorshipError::ScopeDenied => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "sponsorship-scope-denied",
                "sponsorship_scope_denied",
                "The sponsored request does not match its admitted Launch, provider, model, or resource scope.",
                None,
            ),
            SponsorshipError::GrantUnavailable => (
                StatusCode::CONFLICT,
                "sponsorship-grant-unavailable",
                "sponsorship_grant_unavailable",
                "The sponsored grant cannot authorize new usage.",
                None,
            ),
            SponsorshipError::Validation(_) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid-sponsorship-request",
                "invalid_sponsorship_request",
                "The sponsorship request violates the declared policy.",
                None,
            ),
            SponsorshipError::Store(Store::Unavailable(_) | Store::Malformed(_)) => (
                StatusCode::SERVICE_UNAVAILABLE,
                "sponsorship-store-unavailable",
                "sponsorship_store_unavailable",
                "Sponsored state is temporarily unavailable.",
                None,
            ),
            SponsorshipError::Store(Store::RateExceeded) => (
                StatusCode::TOO_MANY_REQUESTS,
                "sponsorship-rate-exhausted",
                "sponsorship_rate_exhausted",
                "The sponsored request rate is exhausted.",
                Some(RATE_RETRY_AFTER_SECONDS),
            ),
            SponsorshipError::Store(
                Store::Conflict
                | Store::StaleGeneration
                | Store::GateChanged
                | Store::LaunchAuthorityChanged
                | Store::AcceptanceChanged
                | Store::GrantUnavailable
                | Store::LaunchQuotaExceeded
                | Store::AggregateQuotaExceeded
                | Store::LaunchBudgetExceeded
                | Store::AggregateBudgetExceeded
                | Store::ConcurrentLaunchLimit,
            ) => (
                StatusCode::CONFLICT,
                "sponsorship-state-conflict",
                "sponsorship_state_conflict",
                "Sponsored state changed or its current quota, budget, or concurrency allowance is exhausted.",
                None,
            ),
        };
        let mut body = problem_document(status, slug, detail, code);
        if let (Some(field), Some(object)) = (validation_field, body.as_object_mut()) {
            object.insert("field".into(), field.into());
        }
        let mut response = problem_response(status, body);
        add_private_headers(&mut response);
        if let Some(seconds) = retry_after {
            response.headers_mut().insert(
                header::RETRY_AFTER,
                HeaderValue::from_str(&seconds.to_string())
                    .expect("a positive integer is a valid Retry-After value"),
            );
        }
        response
    }
}

fn private_json(status: StatusCode, value: impl Serialize) -> Response {
    let mut response = (status, Json(value)).into_response();
    add_private_headers(&mut response);
    response
}

fn private_page_json(value: impl Serialize, headers: HeaderMap) -> Response {
    let mut response = (headers, Json(value)).into_response();
    add_private_headers(&mut response);
    response
}

fn add_private_headers(response: &mut Response) {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
}

#[cfg(test)]
mod tests {
    use axum::body::to_bytes;
    use layer2_core::{
        CredentialRoute, CredentialRouteDisclosure, ModelCredentialRoute, SponsoredCredentialState,
        SponsorshipStoreError, SPONSORSHIP_SCHEMA_VERSION,
    };
    use serde_json::Value;

    use super::*;

    async fn rendered(error: SponsorshipError) -> (Response, String) {
        let response = SponsorshipProblem(error).into_response();
        let (parts, body) = response.into_parts();
        let body = to_bytes(body, usize::MAX).await.unwrap();
        (
            Response::from_parts(parts, axum::body::Body::empty()),
            String::from_utf8(body.to_vec()).unwrap(),
        )
    }

    #[tokio::test]
    async fn missing_and_foreign_private_resources_are_indistinguishable() {
        let (missing_response, missing_body) = rendered(SponsorshipError::NotFound).await;
        let (foreign_response, foreign_body) = rendered(SponsorshipError::Forbidden).await;

        assert_eq!(missing_response.status(), StatusCode::NOT_FOUND);
        assert_eq!(foreign_response.status(), StatusCode::NOT_FOUND);
        assert_eq!(missing_body, foreign_body);
        assert_eq!(
            missing_response.headers()[header::CACHE_CONTROL],
            "no-store"
        );
    }

    #[tokio::test]
    async fn provider_store_and_credential_material_never_reach_wire_diagnostics() {
        const MARKER: &str = "private-provider-token-do-not-reflect";
        let (response, body) = rendered(SponsorshipError::Store(
            SponsorshipStoreError::Unavailable(MARKER.into()),
        ))
        .await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(!body.contains(MARKER));

        let credential = CredentialView::from(SponsoredCredentialView {
            id: SponsoredCredentialId("11111111-1111-4111-8111-111111111111".into()),
            provider: "provider".into(),
            provider_policy_key: "policy".into(),
            state: SponsoredCredentialState::Active,
            generation: 1,
            ownership_verified_at: "2026-08-29T00:00:00Z".into(),
            updated_at: "2026-08-29T00:00:00Z".into(),
        });
        let encoded = serde_json::to_value(credential).unwrap();
        assert_eq!(
            encoded
                .as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([
                "generation".into(),
                "id".into(),
                "ownership_verified_at".into(),
                "provider".into(),
                "provider_policy_key".into(),
                "state".into(),
                "updated_at".into(),
            ])
        );
        assert!(!encoded.to_string().contains(MARKER));
    }

    #[tokio::test]
    async fn rate_exhaustion_has_a_bounded_retry_after_problem() {
        let (response, body) =
            rendered(SponsorshipError::Store(SponsorshipStoreError::RateExceeded)).await;
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(response.headers()[header::RETRY_AFTER], "60");
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "application/problem+json"
        );
        let body: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(body["code"], "sponsorship_rate_exhausted");
    }

    #[test]
    fn terms_disclose_contributor_github_and_sponsored_model_routes() {
        let terms = TermsView::from(SponsoredGrantTerms {
            schema_version: SPONSORSHIP_SCHEMA_VERSION,
            policy_id: SponsorshipPolicyId("22222222-2222-4222-8222-222222222222".into()),
            policy_generation: 1,
            sponsor_display_name: "Example sponsor".into(),
            provider: "provider".into(),
            allowed_models: BTreeSet::from(["model".into()]),
            allowed_resources: BTreeSet::from(["conversation".into()]),
            per_launch_quota: SponsoredQuota {
                requests: 1,
                input_tokens: 100,
                output_tokens: 100,
            },
            aggregate_remaining_quota: SponsoredQuota {
                requests: 2,
                input_tokens: 200,
                output_tokens: 200,
            },
            per_launch_budget_micros: 1_000,
            aggregate_remaining_budget_micros: 2_000,
            max_requests_per_minute: 1,
            max_concurrent_launches: 1,
            available_launches: 1,
            expires_at: "2026-08-30T00:00:00Z".into(),
            credential_route: CredentialRouteDisclosure {
                github: CredentialRoute::ContributorConnection,
                model_provider: ModelCredentialRoute::SponsoredGrant,
            },
            digest: "a".repeat(64),
        });
        let encoded = serde_json::to_value(terms).unwrap();
        assert_eq!(encoded["github_credential_route"], "contributor_connection");
        assert_eq!(encoded["model_credential_route"], "sponsored_grant");
    }
}
