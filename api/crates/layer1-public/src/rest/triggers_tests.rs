use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use layer2_core::{
    AuthorityProvenance, Authorization, BillingMode, Capability, CommercialPolicy,
    CommitAttributionPolicy, ConnectedAccount, ConnectionSnapshot, ConnectionState,
    ContributionPolicy, CredentialRoute, DelegationState, EditableField, ExecutorPolicy,
    InputFieldDefinition, InputFieldKind, Launch, LaunchAuthority, LaunchId, LaunchQuery,
    LaunchStore, OperationClaim, OperationReservation, OperationStore, OverridePolicy,
    PermissionMode, PublicProjectionPolicy, PublicTriggerId, PublishedRevision, PublisherAuthority,
    PublisherRepositoryRole, PullRequestDefaults, RepositoryAccessPolicy, RepositoryIdentity,
    RepositoryOperationClass, RepositoryVisibility, RequiredCapability, ResourcePolicy, ScopeClass,
    SessionSize, SetupPolicy, SourceMode, StoreError, SubjectId, TriggerAuthorityVerifier,
    TriggerConnection, TriggerConnectionAuthorization, TriggerConnectionManager, TriggerDraft,
    TriggerId, TriggerPage, TriggerPageRequest, TriggerRepositorySelection, TriggerRevision,
    TriggerRevisionContent, TriggerRevisionId, TriggerService, TriggerServiceError,
    TriggerSessionPolicy, TriggerStore, TriggerStoreError, TriggerTask, UpstreamRevision,
    WorkerLease, WorkflowChangePolicy, TRIGGER_SCHEMA_VERSION,
};
use serde_json::{json, Value};
use tower::ServiceExt;

use super::*;
use crate::auth::AuthenticatedSubject;

const NOW: &str = "2026-08-26T12:00:00Z";
const OWNER: &str = "contributor";
const PRIVATE_PROMPT: &str = "PRIVATE_TRIGGER_PROMPT {{topic}}";

#[tokio::test]
async fn anonymous_preview_is_side_effect_free_and_prompt_free() {
    let fixture = Fixture::new();
    let public_id = fixture.revision.public_id.0.clone();
    let app = fixture.router();

    let response = app
        .oneshot(
            Request::get(format!("/api/v1/triggers/public/{public_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(header::REFERRER_POLICY).unwrap(),
        "no-referrer"
    );
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let wire = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(wire.contains("Fix one bounded issue"));
    assert!(!wire.contains(PRIVATE_PROMPT));
    assert!(!wire.contains("backing-key"));
    assert_eq!(fixture.store.writes.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.operations.writes.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn every_mutation_requires_an_authenticated_subject_before_writing() {
    let fixture = Fixture::new();
    let launch_id = fixture.launch.id.0.clone();
    let body = json!({
        "expected_revision": 0,
        "snapshot_digest": fixture.launch.effective.digest,
    });
    let request = Request::builder()
        .method(Method::POST)
        .uri(format!("/api/v1/launches/{launch_id}/start"))
        .header(header::CONTENT_TYPE, "application/json")
        .header("idempotency-key", "unauthenticated-start")
        .body(Body::from(body.to_string()))
        .unwrap();

    let response = fixture.router().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(fixture.store.writes.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.operations.writes.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn repeated_start_replays_the_exact_response_and_advances_once() {
    let fixture = Fixture::new();
    let launch_id = fixture.launch.id.0.clone();
    let body = json!({
        "expected_revision": 0,
        "snapshot_digest": fixture.launch.effective.digest,
    });
    let first = send(
        &fixture.router(),
        Method::POST,
        &format!("/api/v1/launches/{launch_id}/start"),
        body.clone(),
        "one-start",
    )
    .await;
    let replay = send(
        &fixture.router(),
        Method::POST,
        &format!("/api/v1/launches/{launch_id}/start"),
        body,
        "one-start",
    )
    .await;

    assert_eq!(first.0, StatusCode::ACCEPTED);
    assert_eq!(replay.0, StatusCode::ACCEPTED);
    assert_eq!(first.1, replay.1);
    assert_eq!(fixture.store.writes.load(Ordering::SeqCst), 1);
    assert_eq!(
        fixture
            .store
            .launch
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .revision_number,
        1
    );
}

#[test]
fn unknown_fields_in_nested_trigger_policy_are_refused() {
    let mut body = serde_json::to_value(content(SubjectId(OWNER.into()))).unwrap();
    body["task"]["unexpected_secret_switch"] = Value::Bool(true);
    let error = serde_json::from_value::<TriggerRevisionContent>(body).unwrap_err();
    assert!(error.to_string().contains("unknown field"));
}

#[test]
fn trigger_openapi_paths_exist_and_only_preview_is_declared_anonymous() {
    let fixture = Fixture::new();
    let documented = documented_router(fixture.state());
    let paths = &documented.get_openapi().paths.paths;
    assert!(paths.contains_key("/api/v1/triggers/public/{public_id}"));
    assert!(paths.contains_key("/api/v1/launches/{launch_id}/start"));

    for path in paths.keys() {
        assert_eq!(
            crate::rest::openapi::anonymous_path(path),
            path == "/api/v1/triggers/public/{public_id}",
            "unexpected anonymous contract at {path}"
        );
    }
}

#[test]
fn launch_projection_excludes_credential_and_worker_internals() {
    let fixture = Fixture::new();
    let wire = serde_json::to_string(&LaunchView::from(fixture.launch.clone())).unwrap();
    for forbidden in [
        "backing_key_id",
        "worker_fencing_token",
        "fencing_counter",
        "subject_token_generation",
        "session_credential_generation",
        "idempotency_key_hash",
        "provider_request_id",
    ] {
        assert!(!wire.contains(forbidden), "leaked {forbidden}");
    }
}

#[tokio::test]
async fn authority_failures_keep_auth_drift_rate_limit_and_outage_distinct() {
    use layer2_core::TriggerAuthorityError as Authority;

    for (error, status, code) in [
        (
            Authority::OwnerCredentialUnavailable,
            StatusCode::UNAUTHORIZED,
            "trigger_owner_credential_unavailable",
        ),
        (
            Authority::ConnectionInactive,
            StatusCode::CONFLICT,
            "trigger_connection_inactive",
        ),
        (
            Authority::AuthorityDrift,
            StatusCode::CONFLICT,
            "trigger_authority_drift",
        ),
        (
            Authority::Transient,
            StatusCode::SERVICE_UNAVAILABLE,
            "trigger_provider_unavailable",
        ),
        (
            Authority::MalformedEvidence,
            StatusCode::BAD_GATEWAY,
            "trigger_provider_response_invalid",
        ),
        (
            Authority::UpstreamNotForkable,
            StatusCode::UNPROCESSABLE_ENTITY,
            "upstream_not_forkable",
        ),
        (
            Authority::ForkNetworkOccupied,
            StatusCode::UNPROCESSABLE_ENTITY,
            "fork_network_occupied",
        ),
    ] {
        let response = TriggerProblem(TriggerServiceError::Authority(error)).into_response();
        assert_eq!(response.status(), status);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap()["code"],
            code
        );
    }

    let response = TriggerProblem(TriggerServiceError::Authority(Authority::RateLimited {
        retry_after: Some("17".into()),
    }))
    .into_response();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(response.headers().get(header::RETRY_AFTER).unwrap(), "17");
}

async fn send(
    app: &axum::Router,
    method: Method,
    uri: &str,
    body: Value,
    idempotency_key: &str,
) -> (StatusCode, Vec<u8>) {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .header("idempotency-key", idempotency_key)
        .body(Body::from(body.to_string()))
        .unwrap();
    request
        .extensions_mut()
        .insert(AuthenticatedSubject(SubjectId(OWNER.into())));
    request
        .extensions_mut()
        .insert(Authorization::from_capabilities([
            Capability::CreateSession,
        ]));
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec();
    (status, body)
}

struct Fixture {
    revision: TriggerRevision,
    launch: Launch,
    store: Arc<FixtureStore>,
    operations: Arc<MemoryOperations>,
}

impl Fixture {
    fn new() -> Self {
        let owner = SubjectId(OWNER.into());
        let revision = revision(SubjectId("publisher".into()));
        let mut launch = Launch::new(owner, revision.clone(), NOW, "create").unwrap();
        launch
            .select_connection(connection(), NOW, "connection")
            .unwrap();
        launch.authority = Some(provenance());
        let store = Arc::new(FixtureStore {
            revision: revision.clone(),
            launch: Mutex::new(Some(launch.clone())),
            writes: AtomicUsize::new(0),
        });
        Self {
            revision,
            launch,
            store,
            operations: Arc::new(MemoryOperations::default()),
        }
    }

    fn state(&self) -> TriggersState {
        TriggersState {
            triggers: Arc::new(TriggerService::new(
                self.store.clone(),
                self.store.clone(),
                Arc::new(StaticAuthority),
            )),
            connections: Arc::new(StaticConnections),
            operations: self.operations.clone(),
        }
    }

    fn router(&self) -> axum::Router {
        documented_router(self.state()).into()
    }
}

struct FixtureStore {
    revision: TriggerRevision,
    launch: Mutex<Option<Launch>>,
    writes: AtomicUsize,
}

#[async_trait]
impl TriggerStore for FixtureStore {
    async fn insert_draft(&self, _draft: &TriggerDraft) -> Result<(), TriggerStoreError> {
        self.write_unimplemented()
    }

    async fn get_draft(&self, _id: &TriggerId) -> Result<Option<TriggerDraft>, TriggerStoreError> {
        Ok(None)
    }

    async fn list_drafts_for(
        &self,
        _owner: &SubjectId,
        _page: &TriggerPageRequest,
    ) -> Result<TriggerPage<TriggerDraft>, TriggerStoreError> {
        Ok(TriggerPage {
            items: Vec::new(),
            next: None,
        })
    }

    async fn replace_draft(
        &self,
        _expected_version: u64,
        _draft: &TriggerDraft,
    ) -> Result<TriggerDraft, TriggerStoreError> {
        self.write_unimplemented()
    }

    async fn publish_revision(
        &self,
        _expected_draft_version: u64,
        _draft: &TriggerDraft,
        _revision: &TriggerRevision,
    ) -> Result<PublishedRevision, TriggerStoreError> {
        self.write_unimplemented()
    }

    async fn get_revision(
        &self,
        id: &TriggerRevisionId,
    ) -> Result<Option<TriggerRevision>, TriggerStoreError> {
        Ok((id == &self.revision.id).then(|| self.revision.clone()))
    }

    async fn get_public_revision(
        &self,
        id: &PublicTriggerId,
    ) -> Result<Option<TriggerRevision>, TriggerStoreError> {
        Ok((id == &self.revision.public_id).then(|| self.revision.clone()))
    }

    async fn list_revisions_for(
        &self,
        _owner: &SubjectId,
        _page: &TriggerPageRequest,
    ) -> Result<TriggerPage<TriggerRevision>, TriggerStoreError> {
        Ok(TriggerPage {
            items: vec![self.revision.clone()],
            next: None,
        })
    }

    async fn retire_revision(
        &self,
        _id: &TriggerRevisionId,
        _owner: &SubjectId,
        _retired_at: &str,
    ) -> Result<Option<TriggerRevision>, TriggerStoreError> {
        self.write_unimplemented()
    }
}

#[async_trait]
impl LaunchStore for FixtureStore {
    async fn insert_or_get_current(&self, _launch: &Launch) -> Result<Launch, TriggerStoreError> {
        self.write_unimplemented()
    }

    async fn insert_new(&self, _launch: &Launch) -> Result<(), TriggerStoreError> {
        self.write_unimplemented()
    }

    async fn get_launch(&self, id: &LaunchId) -> Result<Option<Launch>, TriggerStoreError> {
        Ok(self
            .launch
            .lock()
            .unwrap()
            .as_ref()
            .filter(|launch| &launch.id == id)
            .cloned())
    }

    async fn list_launches(
        &self,
        _query: &LaunchQuery,
    ) -> Result<TriggerPage<Launch>, TriggerStoreError> {
        Ok(TriggerPage {
            items: self.launch.lock().unwrap().iter().cloned().collect(),
            next: None,
        })
    }

    async fn replace_launch(
        &self,
        expected_revision: u64,
        expected_fence: Option<u64>,
        launch: &Launch,
    ) -> Result<Launch, TriggerStoreError> {
        let mut current = self.launch.lock().unwrap();
        let stored = current
            .as_ref()
            .ok_or_else(|| TriggerStoreError::Malformed("fixture Launch disappeared".into()))?;
        if stored.revision_number != expected_revision {
            return Err(TriggerStoreError::RevisionConflict {
                expected: expected_revision,
                actual: stored.revision_number,
            });
        }
        if expected_fence.is_some_and(|fence| fence != stored.fencing_counter) {
            return Err(TriggerStoreError::StaleFence {
                expected: expected_fence.unwrap(),
                actual: stored.fencing_counter,
            });
        }
        self.writes.fetch_add(1, Ordering::SeqCst);
        *current = Some(launch.clone());
        Ok(launch.clone())
    }

    async fn acquire_lease(
        &self,
        _id: &LaunchId,
        _expected_revision: u64,
        _holder: &str,
        _acquired_at: &str,
        _expires_at: &str,
    ) -> Result<Option<Launch>, TriggerStoreError> {
        self.write_unimplemented()
    }

    async fn release_lease(
        &self,
        _id: &LaunchId,
        _expected_revision: u64,
        _lease: &WorkerLease,
    ) -> Result<Launch, TriggerStoreError> {
        self.write_unimplemented()
    }
}

impl FixtureStore {
    fn write_unimplemented<T>(&self) -> Result<T, TriggerStoreError> {
        self.writes.fetch_add(1, Ordering::SeqCst);
        Err(TriggerStoreError::Unavailable(
            "fixture write is not implemented".into(),
        ))
    }
}

#[derive(Default)]
struct MemoryOperations {
    entries: Mutex<HashMap<(String, String, String), ([u8; 32], Option<Vec<u8>>)>>,
    writes: AtomicUsize,
}

#[async_trait]
impl OperationStore for MemoryOperations {
    async fn reserve(&self, claim: &OperationClaim) -> Result<OperationReservation, StoreError> {
        let key = (
            claim.subject().0.clone(),
            claim.operation().to_string(),
            claim.key().to_string(),
        );
        let mut entries = self.entries.lock().unwrap();
        match entries.get(&key) {
            Some((fingerprint, _)) if fingerprint != claim.fingerprint() => {
                Ok(OperationReservation::FingerprintMismatch)
            }
            Some((_, Some(outcome))) => Ok(OperationReservation::Complete(outcome.clone())),
            Some((_, None)) => Ok(OperationReservation::Pending),
            None => {
                entries.insert(key, (*claim.fingerprint(), None));
                self.writes.fetch_add(1, Ordering::SeqCst);
                Ok(OperationReservation::Acquired)
            }
        }
    }

    async fn complete(&self, claim: &OperationClaim, outcome: Vec<u8>) -> Result<(), StoreError> {
        let key = (
            claim.subject().0.clone(),
            claim.operation().to_string(),
            claim.key().to_string(),
        );
        let mut entries = self.entries.lock().unwrap();
        let entry = entries
            .get_mut(&key)
            .ok_or_else(|| StoreError::Malformed("operation claim disappeared".into()))?;
        entry.1 = Some(outcome);
        self.writes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

struct StaticAuthority;

#[async_trait]
impl TriggerAuthorityVerifier for StaticAuthority {
    async fn verify_publisher(
        &self,
        subject: &SubjectId,
        content: &TriggerRevisionContent,
        selected_service_id: &str,
    ) -> Result<PublisherAuthority, TriggerServiceError> {
        let mut value = publisher_authority(subject, &content.upstream.repository);
        value.connection.service_id = selected_service_id.into();
        Ok(value)
    }

    async fn verify_launch(
        &self,
        subject: &SubjectId,
        _revision: &TriggerRevision,
        selected_service_id: &str,
        _previous: Option<&ConnectionSnapshot>,
    ) -> Result<LaunchAuthority, TriggerServiceError> {
        let mut selected = connection();
        selected.service_id = selected_service_id.into();
        let mut authority = provenance();
        authority.subject = subject.clone();
        authority.service_id = selected_service_id.into();
        Ok(LaunchAuthority {
            connection: selected,
            provenance: authority,
            source_sha: "a".repeat(40),
            workflow_paths: Vec::new(),
        })
    }
}

struct StaticConnections;

#[async_trait]
impl TriggerConnectionManager for StaticConnections {
    async fn list_connections(
        &self,
        _subject: &SubjectId,
    ) -> Result<Vec<TriggerConnection>, TriggerServiceError> {
        Ok(Vec::new())
    }

    async fn begin_connection(
        &self,
        _subject: &SubjectId,
        _intent_id: &str,
    ) -> Result<TriggerConnectionAuthorization, TriggerServiceError> {
        Err(TriggerServiceError::NotFound)
    }

    async fn reconnect(
        &self,
        _subject: &SubjectId,
        _service_id: &str,
    ) -> Result<TriggerConnectionAuthorization, TriggerServiceError> {
        Err(TriggerServiceError::NotFound)
    }

    async fn preflight_connection(
        &self,
        _subject: &SubjectId,
        _service_id: &str,
    ) -> Result<TriggerConnection, TriggerServiceError> {
        Err(TriggerServiceError::NotFound)
    }

    async fn resolve_repository(
        &self,
        _subject: &SubjectId,
        _service_id: &str,
        _repository_id: u64,
        _selected_ref: &str,
    ) -> Result<TriggerRepositorySelection, TriggerServiceError> {
        Err(TriggerServiceError::NotFound)
    }
}

fn revision(owner: SubjectId) -> TriggerRevision {
    let content = content(owner.clone());
    TriggerRevision {
        id: TriggerRevisionId::new(),
        public_id: PublicTriggerId::new(),
        digest: content.digest().unwrap(),
        publisher_authority: publisher_authority(&owner, &content.upstream.repository),
        content,
        availability: TriggerAvailability::Published,
        published_at: NOW.into(),
        retired_at: None,
    }
}

fn content(owner: SubjectId) -> TriggerRevisionContent {
    TriggerRevisionContent {
        schema_version: TRIGGER_SCHEMA_VERSION,
        trigger_id: TriggerId::new(),
        revision_number: 1,
        publisher_subject_id: owner,
        publisher_display_identity: "Chrono Maintainer".into(),
        upstream: UpstreamRevision {
            repository: upstream(),
            source_mode: SourceMode::PinnedCommit,
            observed_source_sha: Some("a".repeat(40)),
            contribution_base_ref: "main".into(),
        },
        task: TriggerTask {
            public_summary: "Fix one bounded issue".into(),
            prompt_template: PRIVATE_PROMPT.into(),
            renderer_version: layer2_core::PROMPT_RENDERER_VERSION,
            input_fields: vec![InputFieldDefinition {
                key: "topic".into(),
                kind: InputFieldKind::Text {
                    min_bytes: 1,
                    max_bytes: 100,
                },
                required: true,
                default: Some(json!("the selected issue")),
                editable: true,
            }],
        },
        session: TriggerSessionPolicy {
            required_capabilities: vec![
                RequiredCapability::Conversation,
                RequiredCapability::DurableConversation,
                RequiredCapability::WorkspaceCheckpoint,
                RequiredCapability::RestrictedServiceGateway,
                RequiredCapability::CredentialRefresh,
                RequiredCapability::CredentialRevocation,
            ],
            permission_ceiling: PermissionMode::Autonomous,
            sandbox_profile: "cma-trigger-v1".into(),
            resources: ResourcePolicy {
                default: SessionSize::new(500, 1_024).unwrap(),
                ceiling: SessionSize::new(1_000, 2_048).unwrap(),
            },
            timeout_default_seconds: 3_600,
            timeout_ceiling_seconds: 7_200,
        },
        repository_access: RepositoryAccessPolicy {
            credential_route: CredentialRoute::ContributorConnection,
            required_scope_classes: vec![ScopeClass::PublicRepository],
            allowed_operation_classes: vec![RepositoryOperationClass::RepositoryRead],
            workflow_change_policy: WorkflowChangePolicy::Refuse,
        },
        setup: SetupPolicy {
            executor_policy: ExecutorPolicy::Disabled,
            steps: Vec::new(),
        },
        contribution: ContributionPolicy {
            branch_prefix: "cma/".into(),
            commit_attribution: CommitAttributionPolicy::ContributorNoReply,
            pull_request: PullRequestDefaults {
                title_template: "CMA contribution".into(),
                body_template: "Created by CMA".into(),
            },
        },
        commercial_policy: CommercialPolicy {
            billing_mode: BillingMode::ZeroCost,
            policy_reference: "public-zero-cost-v1".into(),
        },
        override_policy: OverridePolicy {
            editable_fields: vec![EditableField::TaskInput],
            optional_checks: Vec::new(),
        },
        public_projection: PublicProjectionPolicy { version: 1 },
    }
}

fn upstream() -> RepositoryIdentity {
    RepositoryIdentity {
        repository_id: 10,
        repository_node_id: "R_upstream".into(),
        owner_id: 20,
        owner_node_id: "U_upstream_owner".into(),
        canonical_owner: "upstream-owner".into(),
        canonical_name: "project".into(),
        visibility: RepositoryVisibility::Public,
        allow_forking: true,
        default_branch: "main".into(),
        selected_ref: "main".into(),
        fork_parent_id: None,
        fork_source_id: None,
    }
}

fn connection() -> ConnectionSnapshot {
    ConnectionSnapshot {
        service_id: "service-1".into(),
        service_state_version: 2,
        backing_key_id: "backing-key".into(),
        account: ConnectedAccount {
            account_id: 21,
            account_node_id: "U_contributor".into(),
            login: OWNER.into(),
            avatar_url: None,
        },
        state: ConnectionState::Active,
        granted_scopes: vec!["public_repo".into()],
        satisfied_scope_classes: vec![ScopeClass::PublicRepository],
        consent_observed_at: NOW.into(),
        preflight_observed_at: NOW.into(),
    }
}

fn provenance() -> AuthorityProvenance {
    AuthorityProvenance {
        subject: SubjectId(OWNER.into()),
        subject_token_generation: 1,
        service_id: "service-1".into(),
        service_state_version: 2,
        backing_key_id: "backing-key".into(),
        session_credential_generation: None,
        state: DelegationState::Available,
        observed_at: NOW.into(),
    }
}

fn publisher_authority(subject: &SubjectId, repository: &RepositoryIdentity) -> PublisherAuthority {
    PublisherAuthority {
        subject: subject.clone(),
        subject_token_generation: 1,
        connection: connection(),
        repository: repository.clone(),
        role: PublisherRepositoryRole::Owner,
        verified_at: NOW.into(),
    }
}
