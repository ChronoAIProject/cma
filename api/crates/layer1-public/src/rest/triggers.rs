//! Public Trigger, connection, Launch, and progress resources.

use std::sync::Arc;
use std::time::SystemTime;

use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use layer2_core::{
    BillingMode, CommercialPolicy, Confirmation, ConfirmationAction, ContributionPolicy,
    CredentialRoute, EditableField, ExecutorPolicy, InputFieldDefinition, Launch, LaunchId,
    LaunchOverrides, LaunchStep, OperationStore, OverridePolicy, PublicProjectionPolicy,
    PublicTriggerId, RepositoryAccessPolicy, RequiredCapability, ResourcePolicy, ScopeClass,
    SetupPolicy, SourceMode, TriggerAvailability, TriggerConnection,
    TriggerConnectionAuthorization, TriggerConnectionManager, TriggerDraft, TriggerId,
    TriggerPageRequest, TriggerPreview, TriggerRepositorySelection, TriggerRevision,
    TriggerRevisionContent, TriggerRevisionId, TriggerServiceError, TriggerSessionPolicy,
    TriggerStoreError, TriggerTask, UpstreamRevision, WorkflowChangePolicy,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};

use super::{
    idempotency, problem_document, problem_response, BodyProblem, RequestFailure, TriggersState,
};
use crate::auth::AuthenticatedCaller;

const PUBLIC_PREVIEW_CACHE_SECONDS: u64 = 60;

pub(super) fn documented_router(state: TriggersState) -> OpenApiRouter {
    OpenApiRouter::new()
        .routes(routes!(public_preview))
        .routes(routes!(create_trigger, list_triggers))
        .routes(routes!(read_trigger, update_trigger))
        .routes(routes!(publish_trigger))
        .routes(routes!(list_revisions))
        .routes(routes!(read_revision))
        .routes(routes!(retire_revision))
        .routes(routes!(list_connections, begin_connection))
        .routes(routes!(reconnect_connection))
        .routes(routes!(preflight_connection))
        .routes(routes!(create_or_resume_launch))
        .routes(routes!(create_new_launch))
        .routes(routes!(list_launches))
        .routes(routes!(read_launch))
        .routes(routes!(update_launch_overrides))
        .routes(routes!(select_launch_connection))
        .routes(routes!(start_launch))
        .routes(routes!(cancel_launch))
        .routes(routes!(launch_progress))
        .with_state(state)
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct RepositorySelectionRequest {
    repository_id: u64,
    selected_ref: String,
    contribution_base_ref: String,
    #[schema(value_type = String)]
    source_mode: SourceMode,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct TriggerContentRequest {
    schema_version: u32,
    publisher_display_identity: String,
    repository: RepositorySelectionRequest,
    #[schema(value_type = Object)]
    task: TriggerTask,
    #[schema(value_type = Object)]
    session: TriggerSessionPolicy,
    #[schema(value_type = Object)]
    repository_access: RepositoryAccessPolicy,
    #[schema(value_type = Object)]
    setup: SetupPolicy,
    #[schema(value_type = Object)]
    contribution: ContributionPolicy,
    #[schema(value_type = Object)]
    commercial_policy: CommercialPolicy,
    #[schema(value_type = Object)]
    override_policy: OverridePolicy,
    #[schema(value_type = Object)]
    public_projection: PublicProjectionPolicy,
}

impl TriggerContentRequest {
    async fn resolve(
        self,
        connections: &dyn TriggerConnectionManager,
        subject: &layer2_core::SubjectId,
        selected_service_id: &str,
    ) -> Result<TriggerRevisionContent, TriggerServiceError> {
        let selected = connections
            .resolve_repository(
                subject,
                selected_service_id,
                self.repository.repository_id,
                &self.repository.selected_ref,
            )
            .await?;
        if self.repository.contribution_base_ref != self.repository.selected_ref {
            let base = connections
                .resolve_repository(
                    subject,
                    selected_service_id,
                    self.repository.repository_id,
                    &self.repository.contribution_base_ref,
                )
                .await?;
            if base.repository.repository_id != selected.repository.repository_id
                || base.repository.repository_node_id != selected.repository.repository_node_id
            {
                return Err(TriggerServiceError::Authority(
                    "the contribution base resolved to a different repository".into(),
                ));
            }
        }
        Ok(self.into_content(subject.clone(), selected))
    }

    fn into_content(
        self,
        subject: layer2_core::SubjectId,
        selected: TriggerRepositorySelection,
    ) -> TriggerRevisionContent {
        TriggerRevisionContent {
            schema_version: self.schema_version,
            trigger_id: TriggerId::new(),
            revision_number: 1,
            publisher_subject_id: subject,
            publisher_display_identity: self.publisher_display_identity,
            upstream: UpstreamRevision {
                repository: selected.repository,
                source_mode: self.repository.source_mode,
                observed_source_sha: Some(selected.source_sha),
                contribution_base_ref: self.repository.contribution_base_ref,
            },
            task: self.task,
            session: self.session,
            repository_access: self.repository_access,
            setup: self.setup,
            contribution: self.contribution,
            commercial_policy: self.commercial_policy,
            override_policy: self.override_policy,
            public_projection: self.public_projection,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct CreateTriggerRequest {
    selected_service_id: String,
    content: TriggerContentRequest,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct UpdateTriggerRequest {
    expected_version: u64,
    selected_service_id: String,
    content: TriggerContentRequest,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct PublishTriggerRequest {
    expected_version: u64,
    selected_service_id: String,
}

#[derive(Debug, Deserialize, ToSchema, Default)]
#[serde(deny_unknown_fields)]
struct EmptyCommand {}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct BeginConnectionRequest {
    intent_id: String,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct UpdateOverridesRequest {
    expected_revision: u64,
    #[schema(value_type = Object)]
    overrides: LaunchOverrides,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct SelectConnectionRequest {
    expected_revision: u64,
    service_id: String,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct ConfirmLaunchRequest {
    expected_revision: u64,
    snapshot_digest: String,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct NewLaunchRequest {
    acknowledge_additional_resources: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PageParams {
    #[serde(default = "default_page_limit")]
    limit: u32,
    #[serde(default)]
    cursor: Option<String>,
}

fn default_page_limit() -> u32 {
    layer2_core::DEFAULT_TRIGGER_PAGE_SIZE
}

impl Default for PageParams {
    fn default() -> Self {
        Self {
            limit: default_page_limit(),
            cursor: None,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProgressParams {
    #[serde(default)]
    after: u64,
    #[serde(default = "default_progress_limit")]
    limit: usize,
}

fn default_progress_limit() -> usize {
    100
}

#[derive(Debug, Serialize, ToSchema)]
struct SessionSizeView {
    cpu_millis: u32,
    memory_mib: u32,
}

impl From<layer2_core::SessionSize> for SessionSizeView {
    fn from(value: layer2_core::SessionSize) -> Self {
        Self {
            cpu_millis: value.cpu_millis(),
            memory_mib: value.memory_mib(),
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
struct TriggerPreviewView {
    public_id: String,
    availability: String,
    publisher_display_identity: String,
    repository_id: u64,
    repository_owner: String,
    repository_name: String,
    visibility: String,
    allow_forking: bool,
    public_summary: String,
    permission_ceiling: String,
    required_capabilities: Vec<String>,
    resource_default: SessionSizeView,
    resource_ceiling: SessionSizeView,
    timeout_default_seconds: u32,
    timeout_ceiling_seconds: u32,
    billing_mode: String,
    pricing_policy_reference: String,
    revision_digest: String,
}

impl From<TriggerPreview> for TriggerPreviewView {
    fn from(value: TriggerPreview) -> Self {
        Self {
            public_id: value.public_id.0,
            availability: enum_name(value.availability),
            publisher_display_identity: value.publisher_display_identity,
            repository_id: value.repository_id,
            repository_owner: value.repository_owner,
            repository_name: value.repository_name,
            visibility: enum_name(value.visibility),
            allow_forking: value.allow_forking,
            public_summary: value.public_summary,
            permission_ceiling: enum_name(value.permission_ceiling),
            required_capabilities: value
                .required_capabilities
                .into_iter()
                .map(enum_name)
                .collect(),
            resource_default: value.resource_default.into(),
            resource_ceiling: value.resource_ceiling.into(),
            timeout_default_seconds: value.timeout_default_seconds,
            timeout_ceiling_seconds: value.timeout_ceiling_seconds,
            billing_mode: enum_name(value.billing_mode),
            pricing_policy_reference: value.pricing_policy_reference,
            revision_digest: value.revision_digest,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
struct TriggerDraftView {
    id: String,
    version: u64,
    next_revision_number: u32,
    #[schema(value_type = Object)]
    content: Value,
    created_at: String,
    updated_at: String,
}

impl From<TriggerDraft> for TriggerDraftView {
    fn from(value: TriggerDraft) -> Self {
        Self {
            id: value.id.0,
            version: value.version,
            next_revision_number: value.next_revision_number,
            content: json_value(value.content),
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
struct PublisherAuthorityView {
    service_id: String,
    service_state_version: i64,
    account_id: u64,
    account_login: String,
    role: String,
    verified_at: String,
}

#[derive(Debug, Serialize, ToSchema)]
struct TriggerRevisionView {
    id: String,
    public_id: String,
    digest: String,
    availability: String,
    #[schema(value_type = Object)]
    content: Value,
    authority: PublisherAuthorityView,
    published_at: String,
    retired_at: Option<String>,
}

impl From<TriggerRevision> for TriggerRevisionView {
    fn from(value: TriggerRevision) -> Self {
        let authority = PublisherAuthorityView {
            service_id: value.publisher_authority.connection.service_id.clone(),
            service_state_version: value.publisher_authority.connection.service_state_version,
            account_id: value.publisher_authority.connection.account.account_id,
            account_login: value.publisher_authority.connection.account.login.clone(),
            role: enum_name(value.publisher_authority.role),
            verified_at: value.publisher_authority.verified_at.clone(),
        };
        Self {
            id: value.id.0,
            public_id: value.public_id.0,
            digest: value.digest,
            availability: enum_name(value.availability),
            content: json_value(value.content),
            authority,
            published_at: value.published_at,
            retired_at: value.retired_at,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
struct TriggerDraftPageView {
    items: Vec<TriggerDraftView>,
    next_cursor: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
struct TriggerRevisionPageView {
    items: Vec<TriggerRevisionView>,
    next_cursor: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
struct TriggerConnectionView {
    service_id: String,
    state_version: i64,
    state: String,
    account_id: Option<u64>,
    account_node_id: Option<String>,
    account_login: Option<String>,
    avatar_url: Option<String>,
    granted_scopes: Vec<String>,
    consent_observed_at: Option<String>,
}

impl From<TriggerConnection> for TriggerConnectionView {
    fn from(value: TriggerConnection) -> Self {
        let (account_id, account_node_id, account_login, avatar_url) = value
            .account
            .map(|account| {
                (
                    Some(account.account_id),
                    Some(account.account_node_id),
                    Some(account.login),
                    account.avatar_url,
                )
            })
            .unwrap_or_default();
        Self {
            service_id: value.service_id,
            state_version: value.state_version,
            state: enum_name(value.state),
            account_id,
            account_node_id,
            account_login,
            avatar_url,
            granted_scopes: value.granted_scopes,
            consent_observed_at: value.consent_observed_at,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
struct TriggerConnectionsView {
    connections: Vec<TriggerConnectionView>,
}

#[derive(Debug, Serialize, ToSchema)]
struct ConnectionAuthorizationView {
    service_id: String,
    authorization_url: String,
    attempt_nonce: String,
}

impl From<TriggerConnectionAuthorization> for ConnectionAuthorizationView {
    fn from(value: TriggerConnectionAuthorization) -> Self {
        Self {
            service_id: value.service_id,
            authorization_url: value.authorization_url,
            attempt_nonce: value.attempt_nonce,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
struct LaunchView {
    id: String,
    trigger_public_id: String,
    revision_digest: String,
    source_sha: String,
    state: String,
    revision_number: u64,
    #[schema(value_type = Object)]
    revision_content: Value,
    #[schema(value_type = Object)]
    blocked_reason: Option<Value>,
    #[schema(value_type = Object)]
    failure: Option<Value>,
    #[schema(value_type = Object)]
    overrides: Value,
    #[schema(value_type = Object)]
    effective: Value,
    #[schema(value_type = Object)]
    pricing: Option<Value>,
    connection: Option<TriggerConnectionView>,
    workflow_paths: Vec<String>,
    workflow_risk_acknowledged: bool,
    steps: Vec<LaunchStepView>,
    #[schema(value_type = Object)]
    fork: Option<Value>,
    #[schema(value_type = Object)]
    branch: Option<Value>,
    #[schema(value_type = Object)]
    workspace: Option<Value>,
    #[schema(value_type = Object)]
    session: Option<Value>,
    #[schema(value_type = Object)]
    reach: Option<Value>,
    #[schema(value_type = Object)]
    delivery: Option<Value>,
    #[schema(value_type = Object)]
    diff: Option<Value>,
    #[schema(value_type = Object)]
    contribution: Option<Value>,
    #[schema(value_type = Object)]
    pull_request: Option<Value>,
    created_at: String,
    updated_at: String,
    started_at: Option<String>,
    terminal_at: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
struct LaunchStepView {
    key: String,
    sequence: u32,
    status: String,
    started_at: Option<String>,
    completed_at: Option<String>,
    attempt: u32,
    safe_detail: String,
    remediation: String,
    external_resource_ids: std::collections::BTreeMap<String, String>,
    correlation_id: String,
}

impl From<LaunchStep> for LaunchStepView {
    fn from(value: LaunchStep) -> Self {
        Self {
            key: enum_name(value.key),
            sequence: value.sequence,
            status: enum_name(value.status),
            started_at: value.started_at,
            completed_at: value.completed_at,
            attempt: value.attempt,
            safe_detail: value.safe_detail,
            remediation: value.remediation,
            external_resource_ids: value.external_resource_ids,
            correlation_id: value.correlation_id,
        }
    }
}

impl From<Launch> for LaunchView {
    fn from(value: Launch) -> Self {
        let connection = value.connection.map(|connection| {
            TriggerConnectionView::from(TriggerConnection {
                service_id: connection.service_id,
                state_version: connection.service_state_version,
                state: connection.state,
                account: Some(connection.account),
                granted_scopes: connection.granted_scopes,
                consent_observed_at: Some(connection.consent_observed_at),
            })
        });
        Self {
            id: value.id.0,
            trigger_public_id: value.revision.public_id.0,
            revision_digest: value.revision_digest,
            source_sha: value.source_sha,
            state: enum_name(value.state),
            revision_number: value.revision_number,
            revision_content: json_value(value.revision.content),
            blocked_reason: value.blocked_reason.map(json_value),
            failure: value.failure.map(json_value),
            overrides: json_value(value.overrides),
            effective: json_value(value.effective),
            pricing: value.pricing.map(json_value),
            connection,
            workflow_paths: value.workflow_paths,
            workflow_risk_acknowledged: value.workflow_risk_acknowledged,
            steps: value.steps.into_iter().map(Into::into).collect(),
            fork: value.fork.map(json_value),
            branch: value.branch.map(json_value),
            workspace: value.workspace.map(json_value),
            session: value.session.map(json_value),
            reach: value.reach.map(json_value),
            delivery: value.delivery.map(json_value),
            diff: value.diff.map(json_value),
            contribution: value.contribution.map(json_value),
            pull_request: value.pull_request.map(json_value),
            created_at: value.created_at,
            updated_at: value.updated_at,
            started_at: value.started_at,
            terminal_at: value.terminal_at,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
struct LaunchPageView {
    items: Vec<LaunchView>,
    next_cursor: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
struct LaunchProgressView {
    launch_id: String,
    #[schema(value_type = Vec<Object>)]
    events: Vec<Value>,
    next_cursor: u64,
}

fn json_value(value: impl Serialize) -> Value {
    serde_json::to_value(value).expect("core Trigger projections are serializable")
}

fn enum_name(value: impl Serialize) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_else(|| "unknown".to_string())
}

fn now() -> String {
    layer2_core::rfc3339_at(SystemTime::now())
}

fn correlation_id(headers: &HeaderMap) -> String {
    headers
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string())
}

async fn command<T: DeserializeOwned>(
    key: idempotency::IdempotencyKey,
    operations: Arc<dyn OperationStore>,
    caller: &layer2_core::Caller,
    operation: String,
    body: Result<idempotency::RequestBody<T>, BodyProblem>,
) -> Result<(idempotency::Idempotency, Result<T, BodyProblem>, String), Response> {
    let key = key.required().map_err(IntoResponse::into_response)?;
    let confirmation_hash = key
        .confirmation_hash(&operation)
        .expect("a required operation key has a digest");
    let (body, fingerprint) = match body {
        Ok(body) => body.into_parts(),
        Err(error) => return Err(RequestFailure::from(error).into_response()),
    };
    let guard = match idempotency::Idempotency::begin(
        key,
        operations,
        caller,
        operation,
        fingerprint,
    )
    .await
    {
        Ok(idempotency::Idempotency::Replay(response)) => return Err(response),
        Ok(guard) => guard,
        Err(response) => return Err(response),
    };
    Ok((guard, body, confirmation_hash))
}

fn parse_trigger_id(value: String) -> Result<TriggerId, TriggerServiceError> {
    TriggerId::parse(value).map_err(|_| TriggerServiceError::NotFound)
}

fn parse_revision_id(value: String) -> Result<TriggerRevisionId, TriggerServiceError> {
    TriggerRevisionId::parse(value).map_err(|_| TriggerServiceError::NotFound)
}

fn parse_public_id(value: String) -> Result<PublicTriggerId, TriggerServiceError> {
    PublicTriggerId::parse(value).map_err(|_| TriggerServiceError::NotFound)
}

fn parse_launch_id(value: String) -> Result<LaunchId, TriggerServiceError> {
    LaunchId::parse(value).map_err(|_| TriggerServiceError::NotFound)
}

#[utoipa::path(
    get,
    path = "/api/v1/triggers/public/{public_id}",
    tag = "triggers",
    params(("public_id" = String, Path)),
    responses((status = 200, description = "Side-effect-free public Trigger preview", body = TriggerPreviewView)),
    security(())
)]
async fn public_preview(
    State(state): State<TriggersState>,
    Path(public_id): Path<String>,
) -> Result<Response, TriggerProblem> {
    let preview = state.triggers.preview(&parse_public_id(public_id)?).await?;
    let mut response = Json(TriggerPreviewView::from(preview)).into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_str(&format!("public, max-age={PUBLIC_PREVIEW_CACHE_SECONDS}"))
            .expect("static cache policy is valid"),
    );
    response.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    Ok(response)
}

#[utoipa::path(
    post,
    path = "/api/v1/triggers",
    tag = "triggers",
    params(("Idempotency-Key" = String, Header)),
    request_body = CreateTriggerRequest,
    responses((status = 201, description = "Trigger draft created", body = TriggerDraftView))
)]
async fn create_trigger(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    key: idempotency::IdempotencyKey,
    body: Result<idempotency::RequestBody<CreateTriggerRequest>, BodyProblem>,
) -> Response {
    let (guard, body, _) = match command(
        key,
        Arc::clone(&state.operations),
        &caller,
        "trigger-create".into(),
        body,
    )
    .await
    {
        Ok(parts) => parts,
        Err(response) => return response,
    };
    let result: Result<Response, RequestFailure> = async {
        let body = body.map_err(RequestFailure::from)?;
        let content = body
            .content
            .resolve(
                state.connections.as_ref(),
                &caller.subject,
                &body.selected_service_id,
            )
            .await?;
        let draft = state
            .triggers
            .create_draft(caller.subject.clone(), content, now())
            .await?;
        Ok((StatusCode::CREATED, Json(TriggerDraftView::from(draft))).into_response())
    }
    .await;
    guard.finish(result).await
}

#[utoipa::path(
    get,
    path = "/api/v1/triggers",
    tag = "triggers",
    params(("limit" = Option<u32>, Query), ("cursor" = Option<String>, Query)),
    responses((status = 200, description = "Publisher Trigger drafts", body = TriggerDraftPageView))
)]
async fn list_triggers(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    params: Result<Query<PageParams>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<TriggerDraftPageView>, RequestFailure> {
    let Query(params) = params?;
    let page = state
        .triggers
        .list_drafts(
            &caller.subject,
            &TriggerPageRequest::new(params.limit, params.cursor),
        )
        .await?;
    Ok(Json(TriggerDraftPageView {
        items: page.items.into_iter().map(Into::into).collect(),
        next_cursor: page.next,
    }))
}

#[utoipa::path(
    get,
    path = "/api/v1/triggers/{trigger_id}",
    tag = "triggers",
    params(("trigger_id" = String, Path)),
    responses((status = 200, description = "Publisher Trigger draft", body = TriggerDraftView))
)]
async fn read_trigger(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(trigger_id): Path<String>,
) -> Result<Json<TriggerDraftView>, TriggerProblem> {
    Ok(Json(
        state
            .triggers
            .get_draft(&caller.subject, &parse_trigger_id(trigger_id)?)
            .await?
            .into(),
    ))
}

#[utoipa::path(
    put,
    path = "/api/v1/triggers/{trigger_id}",
    tag = "triggers",
    params(("trigger_id" = String, Path), ("Idempotency-Key" = String, Header)),
    request_body = UpdateTriggerRequest,
    responses((status = 200, description = "Trigger draft replaced", body = TriggerDraftView))
)]
async fn update_trigger(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(trigger_id): Path<String>,
    key: idempotency::IdempotencyKey,
    body: Result<idempotency::RequestBody<UpdateTriggerRequest>, BodyProblem>,
) -> Response {
    let operation = format!("trigger-update:{trigger_id}");
    let (guard, body, _) =
        match command(key, Arc::clone(&state.operations), &caller, operation, body).await {
            Ok(parts) => parts,
            Err(response) => return response,
        };
    let result: Result<Response, RequestFailure> = async {
        let trigger_id = parse_trigger_id(trigger_id)?;
        let body = body.map_err(RequestFailure::from)?;
        let content = body
            .content
            .resolve(
                state.connections.as_ref(),
                &caller.subject,
                &body.selected_service_id,
            )
            .await?;
        let draft = state
            .triggers
            .update_draft(
                &caller.subject,
                &trigger_id,
                body.expected_version,
                content,
                now(),
            )
            .await?;
        Ok(Json(TriggerDraftView::from(draft)).into_response())
    }
    .await;
    guard.finish(result).await
}

#[utoipa::path(
    post,
    path = "/api/v1/triggers/{trigger_id}/publish",
    tag = "triggers",
    params(("trigger_id" = String, Path), ("Idempotency-Key" = String, Header)),
    request_body = PublishTriggerRequest,
    responses((status = 201, description = "Immutable Trigger revision published", body = TriggerRevisionView))
)]
async fn publish_trigger(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(trigger_id): Path<String>,
    key: idempotency::IdempotencyKey,
    body: Result<idempotency::RequestBody<PublishTriggerRequest>, BodyProblem>,
) -> Response {
    let operation = format!("trigger-publish:{trigger_id}");
    let (guard, body, _) =
        match command(key, Arc::clone(&state.operations), &caller, operation, body).await {
            Ok(parts) => parts,
            Err(response) => return response,
        };
    let result: Result<Response, RequestFailure> = async {
        let body = body.map_err(RequestFailure::from)?;
        let published = state
            .triggers
            .publish(
                &caller.subject,
                &parse_trigger_id(trigger_id)?,
                body.expected_version,
                &body.selected_service_id,
                now(),
            )
            .await?;
        Ok((
            StatusCode::CREATED,
            Json(TriggerRevisionView::from(published.revision)),
        )
            .into_response())
    }
    .await;
    guard.finish(result).await
}

#[utoipa::path(
    get,
    path = "/api/v1/trigger-revisions",
    tag = "triggers",
    params(("limit" = Option<u32>, Query), ("cursor" = Option<String>, Query)),
    responses((status = 200, description = "Publisher immutable revisions", body = TriggerRevisionPageView))
)]
async fn list_revisions(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    params: Result<Query<PageParams>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<TriggerRevisionPageView>, RequestFailure> {
    let Query(params) = params?;
    let page = state
        .triggers
        .list_revisions(
            &caller.subject,
            &TriggerPageRequest::new(params.limit, params.cursor),
        )
        .await?;
    Ok(Json(TriggerRevisionPageView {
        items: page.items.into_iter().map(Into::into).collect(),
        next_cursor: page.next,
    }))
}

#[utoipa::path(
    get,
    path = "/api/v1/trigger-revisions/{revision_id}",
    tag = "triggers",
    params(("revision_id" = String, Path)),
    responses((status = 200, description = "Publisher immutable revision", body = TriggerRevisionView))
)]
async fn read_revision(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(revision_id): Path<String>,
) -> Result<Json<TriggerRevisionView>, TriggerProblem> {
    Ok(Json(
        state
            .triggers
            .get_revision(&caller.subject, &parse_revision_id(revision_id)?)
            .await?
            .into(),
    ))
}

#[utoipa::path(
    post,
    path = "/api/v1/trigger-revisions/{revision_id}/retire",
    tag = "triggers",
    params(("revision_id" = String, Path), ("Idempotency-Key" = String, Header)),
    request_body = EmptyCommand,
    responses((status = 200, description = "Revision retired", body = TriggerRevisionView))
)]
async fn retire_revision(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(revision_id): Path<String>,
    key: idempotency::IdempotencyKey,
    body: Result<idempotency::RequestBody<EmptyCommand>, BodyProblem>,
) -> Response {
    let operation = format!("trigger-retire:{revision_id}");
    let (guard, body, _) =
        match command(key, Arc::clone(&state.operations), &caller, operation, body).await {
            Ok(parts) => parts,
            Err(response) => return response,
        };
    let result: Result<Response, RequestFailure> = async {
        body.map_err(RequestFailure::from)?;
        let revision = state
            .triggers
            .retire(&caller.subject, &parse_revision_id(revision_id)?, &now())
            .await?;
        Ok(Json(TriggerRevisionView::from(revision)).into_response())
    }
    .await;
    guard.finish(result).await
}

#[utoipa::path(
    get,
    path = "/api/v1/trigger-connections",
    tag = "trigger-connections",
    responses((status = 200, description = "Reusable GitHub connections", body = TriggerConnectionsView))
)]
async fn list_connections(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
) -> Result<Json<TriggerConnectionsView>, TriggerProblem> {
    Ok(Json(TriggerConnectionsView {
        connections: state
            .connections
            .list_connections(&caller.subject)
            .await?
            .into_iter()
            .map(Into::into)
            .collect(),
    }))
}

#[utoipa::path(
    post,
    path = "/api/v1/trigger-connections",
    tag = "trigger-connections",
    params(("Idempotency-Key" = String, Header)),
    request_body = BeginConnectionRequest,
    responses((status = 200, description = "GitHub OAuth handoff", body = ConnectionAuthorizationView))
)]
async fn begin_connection(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    key: idempotency::IdempotencyKey,
    body: Result<idempotency::RequestBody<BeginConnectionRequest>, BodyProblem>,
) -> Response {
    let (guard, body, _) = match command(
        key,
        Arc::clone(&state.operations),
        &caller,
        "trigger-connection-create".into(),
        body,
    )
    .await
    {
        Ok(parts) => parts,
        Err(response) => return response,
    };
    let result: Result<Response, RequestFailure> = async {
        let body = body.map_err(RequestFailure::from)?;
        let authorization = state
            .connections
            .begin_connection(&caller.subject, &body.intent_id)
            .await?;
        Ok(Json(ConnectionAuthorizationView::from(authorization)).into_response())
    }
    .await;
    guard.finish(result).await
}

#[utoipa::path(
    post,
    path = "/api/v1/trigger-connections/{service_id}/reconnect",
    tag = "trigger-connections",
    params(("service_id" = String, Path), ("Idempotency-Key" = String, Header)),
    request_body = EmptyCommand,
    responses((status = 200, description = "GitHub OAuth reconnection handoff", body = ConnectionAuthorizationView))
)]
async fn reconnect_connection(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(service_id): Path<String>,
    key: idempotency::IdempotencyKey,
    body: Result<idempotency::RequestBody<EmptyCommand>, BodyProblem>,
) -> Response {
    let operation = format!("trigger-connection-reconnect:{service_id}");
    let (guard, body, _) =
        match command(key, Arc::clone(&state.operations), &caller, operation, body).await {
            Ok(parts) => parts,
            Err(response) => return response,
        };
    let result: Result<Response, RequestFailure> = async {
        body.map_err(RequestFailure::from)?;
        let authorization = state
            .connections
            .reconnect(&caller.subject, &service_id)
            .await?;
        Ok(Json(ConnectionAuthorizationView::from(authorization)).into_response())
    }
    .await;
    guard.finish(result).await
}

#[utoipa::path(
    get,
    path = "/api/v1/trigger-connections/{service_id}/preflight",
    tag = "trigger-connections",
    params(("service_id" = String, Path)),
    responses((status = 200, description = "Current connection preflight", body = TriggerConnectionView))
)]
async fn preflight_connection(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(service_id): Path<String>,
) -> Result<Json<TriggerConnectionView>, TriggerProblem> {
    Ok(Json(
        state
            .connections
            .preflight_connection(&caller.subject, &service_id)
            .await?
            .into(),
    ))
}

#[utoipa::path(
    post,
    path = "/api/v1/triggers/public/{public_id}/launches",
    tag = "launches",
    params(("public_id" = String, Path), ("Idempotency-Key" = String, Header)),
    request_body = EmptyCommand,
    responses((status = 201, description = "Current Launch created or resumed", body = LaunchView))
)]
async fn create_or_resume_launch(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(public_id): Path<String>,
    headers: HeaderMap,
    key: idempotency::IdempotencyKey,
    body: Result<idempotency::RequestBody<EmptyCommand>, BodyProblem>,
) -> Response {
    let operation = format!("trigger-launch-create:{public_id}");
    let (guard, body, _) =
        match command(key, Arc::clone(&state.operations), &caller, operation, body).await {
            Ok(parts) => parts,
            Err(response) => return response,
        };
    let result: Result<Response, RequestFailure> = async {
        caller
            .authorization
            .require(layer2_core::Capability::CreateSession)?;
        body.map_err(RequestFailure::from)?;
        let launch = state
            .triggers
            .create_or_resume_launch(
                caller.subject.clone(),
                &parse_public_id(public_id)?,
                now(),
                correlation_id(&headers),
            )
            .await?;
        Ok((StatusCode::CREATED, Json(LaunchView::from(launch))).into_response())
    }
    .await;
    guard.finish(result).await
}

#[utoipa::path(
    post,
    path = "/api/v1/triggers/public/{public_id}/launches/new",
    tag = "launches",
    params(("public_id" = String, Path), ("Idempotency-Key" = String, Header)),
    request_body = NewLaunchRequest,
    responses((status = 201, description = "Separate Launch created", body = LaunchView))
)]
async fn create_new_launch(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(public_id): Path<String>,
    headers: HeaderMap,
    key: idempotency::IdempotencyKey,
    body: Result<idempotency::RequestBody<NewLaunchRequest>, BodyProblem>,
) -> Response {
    let operation = format!("trigger-launch-new:{public_id}");
    let (guard, body, confirmation_hash) =
        match command(key, Arc::clone(&state.operations), &caller, operation, body).await {
            Ok(parts) => parts,
            Err(response) => return response,
        };
    let result: Result<Response, RequestFailure> = async {
        caller
            .authorization
            .require(layer2_core::Capability::CreateSession)?;
        let body = body.map_err(RequestFailure::from)?;
        if !body.acknowledge_additional_resources {
            return Err(TriggerServiceError::Validation(
                layer2_core::TriggerValidationError::field(
                    "acknowledge_additional_resources",
                    "must be accepted before creating a separate Launch",
                ),
            )
            .into());
        }
        let launch = state
            .triggers
            .create_new_launch(
                caller.subject.clone(),
                &parse_public_id(public_id)?,
                confirmation_hash,
                now(),
                correlation_id(&headers),
            )
            .await?;
        Ok((StatusCode::CREATED, Json(LaunchView::from(launch))).into_response())
    }
    .await;
    guard.finish(result).await
}

#[utoipa::path(
    get,
    path = "/api/v1/launches",
    tag = "launches",
    params(("limit" = Option<u32>, Query), ("cursor" = Option<String>, Query)),
    responses((status = 200, description = "Contributor Launches", body = LaunchPageView))
)]
async fn list_launches(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    params: Result<Query<PageParams>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<LaunchPageView>, RequestFailure> {
    let Query(params) = params?;
    let page = state
        .triggers
        .list_launches(
            &caller.subject,
            TriggerPageRequest::new(params.limit, params.cursor),
        )
        .await?;
    Ok(Json(LaunchPageView {
        items: page.items.into_iter().map(Into::into).collect(),
        next_cursor: page.next,
    }))
}

#[utoipa::path(
    get,
    path = "/api/v1/launches/{launch_id}",
    tag = "launches",
    params(("launch_id" = String, Path)),
    responses((status = 200, description = "Launch review and current state", body = LaunchView))
)]
async fn read_launch(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(launch_id): Path<String>,
) -> Result<Json<LaunchView>, TriggerProblem> {
    Ok(Json(
        state
            .triggers
            .get_launch(&caller.subject, &parse_launch_id(launch_id)?)
            .await?
            .into(),
    ))
}

#[utoipa::path(
    patch,
    path = "/api/v1/launches/{launch_id}/overrides",
    tag = "launches",
    params(("launch_id" = String, Path), ("Idempotency-Key" = String, Header)),
    request_body = UpdateOverridesRequest,
    responses((status = 200, description = "Launch overrides applied", body = LaunchView))
)]
async fn update_launch_overrides(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(launch_id): Path<String>,
    headers: HeaderMap,
    key: idempotency::IdempotencyKey,
    body: Result<idempotency::RequestBody<UpdateOverridesRequest>, BodyProblem>,
) -> Response {
    let operation = format!("trigger-launch-overrides:{launch_id}");
    let (guard, body, _) =
        match command(key, Arc::clone(&state.operations), &caller, operation, body).await {
            Ok(parts) => parts,
            Err(response) => return response,
        };
    let result: Result<Response, RequestFailure> = async {
        let body = body.map_err(RequestFailure::from)?;
        let launch = state
            .triggers
            .update_overrides(
                &caller.subject,
                &parse_launch_id(launch_id)?,
                body.expected_revision,
                body.overrides,
                now(),
                correlation_id(&headers),
            )
            .await?;
        Ok(Json(LaunchView::from(launch)).into_response())
    }
    .await;
    guard.finish(result).await
}

#[utoipa::path(
    put,
    path = "/api/v1/launches/{launch_id}/connection",
    tag = "launches",
    params(("launch_id" = String, Path), ("Idempotency-Key" = String, Header)),
    request_body = SelectConnectionRequest,
    responses((status = 200, description = "Exact connection selected", body = LaunchView))
)]
async fn select_launch_connection(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(launch_id): Path<String>,
    headers: HeaderMap,
    key: idempotency::IdempotencyKey,
    body: Result<idempotency::RequestBody<SelectConnectionRequest>, BodyProblem>,
) -> Response {
    let operation = format!("trigger-launch-connection:{launch_id}");
    let (guard, body, _) =
        match command(key, Arc::clone(&state.operations), &caller, operation, body).await {
            Ok(parts) => parts,
            Err(response) => return response,
        };
    let result: Result<Response, RequestFailure> = async {
        let body = body.map_err(RequestFailure::from)?;
        let launch = state
            .triggers
            .select_connection(
                &caller.subject,
                &parse_launch_id(launch_id)?,
                body.expected_revision,
                &body.service_id,
                now(),
                correlation_id(&headers),
            )
            .await?;
        Ok(Json(LaunchView::from(launch)).into_response())
    }
    .await;
    guard.finish(result).await
}

#[utoipa::path(
    post,
    path = "/api/v1/launches/{launch_id}/start",
    tag = "launches",
    params(("launch_id" = String, Path), ("Idempotency-Key" = String, Header)),
    request_body = ConfirmLaunchRequest,
    responses((status = 202, description = "Launch orchestration accepted", body = LaunchView))
)]
async fn start_launch(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(launch_id): Path<String>,
    headers: HeaderMap,
    key: idempotency::IdempotencyKey,
    body: Result<idempotency::RequestBody<ConfirmLaunchRequest>, BodyProblem>,
) -> Response {
    let operation = format!("trigger-launch-start:{launch_id}");
    let (guard, body, confirmation_hash) =
        match command(key, Arc::clone(&state.operations), &caller, operation, body).await {
            Ok(parts) => parts,
            Err(response) => return response,
        };
    let result: Result<Response, RequestFailure> = async {
        caller
            .authorization
            .require(layer2_core::Capability::CreateSession)?;
        let body = body.map_err(RequestFailure::from)?;
        let timestamp = now();
        let launch = state
            .triggers
            .start(
                &caller.subject,
                &parse_launch_id(launch_id)?,
                body.expected_revision,
                Confirmation {
                    action: ConfirmationAction::Start,
                    subject: caller.subject.clone(),
                    snapshot_digest: body.snapshot_digest,
                    confirmed_at: timestamp.clone(),
                    idempotency_key_hash: confirmation_hash,
                },
                timestamp,
                correlation_id(&headers),
            )
            .await?;
        Ok((StatusCode::ACCEPTED, Json(LaunchView::from(launch))).into_response())
    }
    .await;
    guard.finish(result).await
}

#[utoipa::path(
    post,
    path = "/api/v1/launches/{launch_id}/cancel",
    tag = "launches",
    params(("launch_id" = String, Path), ("Idempotency-Key" = String, Header)),
    request_body = ConfirmLaunchRequest,
    responses((status = 200, description = "Launch cancellation recorded", body = LaunchView))
)]
async fn cancel_launch(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(launch_id): Path<String>,
    headers: HeaderMap,
    key: idempotency::IdempotencyKey,
    body: Result<idempotency::RequestBody<ConfirmLaunchRequest>, BodyProblem>,
) -> Response {
    let operation = format!("trigger-launch-cancel:{launch_id}");
    let (guard, body, confirmation_hash) =
        match command(key, Arc::clone(&state.operations), &caller, operation, body).await {
            Ok(parts) => parts,
            Err(response) => return response,
        };
    let result: Result<Response, RequestFailure> = async {
        let body = body.map_err(RequestFailure::from)?;
        let timestamp = now();
        let launch = state
            .triggers
            .cancel(
                &caller.subject,
                &parse_launch_id(launch_id)?,
                body.expected_revision,
                Confirmation {
                    action: ConfirmationAction::Cancel,
                    subject: caller.subject.clone(),
                    snapshot_digest: body.snapshot_digest,
                    confirmed_at: timestamp.clone(),
                    idempotency_key_hash: confirmation_hash,
                },
                timestamp,
                correlation_id(&headers),
            )
            .await?;
        Ok(Json(LaunchView::from(launch)).into_response())
    }
    .await;
    guard.finish(result).await
}

#[utoipa::path(
    get,
    path = "/api/v1/launches/{launch_id}/events",
    tag = "launches",
    params(("launch_id" = String, Path), ("after" = Option<u64>, Query), ("limit" = Option<usize>, Query)),
    responses((status = 200, description = "Replayable Launch progress", body = LaunchProgressView))
)]
async fn launch_progress(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(launch_id): Path<String>,
    params: Result<Query<ProgressParams>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<LaunchProgressView>, RequestFailure> {
    let launch_id = parse_launch_id(launch_id)?;
    let Query(params) = params?;
    let events = state
        .triggers
        .progress(&caller.subject, &launch_id, params.after, params.limit)
        .await?;
    let next_cursor = events.last().map_or(params.after, |event| event.sequence);
    Ok(Json(LaunchProgressView {
        launch_id: launch_id.0,
        events: events.into_iter().map(json_value).collect(),
        next_cursor,
    }))
}

/// Trigger-domain failure projected without leaking provider bodies or stored
/// persistence diagnostics.
pub(super) struct TriggerProblem(pub TriggerServiceError);

impl From<TriggerServiceError> for TriggerProblem {
    fn from(value: TriggerServiceError) -> Self {
        Self(value)
    }
}

impl IntoResponse for TriggerProblem {
    fn into_response(self) -> Response {
        let (status, slug, code, detail, field): (StatusCode, &str, &str, String, Option<String>) =
            match self.0 {
                TriggerServiceError::NotFound | TriggerServiceError::Forbidden => (
                    StatusCode::NOT_FOUND,
                    "trigger-resource-not-found",
                    "trigger_resource_not_found",
                    "The requested Trigger resource does not exist.".into(),
                    None,
                ),
                TriggerServiceError::Validation(error) => (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "invalid-trigger-request",
                    "invalid_trigger_request",
                    "The Trigger request violates the declared policy.".into(),
                    error.field,
                ),
                TriggerServiceError::Authority(detail) => (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "trigger-authority-required",
                    "trigger_authority_required",
                    detail,
                    None,
                ),
                TriggerServiceError::Store(error) => match error {
                    TriggerStoreError::Unavailable(_) | TriggerStoreError::Malformed(_) => (
                        StatusCode::SERVICE_UNAVAILABLE,
                        "trigger-store-unavailable",
                        "trigger_store_unavailable",
                        "Trigger state is temporarily unavailable.".into(),
                        None,
                    ),
                    TriggerStoreError::AlreadyExists
                    | TriggerStoreError::RevisionConflict { .. }
                    | TriggerStoreError::StaleFence { .. }
                    | TriggerStoreError::LeaseHeld
                    | TriggerStoreError::Superseded => (
                        StatusCode::CONFLICT,
                        "trigger-state-conflict",
                        "trigger_state_conflict",
                        "The Trigger resource changed; read its current state before retrying."
                            .into(),
                        None,
                    ),
                },
            };
        let mut body = problem_document(status, slug, &detail, code);
        if let (Some(field), Some(object)) = (field, body.as_object_mut()) {
            object.insert("field".into(), field.into());
        }
        problem_response(status, body)
    }
}

// Keep the closed MVP vocabulary named in this interface module. These
// compile-time references make removing one from the core an intentional API
// change rather than an unnoticed schema narrowing.
#[allow(dead_code)]
fn closed_contract_vocabulary() {
    let _ = [
        BillingMode::Contributor,
        BillingMode::ZeroCost,
        BillingMode::Sponsored,
    ];
    let _ = [
        CredentialRoute::ContributorConnection,
        CredentialRoute::SponsoredGrant,
    ];
    let _ = [ExecutorPolicy::Disabled];
    let _ = [
        WorkflowChangePolicy::Refuse,
        WorkflowChangePolicy::RequireScopeAndConfirmation,
    ];
    let _ = [ScopeClass::PublicRepository, ScopeClass::Workflow];
    let _ = [
        EditableField::TaskInput,
        EditableField::BranchSuffix,
        EditableField::ResourceSize,
        EditableField::Timeout,
        EditableField::OptionalChecks,
    ];
    let _ = [
        RequiredCapability::Conversation,
        RequiredCapability::DurableConversation,
        RequiredCapability::WorkspaceCheckpoint,
        RequiredCapability::RestrictedServiceGateway,
        RequiredCapability::CredentialRefresh,
        RequiredCapability::CredentialRevocation,
    ];
    let _ = TriggerAvailability::Published;
    let _ = ResourcePolicy {
        default: layer2_core::SessionSize::new(250, 512).unwrap(),
        ceiling: layer2_core::SessionSize::new(250, 512).unwrap(),
    };
    let _ = InputFieldDefinition {
        key: String::new(),
        kind: layer2_core::InputFieldKind::Boolean,
        required: false,
        default: None,
        editable: false,
    };
}

#[cfg(test)]
#[path = "triggers_tests.rs"]
mod tests;
