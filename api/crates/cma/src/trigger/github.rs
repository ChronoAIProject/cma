//! Typed NyxID and GitHub authority calls for CMA Trigger.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::SystemTime;

use async_trait::async_trait;
use layer2_core::{
    AuthorityProvenance, BranchRecord, ConnectedAccount, ConnectionRevocationLease,
    ConnectionRevocationRecord, ConnectionRevocationStore, ConnectionSnapshot, ConnectionState,
    DelegationState, ForkRecord, Launch, LaunchAuthority, PublisherAuthority,
    PublisherRepositoryRole, RepositoryIdentity, RepositoryVisibility, ScopeClass, SourceMode,
    SubjectId, TriggerAuthorityVerifier, TriggerConnection, TriggerConnectionAuthorization,
    TriggerConnectionManager, TriggerRepositorySelection, TriggerRevision, TriggerRevisionContent,
    TriggerServiceError,
};
use reqwest::{Method, StatusCode, Url};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::auth::AuthState;

mod workspace;
pub(crate) use workspace::{
    build_baseline_archive_plan, workspace_blob_metadata, WorkspaceBlobMetadata, WorkspaceContext,
    WorkspaceSourceEntry, WorkspaceSourcePlan, WorkspaceUnsupported,
};
mod publication;
#[cfg(test)]
pub(crate) use publication::{PreparedGitData, PublishedGitData};
pub(crate) use publication::{PublicationAdvance, PublicationContext};
mod pull_request;
pub(crate) use pull_request::{PullRequestAdvance, PullRequestContext};

const GITHUB_SERVICE_SLUG: &str = "api-github";
const GITHUB_PROVIDER_ID: &str = "github";
const PUBLIC_REPOSITORY_SCOPE: &str = "public_repo";
const MAX_MANAGEMENT_BODY_BYTES: usize = 2 * 1024 * 1024;
const MAX_GITHUB_BODY_BYTES: usize = 32 * 1024 * 1024;
const MAX_USER_REPOSITORY_PAGES: u32 = 100;
const USER_REPOSITORY_PAGE_SIZE: usize = 100;
const MAX_GITHUB_TREE_ENTRIES: usize = 100_000;
const CONNECTION_REVOCATION_LEASE_SECONDS: u64 = 30;
const CONNECTION_REVOCATION_JOIN_ATTEMPTS: usize = 40;

#[derive(Debug, thiserror::Error)]
pub(crate) enum GithubBrokerError {
    #[error("the CMA sign-in no longer has usable delegated authority")]
    OwnerCredentialUnavailable,
    #[error("the selected GitHub connection does not exist")]
    ConnectionNotFound,
    #[error("the public MVP requires a personal GitHub connection")]
    ConnectionNotPersonal,
    #[error("the selected GitHub connection is inactive")]
    ConnectionInactive,
    #[error("the selected GitHub connection must be reauthorized")]
    ConnectionDisconnected,
    #[error("the selected GitHub connection is missing required scope {required}")]
    InsufficientScope { required: &'static str },
    #[error("the selected GitHub authority changed since preflight")]
    AuthorityDrift,
    #[error("NyxID refused this subject or exact UserService")]
    Forbidden,
    #[error("the requested provider resource does not exist")]
    NotFound,
    #[error("the provider rate limit was reached")]
    RateLimited { retry_after: Option<String> },
    #[error("the provider is temporarily unavailable")]
    Transient { status: Option<u16> },
    #[error("the outcome of a provider write is unknown")]
    AmbiguousWrite,
    #[error("the repository does not currently allow forking")]
    ForkingDisabled,
    #[error("the connected account's fork-network slot is occupied")]
    ForkNetworkOccupied,
    #[error("the launch branch already points at a different commit")]
    BranchHeadConflict,
    #[error("the upstream contribution base changed after the Launch snapshot")]
    UpstreamBaseConflict,
    #[error("the linked pull request changed outside the retained CMA intent")]
    PullRequestChanged,
    #[error("the repository workspace is unsupported: {0}")]
    UnsupportedWorkspace(WorkspaceUnsupported),
    #[error("the provider rejected the typed request with HTTP {status} ({kind:?})")]
    Rejected {
        status: u16,
        kind: GithubRejectionKind,
        provider_code: Option<String>,
    },
    #[error("provider data is malformed: {0}")]
    Malformed(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GithubRejectionKind {
    DuplicatePullRequest,
    WorkflowScope,
    NonFastForward,
    Validation,
    Unknown,
}

impl From<GithubBrokerError> for TriggerServiceError {
    fn from(error: GithubBrokerError) -> Self {
        use layer2_core::TriggerAuthorityError as Authority;
        let error = match error {
            GithubBrokerError::OwnerCredentialUnavailable => Authority::OwnerCredentialUnavailable,
            GithubBrokerError::ConnectionNotFound => Authority::ConnectionNotFound,
            GithubBrokerError::ConnectionNotPersonal => Authority::ConnectionNotPersonal,
            GithubBrokerError::ConnectionInactive => Authority::ConnectionInactive,
            GithubBrokerError::ConnectionDisconnected => Authority::ReauthorizationRequired,
            GithubBrokerError::InsufficientScope { required } => Authority::InsufficientScope {
                required: required.to_string(),
            },
            GithubBrokerError::AuthorityDrift => Authority::AuthorityDrift,
            GithubBrokerError::Forbidden => Authority::Forbidden,
            GithubBrokerError::NotFound => Authority::ResourceNotFound,
            GithubBrokerError::RateLimited { retry_after } => {
                Authority::RateLimited { retry_after }
            }
            GithubBrokerError::Transient { .. } => Authority::Transient,
            GithubBrokerError::AmbiguousWrite => Authority::Transient,
            GithubBrokerError::ForkingDisabled => Authority::UpstreamNotForkable,
            GithubBrokerError::ForkNetworkOccupied => Authority::ForkNetworkOccupied,
            GithubBrokerError::BranchHeadConflict => Authority::ProviderRejected,
            GithubBrokerError::UpstreamBaseConflict => Authority::ProviderRejected,
            GithubBrokerError::PullRequestChanged => Authority::ProviderRejected,
            GithubBrokerError::UnsupportedWorkspace(_) => Authority::ProviderRejected,
            GithubBrokerError::Rejected { .. } => Authority::ProviderRejected,
            GithubBrokerError::Malformed(_) => Authority::MalformedEvidence,
        };
        TriggerServiceError::Authority(error)
    }
}

#[derive(Clone)]
pub(crate) struct GithubBroker {
    base_url: Url,
    http: reqwest::Client,
    authority: Arc<dyn SubjectAuthoritySource>,
    revocations: Arc<dyn ConnectionRevocationStore>,
}

#[derive(Clone)]
struct SubjectRequestAuthority {
    bearer: String,
    token_generation: u64,
}

#[async_trait]
trait SubjectAuthoritySource: Send + Sync {
    async fn current(
        &self,
        subject: &SubjectId,
    ) -> Result<Option<SubjectRequestAuthority>, GithubBrokerError>;
}

#[async_trait]
impl SubjectAuthoritySource for AuthState {
    async fn current(
        &self,
        subject: &SubjectId,
    ) -> Result<Option<SubjectRequestAuthority>, GithubBrokerError> {
        self.subject_bearer(subject)
            .await
            .map_err(|_| GithubBrokerError::Transient { status: None })
            .map(|authority| {
                authority.map(|authority| SubjectRequestAuthority {
                    bearer: authority.bearer,
                    token_generation: authority.token_generation,
                })
            })
    }
}

#[cfg(test)]
#[derive(Default)]
struct TestConnectionRevocationStore {
    records:
        std::sync::Mutex<std::collections::BTreeMap<(String, String), ConnectionRevocationRecord>>,
    fenced: std::sync::atomic::AtomicUsize,
    fail_fencing_once: std::sync::atomic::AtomicBool,
}

#[cfg(test)]
impl TestConnectionRevocationStore {
    fn key(owner: &SubjectId, service_id: &str) -> (String, String) {
        (owner.0.clone(), service_id.to_string())
    }

    fn fenced(&self) -> usize {
        self.fenced.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn fail_fencing_once(&self) {
        self.fail_fencing_once
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

#[cfg(test)]
#[async_trait]
impl ConnectionRevocationStore for TestConnectionRevocationStore {
    async fn insert_or_get_connection_revocation(
        &self,
        record: &ConnectionRevocationRecord,
    ) -> Result<ConnectionRevocationRecord, layer2_core::TriggerStoreError> {
        let mut records = self.records.lock().unwrap();
        Ok(records
            .entry(Self::key(&record.owner, &record.service_id))
            .or_insert_with(|| record.clone())
            .clone())
    }

    async fn get_connection_revocation(
        &self,
        owner: &SubjectId,
        service_id: &str,
    ) -> Result<Option<ConnectionRevocationRecord>, layer2_core::TriggerStoreError> {
        Ok(self
            .records
            .lock()
            .unwrap()
            .get(&Self::key(owner, service_id))
            .cloned())
    }

    async fn restart_connection_revocation(
        &self,
        expected_revision: u64,
        record: &ConnectionRevocationRecord,
    ) -> Result<ConnectionRevocationRecord, layer2_core::TriggerStoreError> {
        let mut records = self.records.lock().unwrap();
        let key = Self::key(&record.owner, &record.service_id);
        let Some(current) = records.get(&key) else {
            return Err(layer2_core::TriggerStoreError::RevisionConflict {
                expected: expected_revision,
                actual: 0,
            });
        };
        if current.revision != expected_revision
            || current.settled_at.is_none()
            || current
                .provider_receipt
                .as_ref()
                .is_none_or(|receipt| receipt.state_version >= record.active_snapshot.state_version)
        {
            return Err(layer2_core::TriggerStoreError::RevisionConflict {
                expected: expected_revision,
                actual: current.revision,
            });
        }
        records.insert(key, record.clone());
        Ok(record.clone())
    }

    async fn claim_connection_revocation(
        &self,
        owner: &SubjectId,
        service_id: &str,
        lease: &ConnectionRevocationLease,
    ) -> Result<Option<ConnectionRevocationRecord>, layer2_core::TriggerStoreError> {
        let mut records = self.records.lock().unwrap();
        let Some(record) = records.get_mut(&Self::key(owner, service_id)) else {
            return Ok(None);
        };
        if record.settled_at.is_some()
            || record
                .dispatch_lease
                .as_ref()
                .is_some_and(|held| held.expires_at > lease.acquired_at)
        {
            return Ok(None);
        }
        record.revision = record.revision.checked_add(1).ok_or_else(|| {
            layer2_core::TriggerStoreError::Malformed(
                "test connection revocation revision is exhausted".into(),
            )
        })?;
        record.dispatch_lease = Some(lease.clone());
        Ok(Some(record.clone()))
    }

    async fn record_connection_revocation_receipt(
        &self,
        expected_revision: u64,
        lease_holder: &str,
        record: &ConnectionRevocationRecord,
    ) -> Result<ConnectionRevocationRecord, layer2_core::TriggerStoreError> {
        self.replace_test_record(expected_revision, lease_holder, record, false)
    }

    async fn fence_launches_for_connection_revocation(
        &self,
        _owner: &SubjectId,
        _service_id: &str,
        _now: &str,
        _correlation_id: &str,
    ) -> Result<(), layer2_core::TriggerStoreError> {
        if self
            .fail_fencing_once
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            return Err(layer2_core::TriggerStoreError::Unavailable(
                "injected Launch fencing failure".into(),
            ));
        }
        self.fenced
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }

    async fn settle_connection_revocation(
        &self,
        expected_revision: u64,
        lease_holder: &str,
        record: &ConnectionRevocationRecord,
    ) -> Result<ConnectionRevocationRecord, layer2_core::TriggerStoreError> {
        self.replace_test_record(expected_revision, lease_holder, record, true)
    }

    async fn release_connection_revocation(
        &self,
        expected_revision: u64,
        lease_holder: &str,
        owner: &SubjectId,
        service_id: &str,
    ) -> Result<(), layer2_core::TriggerStoreError> {
        let mut records = self.records.lock().unwrap();
        let Some(record) = records.get_mut(&Self::key(owner, service_id)) else {
            return Err(layer2_core::TriggerStoreError::RevisionConflict {
                expected: expected_revision,
                actual: 0,
            });
        };
        if record.revision != expected_revision
            || record
                .dispatch_lease
                .as_ref()
                .is_none_or(|lease| lease.holder != lease_holder)
        {
            return Err(layer2_core::TriggerStoreError::RevisionConflict {
                expected: expected_revision,
                actual: record.revision,
            });
        }
        record.revision += 1;
        record.dispatch_lease = None;
        Ok(())
    }

    async fn list_settled_connection_revocations(
        &self,
        owner: &SubjectId,
    ) -> Result<Vec<ConnectionRevocationRecord>, layer2_core::TriggerStoreError> {
        let mut records = self
            .records
            .lock()
            .unwrap()
            .values()
            .filter(|record| &record.owner == owner && record.settled_at.is_some())
            .cloned()
            .collect::<Vec<_>>();
        records.sort_by(|left, right| left.service_id.cmp(&right.service_id));
        Ok(records)
    }
}

#[cfg(test)]
impl TestConnectionRevocationStore {
    fn replace_test_record(
        &self,
        expected_revision: u64,
        lease_holder: &str,
        record: &ConnectionRevocationRecord,
        settling: bool,
    ) -> Result<ConnectionRevocationRecord, layer2_core::TriggerStoreError> {
        let mut records = self.records.lock().unwrap();
        let key = Self::key(&record.owner, &record.service_id);
        let Some(current) = records.get(&key) else {
            return Err(layer2_core::TriggerStoreError::RevisionConflict {
                expected: expected_revision,
                actual: 0,
            });
        };
        if current.revision != expected_revision
            || current
                .dispatch_lease
                .as_ref()
                .is_none_or(|lease| lease.holder != lease_holder)
            || record.revision != expected_revision + 1
            || record.provider_receipt.is_none()
            || settling != record.settled_at.is_some()
        {
            return Err(layer2_core::TriggerStoreError::RevisionConflict {
                expected: expected_revision,
                actual: current.revision,
            });
        }
        records.insert(key, record.clone());
        Ok(record.clone())
    }
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct GithubConnectionView {
    pub service_id: String,
    pub state_version: i64,
    pub backing_key_id: Option<String>,
    pub state: ConnectionState,
    pub account: Option<ConnectedAccount>,
    pub granted_scopes: Vec<String>,
    pub consent_observed_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct GithubOAuthStart {
    pub service_id: String,
    pub authorization_url: String,
    pub attempt_nonce: String,
}

#[derive(Debug, Clone)]
pub(crate) struct ConnectionEvidence {
    pub snapshot: ConnectionSnapshot,
    pub subject_token_generation: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct RepositoryEvidence {
    pub identity: RepositoryIdentity,
    pub permissions: RepositoryPermissions,
    pub archived: bool,
    pub disabled: bool,
    pub html_url: String,
    pub subject_token_generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ForkReconciliation {
    Absent,
    Pending { repository_id: Option<u64> },
    Ready(Box<ForkRecord>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BranchReconciliation {
    Absent,
    Ready(BranchRecord),
}

#[derive(Debug, Clone)]
pub(crate) struct ForkContext {
    pub owner: SubjectId,
    pub connection: ConnectionSnapshot,
    pub upstream: RepositoryIdentity,
    pub source_sha: String,
}

impl TryFrom<&Launch> for ForkContext {
    type Error = GithubBrokerError;

    fn try_from(launch: &Launch) -> Result<Self, Self::Error> {
        Ok(Self {
            owner: launch.owner.clone(),
            connection: launch.connection.clone().ok_or_else(|| {
                GithubBrokerError::Malformed("the Launch has no selected connection".into())
            })?,
            upstream: launch.revision.content.upstream.repository.clone(),
            source_sha: launch.source_sha.clone(),
        })
    }
}

#[derive(Debug, Clone)]
pub(crate) struct BranchContext {
    pub owner: SubjectId,
    pub launch_id: String,
    pub connection: ConnectionSnapshot,
    pub fork: ForkRecord,
    pub source_sha: String,
}

impl BranchContext {
    pub(crate) fn branch_name(&self) -> String {
        format!("cma/{}", self.launch_id)
    }
}

impl TryFrom<&Launch> for BranchContext {
    type Error = GithubBrokerError;

    fn try_from(launch: &Launch) -> Result<Self, Self::Error> {
        let context = Self {
            owner: launch.owner.clone(),
            launch_id: launch.id.0.clone(),
            connection: launch.connection.clone().ok_or_else(|| {
                GithubBrokerError::Malformed("the Launch has no selected connection".into())
            })?,
            fork: launch.fork.clone().ok_or_else(|| {
                GithubBrokerError::Malformed("the Launch has no reconciled fork".into())
            })?,
            source_sha: launch.source_sha.clone(),
        };
        repository_ref_components(&context.branch_name())?;
        validate_commit_sha(&context.source_sha)?;
        Ok(context)
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
pub(crate) struct RepositoryPermissions {
    #[serde(default)]
    pub admin: bool,
    #[serde(default)]
    pub maintain: bool,
    #[serde(default)]
    pub push: bool,
    #[serde(default)]
    pub pull: bool,
}

impl GithubBroker {
    pub(crate) fn new(
        base_url: &str,
        auth: Arc<AuthState>,
        revocations: Arc<dyn ConnectionRevocationStore>,
    ) -> Result<Self, GithubBrokerError> {
        Self::with_ports(base_url, auth, revocations)
    }

    #[cfg(test)]
    fn with_authority(
        base_url: &str,
        authority: Arc<dyn SubjectAuthoritySource>,
    ) -> Result<Self, GithubBrokerError> {
        Self::with_ports(
            base_url,
            authority,
            Arc::new(TestConnectionRevocationStore::default()),
        )
    }

    fn with_ports(
        base_url: &str,
        authority: Arc<dyn SubjectAuthoritySource>,
        revocations: Arc<dyn ConnectionRevocationStore>,
    ) -> Result<Self, GithubBrokerError> {
        let base_url = Url::parse(base_url)
            .map_err(|_| GithubBrokerError::Malformed("NyxID base URL is invalid".into()))?;
        if !matches!(base_url.scheme(), "http" | "https")
            || !base_url.username().is_empty()
            || base_url.password().is_some()
            || base_url.host_str().is_none()
        {
            return Err(GithubBrokerError::Malformed(
                "NyxID base URL must be an HTTP origin without credentials".into(),
            ));
        }
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| GithubBrokerError::Transient { status: None })?;
        Ok(Self {
            base_url,
            http,
            authority,
            revocations,
        })
    }

    pub(crate) async fn list_connections(
        &self,
        subject: &SubjectId,
    ) -> Result<Vec<GithubConnectionView>, GithubBrokerError> {
        let services: Observed<UserServiceListWire> = self
            .nyxid_json(
                subject,
                Method::GET,
                &["api", "v1", "user-services"],
                &[],
                None,
                MAX_MANAGEMENT_BODY_BYTES,
            )
            .await?;
        let mut result = Vec::new();
        for service in services.value.services {
            if service.slug != GITHUB_SERVICE_SLUG {
                continue;
            }
            validate_resource_uri(&service.resource_uri)?;
            let personal = matches!(service.credential_source, CredentialSourceWire::Personal);
            let evidence = self.authorization_evidence(subject, &service.id).await;
            let (state, scopes, consent) = match evidence {
                Ok(evidence) => (
                    connection_state(&service, &evidence),
                    normalize_scopes(evidence.granted_scopes.unwrap_or_default())?,
                    evidence.last_authorized_at,
                ),
                Err(GithubBrokerError::NotFound) => {
                    (ConnectionState::Disconnected, Vec::new(), None)
                }
                Err(error) => return Err(error),
            };
            let account = if personal && state == ConnectionState::Active {
                Some(self.github_user(subject, &service.id).await?.value.into())
            } else {
                None
            };
            result.push(GithubConnectionView {
                service_id: service.id,
                state_version: service.state_version,
                backing_key_id: service.api_key_id,
                state: if personal {
                    state
                } else {
                    ConnectionState::Disconnected
                },
                account,
                granted_scopes: scopes,
                consent_observed_at: consent,
            });
        }
        result.sort_by(|left, right| left.service_id.cmp(&right.service_id));
        Ok(result)
    }

    /// Exact owner-concealing management-plane projection for revocation.
    /// This deliberately avoids GitHub `/user`: deactivating a NyxID service
    /// must remain possible while the downstream provider is unavailable.
    async fn connection_for_management_revocation(
        &self,
        subject: &SubjectId,
        service_id: &str,
    ) -> Result<Option<TriggerConnection>, GithubBrokerError> {
        validate_opaque_id(service_id, "UserService ID")?;
        let services: Observed<UserServiceListWire> = self
            .nyxid_json(
                subject,
                Method::GET,
                &["api", "v1", "user-services"],
                &[],
                None,
                MAX_MANAGEMENT_BODY_BYTES,
            )
            .await?;
        let Some(service) = services
            .value
            .services
            .into_iter()
            .find(|service| service.id == service_id && service.slug == GITHUB_SERVICE_SLUG)
        else {
            return Ok(None);
        };
        validate_resource_uri(&service.resource_uri)?;
        if service.state_version <= 0 {
            return Err(GithubBrokerError::Malformed(
                "the UserService has no usable state version".into(),
            ));
        }
        let personal = matches!(service.credential_source, CredentialSourceWire::Personal);
        let evidence = match self.authorization_evidence(subject, service_id).await {
            Ok(evidence) => Some(evidence),
            Err(GithubBrokerError::NotFound) => None,
            Err(error) => return Err(error),
        };
        let (state, granted_scopes, consent_observed_at) = match evidence {
            Some(evidence) => {
                if evidence.id != service.id
                    || evidence
                        .state_version
                        .is_some_and(|version| version != service.state_version)
                {
                    return Err(GithubBrokerError::AuthorityDrift);
                }
                (
                    connection_state(&service, &evidence),
                    normalize_scopes(evidence.granted_scopes.unwrap_or_default())?,
                    evidence.last_authorized_at,
                )
            }
            None => (ConnectionState::Disconnected, Vec::new(), None),
        };
        Ok(Some(TriggerConnection {
            service_id: service.id,
            state_version: service.state_version,
            state: if personal {
                state
            } else {
                ConnectionState::Disconnected
            },
            account: None,
            granted_scopes,
            consent_observed_at,
        }))
    }

    async fn revoke_connection_durably(
        &self,
        subject: &SubjectId,
        service_id: &str,
    ) -> Result<TriggerConnection, TriggerServiceError> {
        validate_opaque_id(service_id, "UserService ID")?;
        let mut record = self
            .revocations
            .get_connection_revocation(subject, service_id)
            .await?;
        if record.is_none() {
            let active_snapshot = self
                .connection_for_management_revocation(subject, service_id)
                .await?
                .ok_or(GithubBrokerError::ConnectionNotFound)?;
            let intent = ConnectionRevocationRecord {
                owner: subject.clone(),
                service_id: service_id.to_string(),
                revision: 1,
                active_snapshot,
                requested_at: layer2_core::rfc3339_at(SystemTime::now()),
                provider_receipt: None,
                settled_at: None,
                dispatch_lease: None,
            };
            record = Some(
                self.revocations
                    .insert_or_get_connection_revocation(&intent)
                    .await?,
            );
        }
        let mut record = record.expect("a connection revocation intent now exists");
        validate_revocation_record(subject, service_id, &record)?;
        if let Some(settled) = settled_revocation_receipt(&record)? {
            if let Some(current) = self
                .connection_for_management_revocation(subject, service_id)
                .await?
            {
                if current.state_version > settled.state_version
                    && current.state != ConnectionState::Inactive
                {
                    let expected = record.revision;
                    let restarted = ConnectionRevocationRecord {
                        owner: subject.clone(),
                        service_id: service_id.to_string(),
                        revision: expected.checked_add(1).ok_or_else(|| {
                            layer2_core::TriggerStoreError::Malformed(
                                "connection revocation revision is exhausted".into(),
                            )
                        })?,
                        active_snapshot: current,
                        requested_at: layer2_core::rfc3339_at(SystemTime::now()),
                        provider_receipt: None,
                        settled_at: None,
                        dispatch_lease: None,
                    };
                    record = match self
                        .revocations
                        .restart_connection_revocation(expected, &restarted)
                        .await
                    {
                        Ok(record) => record,
                        Err(layer2_core::TriggerStoreError::RevisionConflict { .. }) => self
                            .revocations
                            .get_connection_revocation(subject, service_id)
                            .await?
                            .ok_or(GithubBrokerError::AuthorityDrift)?,
                        Err(error) => return Err(error.into()),
                    };
                    validate_revocation_record(subject, service_id, &record)?;
                    if record.active_snapshot.state_version
                        != restarted.active_snapshot.state_version
                    {
                        return Err(GithubBrokerError::AuthorityDrift.into());
                    }
                    if let Some(receipt) = settled_revocation_receipt(&record)? {
                        return Ok(receipt);
                    }
                } else if current.state != ConnectionState::Inactive {
                    return Err(GithubBrokerError::AuthorityDrift.into());
                } else {
                    return Ok(settled);
                }
            } else {
                return Ok(settled);
            }
        }

        let acquired = SystemTime::now();
        let lease = ConnectionRevocationLease {
            holder: uuid::Uuid::new_v4().hyphenated().to_string(),
            acquired_at: layer2_core::rfc3339_at(acquired),
            expires_at: layer2_core::rfc3339_at(
                acquired + std::time::Duration::from_secs(CONNECTION_REVOCATION_LEASE_SECONDS),
            ),
        };
        let Some(claimed) = self
            .revocations
            .claim_connection_revocation(subject, service_id, &lease)
            .await?
        else {
            return self.join_connection_revocation(subject, service_id).await;
        };
        validate_revocation_record(subject, service_id, &claimed)?;
        let result = self
            .finish_connection_revocation(claimed.clone(), &lease)
            .await;
        if result.is_err() {
            if let Ok(Some(current)) = self
                .revocations
                .get_connection_revocation(subject, service_id)
                .await
            {
                let still_held = current
                    .dispatch_lease
                    .as_ref()
                    .is_some_and(|held| held.holder == lease.holder);
                if still_held {
                    if let Err(error) = self
                        .revocations
                        .release_connection_revocation(
                            current.revision,
                            &lease.holder,
                            subject,
                            service_id,
                        )
                        .await
                    {
                        tracing::warn!(
                            service_id,
                            error = %error,
                            "connection revocation writer lease will expire before retry"
                        );
                    }
                }
            }
        }
        result
    }

    async fn join_connection_revocation(
        &self,
        subject: &SubjectId,
        service_id: &str,
    ) -> Result<TriggerConnection, TriggerServiceError> {
        for _ in 0..CONNECTION_REVOCATION_JOIN_ATTEMPTS {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            let Some(record) = self
                .revocations
                .get_connection_revocation(subject, service_id)
                .await?
            else {
                return Err(GithubBrokerError::AuthorityDrift.into());
            };
            validate_revocation_record(subject, service_id, &record)?;
            if let Some(settled) = settled_revocation_receipt(&record)? {
                return Ok(settled);
            }
        }
        Err(GithubBrokerError::Transient { status: None }.into())
    }

    async fn finish_connection_revocation(
        &self,
        mut record: ConnectionRevocationRecord,
        lease: &ConnectionRevocationLease,
    ) -> Result<TriggerConnection, TriggerServiceError> {
        if record
            .dispatch_lease
            .as_ref()
            .is_none_or(|held| held != lease)
        {
            return Err(GithubBrokerError::AuthorityDrift.into());
        }
        let receipt = match record.provider_receipt.clone() {
            Some(receipt) => receipt,
            None => {
                let before = self
                    .authorization_evidence(&record.owner, &record.service_id)
                    .await?;
                validate_revocation_evidence_identity(&record, &before)?;
                let receipt = if before.is_active {
                    match self
                        .delete_user_service(&record.owner, &record.service_id)
                        .await
                    {
                        Ok(())
                        | Err(GithubBrokerError::AmbiguousWrite)
                        | Err(GithubBrokerError::NotFound) => {}
                        Err(error) => return Err(error.into()),
                    }
                    let after = self
                        .authorization_evidence(&record.owner, &record.service_id)
                        .await?;
                    inactive_connection_receipt(&record, after)?
                } else {
                    inactive_connection_receipt(&record, before)?
                };
                let expected = record.revision;
                record.revision = expected.checked_add(1).ok_or_else(|| {
                    layer2_core::TriggerStoreError::Malformed(
                        "connection revocation revision is exhausted".into(),
                    )
                })?;
                record.provider_receipt = Some(receipt.clone());
                record = self
                    .revocations
                    .record_connection_revocation_receipt(expected, &lease.holder, &record)
                    .await?;
                receipt
            }
        };

        let settled_at = layer2_core::rfc3339_at(SystemTime::now());
        let correlation_id = format!("connection-revocation:{}", lease.holder);
        self.revocations
            .fence_launches_for_connection_revocation(
                &record.owner,
                &record.service_id,
                &settled_at,
                &correlation_id,
            )
            .await?;
        let expected = record.revision;
        record.revision = expected.checked_add(1).ok_or_else(|| {
            layer2_core::TriggerStoreError::Malformed(
                "connection revocation revision is exhausted".into(),
            )
        })?;
        record.settled_at = Some(settled_at);
        record.dispatch_lease = None;
        let settled = self
            .revocations
            .settle_connection_revocation(expected, &lease.holder, &record)
            .await?;
        let stable =
            settled_revocation_receipt(&settled)?.ok_or(GithubBrokerError::AuthorityDrift)?;
        if stable != receipt {
            return Err(GithubBrokerError::AuthorityDrift.into());
        }
        Ok(stable)
    }

    /// Create a deterministic personal OAuth placeholder.
    ///
    /// `label` must carry the caller's persisted connection-intent marker so a
    /// response-loss reconciliation can find the same service before retrying.
    pub(crate) async fn create_oauth_placeholder(
        &self,
        subject: &SubjectId,
        label: &str,
    ) -> Result<String, GithubBrokerError> {
        if label.trim().is_empty() || label.len() > 200 {
            return Err(GithubBrokerError::Malformed(
                "OAuth connection label is outside NyxID's bound".into(),
            ));
        }
        let body = serde_json::json!({
            "service_slug": GITHUB_SERVICE_SLUG,
            "label": label,
        });
        let created: Observed<CreateKeyWire> = self
            .nyxid_json(
                subject,
                Method::POST,
                &["api", "v1", "keys"],
                &[],
                Some(body),
                MAX_MANAGEMENT_BODY_BYTES,
            )
            .await?;
        if created.value.slug != GITHUB_SERVICE_SLUG
            || !matches!(
                created.value.credential_source,
                CredentialSourceWire::Personal
            )
            || created.value.id.trim().is_empty()
        {
            return Err(GithubBrokerError::Malformed(
                "NyxID created a different service than requested".into(),
            ));
        }
        Ok(created.value.id)
    }

    async fn reconcile_oauth_placeholder(
        &self,
        subject: &SubjectId,
        label: &str,
    ) -> Result<String, GithubBrokerError> {
        let services: Observed<UserServiceListWire> = self
            .nyxid_json(
                subject,
                Method::GET,
                &["api", "v1", "user-services"],
                &[],
                None,
                MAX_MANAGEMENT_BODY_BYTES,
            )
            .await?;
        let mut matching = services.value.services.into_iter().filter(|service| {
            service.slug == GITHUB_SERVICE_SLUG
                && matches!(service.credential_source, CredentialSourceWire::Personal)
                && service.label.as_deref() == Some(label)
        });
        let existing = matching.next();
        if matching.next().is_some() {
            return Err(GithubBrokerError::AuthorityDrift);
        }
        if let Some(service) = existing {
            validate_resource_uri(&service.resource_uri)?;
            validate_opaque_id(&service.id, "UserService ID")?;
            return Ok(service.id);
        }
        self.create_oauth_placeholder(subject, label).await
    }

    pub(crate) async fn begin_oauth(
        &self,
        subject: &SubjectId,
        service_id: &str,
    ) -> Result<GithubOAuthStart, GithubBrokerError> {
        validate_opaque_id(service_id, "UserService ID")?;
        self.require_service(subject, service_id).await?;
        let query = [
            ("flow", "cc"),
            ("scope_override", PUBLIC_REPOSITORY_SCOPE),
            ("key_id", service_id),
        ];
        let response: Observed<OAuthStartWire> = self
            .nyxid_json(
                subject,
                Method::GET,
                &[
                    "api",
                    "v1",
                    "providers",
                    GITHUB_PROVIDER_ID,
                    "connect",
                    "oauth",
                ],
                &query,
                None,
                MAX_MANAGEMENT_BODY_BYTES,
            )
            .await?;
        let nonce = response
            .value
            .attempt_nonce
            .filter(|value| uuid::Uuid::parse_str(value).is_ok())
            .ok_or_else(|| {
                GithubBrokerError::Malformed("NyxID omitted the OAuth attempt nonce".into())
            })?;
        validate_github_authorization_url(&response.value.authorization_url, &nonce)?;
        Ok(GithubOAuthStart {
            service_id: service_id.to_string(),
            authorization_url: response.value.authorization_url,
            attempt_nonce: nonce,
        })
    }

    pub(crate) async fn preflight_connection(
        &self,
        subject: &SubjectId,
        service_id: &str,
    ) -> Result<ConnectionEvidence, GithubBrokerError> {
        let service = self.require_service(subject, service_id).await?;
        if !matches!(service.credential_source, CredentialSourceWire::Personal) {
            return Err(GithubBrokerError::ConnectionNotPersonal);
        }
        if !service.is_active {
            return Err(GithubBrokerError::ConnectionInactive);
        }
        if service.state_version <= 0 {
            return Err(GithubBrokerError::Malformed(
                "the UserService has no usable state version".into(),
            ));
        }
        let backing_key_id = service.api_key_id.clone().ok_or_else(|| {
            GithubBrokerError::Malformed("the UserService has no backing key identity".into())
        })?;
        let evidence = self.authorization_evidence(subject, service_id).await?;
        if evidence.id != service.id
            || evidence.api_key_id.as_deref() != Some(backing_key_id.as_str())
            || evidence.state_version != Some(service.state_version)
        {
            return Err(GithubBrokerError::AuthorityDrift);
        }
        if !evidence.is_active
            || evidence.status != "active"
            || evidence.connection_status.as_deref() != Some("active")
        {
            return Err(GithubBrokerError::ConnectionDisconnected);
        }
        let granted_scopes = normalize_scopes(evidence.granted_scopes.unwrap_or_default())?;
        if !granted_scopes
            .iter()
            .any(|scope| scope == PUBLIC_REPOSITORY_SCOPE)
        {
            return Err(GithubBrokerError::InsufficientScope {
                required: PUBLIC_REPOSITORY_SCOPE,
            });
        }
        let consent_observed_at = evidence.last_authorized_at.ok_or_else(|| {
            GithubBrokerError::Malformed("OAuth authorization has no completion timestamp".into())
        })?;
        let account = self.github_user(subject, service_id).await?;
        Ok(ConnectionEvidence {
            snapshot: ConnectionSnapshot {
                service_id: service.id,
                service_state_version: service.state_version,
                backing_key_id,
                account: account.value.into(),
                state: ConnectionState::Active,
                granted_scopes,
                satisfied_scope_classes: vec![ScopeClass::PublicRepository],
                consent_observed_at,
                preflight_observed_at: layer2_core::rfc3339_at(SystemTime::now()),
            },
            subject_token_generation: account.token_generation,
        })
    }

    pub(crate) async fn repository_by_id(
        &self,
        subject: &SubjectId,
        service_id: &str,
        repository_id: u64,
    ) -> Result<RepositoryEvidence, GithubBrokerError> {
        if repository_id == 0 {
            return Err(GithubBrokerError::Malformed(
                "repository ID must be non-zero".into(),
            ));
        }
        let id = repository_id.to_string();
        let response: Observed<GithubRepositoryWire> = self
            .github_json(
                subject,
                service_id,
                Method::GET,
                &["repositories", &id],
                &[],
                None,
            )
            .await?;
        if response.value.id != repository_id {
            return Err(GithubBrokerError::AuthorityDrift);
        }
        repository_evidence(response.value, response.token_generation)
    }

    pub(crate) async fn repository_by_name(
        &self,
        subject: &SubjectId,
        service_id: &str,
        owner: &str,
        name: &str,
    ) -> Result<RepositoryEvidence, GithubBrokerError> {
        validate_repository_locator(owner, "repository owner")?;
        validate_repository_locator(name, "repository name")?;
        let response: Observed<GithubRepositoryWire> = self
            .github_json(
                subject,
                service_id,
                Method::GET,
                &["repos", owner, name],
                &[],
                None,
            )
            .await?;
        repository_evidence(response.value, response.token_generation)
    }

    /// Reconcile the connected account's single compatible fork-network slot
    /// and prove the exact Launch source commit is addressable there.
    pub(crate) async fn reconcile_fork(
        &self,
        context: &ForkContext,
    ) -> Result<ForkReconciliation, GithubBrokerError> {
        let connection = &context.connection;
        connection
            .validate_for(&[ScopeClass::PublicRepository])
            .map_err(|_| GithubBrokerError::ConnectionDisconnected)?;
        let upstream = &context.upstream;
        let network_source_id = upstream.fork_source_id.unwrap_or(upstream.repository_id);
        let mut compatible = None;
        let mut occupied = false;

        for page_number in 1..=MAX_USER_REPOSITORY_PAGES {
            let page = page_number.to_string();
            let repositories: Observed<Vec<GithubRepositoryWire>> = self
                .github_json(
                    &context.owner,
                    &connection.service_id,
                    Method::GET,
                    &["user", "repos"],
                    &[
                        ("affiliation", "owner"),
                        ("per_page", "100"),
                        ("page", page.as_str()),
                    ],
                    None,
                )
                .await?;
            let count = repositories.value.len();
            for repository in repositories.value {
                if repository.owner.id != connection.account.account_id
                    || repository.source.as_ref().map(|source| source.id) != Some(network_source_id)
                {
                    continue;
                }
                if repository.parent.as_ref().map(|parent| parent.id)
                    == Some(upstream.repository_id)
                {
                    if compatible.replace(repository.id).is_some() {
                        return Err(GithubBrokerError::AuthorityDrift);
                    }
                } else {
                    occupied = true;
                }
            }
            if count < USER_REPOSITORY_PAGE_SIZE {
                break;
            }
            if page_number == MAX_USER_REPOSITORY_PAGES {
                return Err(GithubBrokerError::Malformed(
                    "the owned repository inventory exceeds its reconciliation bound".into(),
                ));
            }
        }
        if compatible.is_some() && occupied {
            return Err(GithubBrokerError::AuthorityDrift);
        }
        match compatible {
            Some(repository_id) => self.fork_readiness(context, repository_id, true).await,
            None if occupied => Err(GithubBrokerError::ForkNetworkOccupied),
            None => Ok(ForkReconciliation::Absent),
        }
    }

    /// Create a personal fork only after reconciliation proved its network
    /// slot absent. Unknown POST outcomes are never retried as writes.
    pub(crate) async fn create_fork(
        &self,
        context: &ForkContext,
    ) -> Result<ForkReconciliation, GithubBrokerError> {
        match self.reconcile_fork(context).await? {
            ForkReconciliation::Absent => {}
            existing => return Ok(existing),
        }
        let connection = &context.connection;
        let fresh = self
            .preflight_connection(&context.owner, &connection.service_id)
            .await?;
        if connection_drifted(connection, &fresh.snapshot) {
            return Err(GithubBrokerError::AuthorityDrift);
        }
        let expected = &context.upstream;
        let mut upstream = self
            .repository_by_id(
                &context.owner,
                &connection.service_id,
                expected.repository_id,
            )
            .await?;
        upstream.identity.selected_ref = expected.selected_ref.clone();
        if !upstream.identity.allow_forking {
            return Err(GithubBrokerError::ForkingDisabled);
        }
        validate_repository_rebinding(expected, &upstream)?;
        if !upstream.permissions.pull {
            return Err(GithubBrokerError::AuthorityDrift);
        }
        validate_repository_url(&upstream.html_url, &upstream.identity)?;
        let response: Observed<GithubRepositoryWire> = self
            .github_json(
                &context.owner,
                &connection.service_id,
                Method::POST,
                &[
                    "repos",
                    upstream.identity.canonical_owner.as_str(),
                    upstream.identity.canonical_name.as_str(),
                    "forks",
                ],
                &[],
                Some(serde_json::json!({})),
            )
            .await
            .map_err(|error| {
                if matches!(
                    error,
                    GithubBrokerError::Transient { .. }
                        | GithubBrokerError::Malformed(_)
                        | GithubBrokerError::AmbiguousWrite
                ) {
                    GithubBrokerError::AmbiguousWrite
                } else {
                    error
                }
            })?;
        if response.value.id == 0 {
            return Err(GithubBrokerError::AmbiguousWrite);
        }
        self.fork_readiness(context, response.value.id, false).await
    }

    async fn fork_readiness(
        &self,
        context: &ForkContext,
        repository_id: u64,
        reused: bool,
    ) -> Result<ForkReconciliation, GithubBrokerError> {
        let connection = &context.connection;
        let mut fork = match self
            .repository_by_id(&context.owner, &connection.service_id, repository_id)
            .await
        {
            Ok(fork) => fork,
            Err(GithubBrokerError::NotFound) => {
                return Ok(ForkReconciliation::Pending {
                    repository_id: Some(repository_id),
                })
            }
            Err(error) => return Err(error),
        };
        let upstream = &context.upstream;
        let network_source_id = upstream.fork_source_id.unwrap_or(upstream.repository_id);
        fork.identity.selected_ref = upstream.selected_ref.clone();
        if fork.identity.owner_id != connection.account.account_id
            || fork.identity.owner_node_id != connection.account.account_node_id
            || fork.identity.fork_parent_id != Some(upstream.repository_id)
            || fork.identity.fork_source_id != Some(network_source_id)
            || fork.identity.visibility != RepositoryVisibility::Public
        {
            return Err(GithubBrokerError::AuthorityDrift);
        }
        validate_repository_url(&fork.html_url, &fork.identity)?;
        let default_ref = fork.identity.default_branch.clone();
        match self
            .resolve_branch_sha(
                &context.owner,
                &connection.service_id,
                &fork.identity,
                &default_ref,
            )
            .await
        {
            Ok(_) => {}
            Err(GithubBrokerError::NotFound) => {
                return Ok(ForkReconciliation::Pending {
                    repository_id: Some(repository_id),
                })
            }
            Err(error) => return Err(error),
        }
        match self
            .require_commit(
                &context.owner,
                &connection.service_id,
                &fork.identity,
                &context.source_sha,
            )
            .await
        {
            Ok(()) => {}
            Err(GithubBrokerError::NotFound) => {
                return Ok(ForkReconciliation::Pending {
                    repository_id: Some(repository_id),
                })
            }
            Err(error) => return Err(error),
        }
        Ok(ForkReconciliation::Ready(Box::new(ForkRecord {
            repository: fork.identity,
            reused,
            source_revision: context.source_sha.clone(),
            canonical_url: fork.html_url,
            ready_at: layer2_core::rfc3339_at(SystemTime::now()),
        })))
    }

    /// Read the launch ref without ever changing it.
    ///
    /// The fork is first re-resolved by numeric repository identity. A stored
    /// owner/name pair is display data, not authority, and must not steer a
    /// later write after a rename or ownership change.
    pub(crate) async fn reconcile_branch(
        &self,
        context: &BranchContext,
    ) -> Result<BranchReconciliation, GithubBrokerError> {
        match self.observe_branch(context).await {
            Ok(branch) if branch.current_head == context.source_sha.to_ascii_lowercase() => {
                Ok(BranchReconciliation::Ready(branch))
            }
            Ok(_) => Err(GithubBrokerError::BranchHeadConflict),
            Err(GithubBrokerError::NotFound) => Ok(BranchReconciliation::Absent),
            Err(error) => Err(error),
        }
    }

    /// Observe the exact Launch ref while allowing a permitted session to have
    /// advanced it beyond the original source commit.
    pub(crate) async fn observe_branch(
        &self,
        context: &BranchContext,
    ) -> Result<BranchRecord, GithubBrokerError> {
        let repository = self.current_branch_fork(context).await?;
        let branch = context.branch_name();
        let head = self
            .resolve_branch_sha(
                &context.owner,
                &context.connection.service_id,
                &repository,
                &branch,
            )
            .await?;
        branch_record(context, &repository, head)
    }

    /// Create the stable launch ref after a read-only absence proof.
    ///
    /// An unknown POST result is deliberately collapsed to `AmbiguousWrite`.
    /// The worker may only call `reconcile_branch` after that; it never earns a
    /// second write merely because the first response was lost.
    pub(crate) async fn create_branch(
        &self,
        context: &BranchContext,
    ) -> Result<BranchReconciliation, GithubBrokerError> {
        match self.reconcile_branch(context).await? {
            BranchReconciliation::Absent => {}
            ready => return Ok(ready),
        }

        let repository = self.current_branch_fork(context).await?;
        self.require_commit(
            &context.owner,
            &context.connection.service_id,
            &repository,
            &context.source_sha,
        )
        .await?;

        // Last call before the provider write: ordinary token refresh is fine,
        // but service/key/account/scope drift invalidates the retained grant.
        let fresh = self
            .preflight_connection(&context.owner, &context.connection.service_id)
            .await?;
        if connection_drifted(&context.connection, &fresh.snapshot) {
            return Err(GithubBrokerError::AuthorityDrift);
        }

        // Rebind the numeric repository after every intervening authority/read
        // call so the POST cannot be steered by a retained owner/name.
        let repository = self.current_branch_fork(context).await?;
        let branch = context.branch_name();
        let response: Observed<GithubRefWire> = self
            .github_json(
                &context.owner,
                &context.connection.service_id,
                Method::POST,
                &[
                    "repos",
                    repository.canonical_owner.as_str(),
                    repository.canonical_name.as_str(),
                    "git",
                    "refs",
                ],
                &[],
                Some(serde_json::json!({
                    "ref": format!("refs/heads/{branch}"),
                    "sha": context.source_sha,
                })),
            )
            .await
            .map_err(|error| {
                if matches!(
                    error,
                    GithubBrokerError::Transient { .. }
                        | GithubBrokerError::Malformed(_)
                        | GithubBrokerError::AmbiguousWrite
                ) {
                    GithubBrokerError::AmbiguousWrite
                } else {
                    error
                }
            })?;
        if response.value.reference != format!("refs/heads/{branch}")
            || response.value.object.kind != "commit"
            || !response
                .value
                .object
                .sha
                .eq_ignore_ascii_case(&context.source_sha)
        {
            return Err(GithubBrokerError::AmbiguousWrite);
        }
        Ok(BranchReconciliation::Ready(branch_record(
            context,
            &repository,
            response.value.object.sha.to_ascii_lowercase(),
        )?))
    }

    async fn current_branch_fork(
        &self,
        context: &BranchContext,
    ) -> Result<RepositoryIdentity, GithubBrokerError> {
        let expected = &context.fork.repository;
        let mut observed = self
            .repository_by_id(
                &context.owner,
                &context.connection.service_id,
                expected.repository_id,
            )
            .await?;
        observed.identity.selected_ref = expected.selected_ref.clone();
        validate_repository_rebinding(expected, &observed)?;
        if observed.identity.owner_id != context.connection.account.account_id
            || observed.identity.owner_node_id != context.connection.account.account_node_id
            || !observed.permissions.push
        {
            return Err(GithubBrokerError::AuthorityDrift);
        }
        validate_repository_url(&observed.html_url, &observed.identity)?;
        Ok(observed.identity)
    }

    async fn require_commit(
        &self,
        subject: &SubjectId,
        service_id: &str,
        repository: &RepositoryIdentity,
        sha: &str,
    ) -> Result<(), GithubBrokerError> {
        validate_commit_sha(sha)?;
        let response: Observed<GithubCommitWire> = self
            .github_json(
                subject,
                service_id,
                Method::GET,
                &[
                    "repos",
                    repository.canonical_owner.as_str(),
                    repository.canonical_name.as_str(),
                    "git",
                    "commits",
                    sha,
                ],
                &[],
                None,
            )
            .await?;
        if !response.value.sha.eq_ignore_ascii_case(sha) {
            return Err(GithubBrokerError::AuthorityDrift);
        }
        Ok(())
    }

    async fn workflow_paths(
        &self,
        subject: &SubjectId,
        service_id: &str,
        repository: &RepositoryIdentity,
        source_sha: &str,
    ) -> Result<Vec<String>, GithubBrokerError> {
        validate_commit_sha(source_sha)?;
        let response: Observed<GithubTreeWire> = self
            .github_json(
                subject,
                service_id,
                Method::GET,
                &[
                    "repos",
                    repository.canonical_owner.as_str(),
                    repository.canonical_name.as_str(),
                    "git",
                    "trees",
                    source_sha,
                ],
                &[("recursive", "1")],
                None,
            )
            .await?;
        if response.value.truncated || response.value.tree.len() > MAX_GITHUB_TREE_ENTRIES {
            return Err(GithubBrokerError::Malformed(
                "the source tree is incomplete or exceeds its review bound".into(),
            ));
        }
        let mut paths = Vec::new();
        for entry in response.value.tree {
            if entry.kind != "blob" {
                continue;
            }
            let Some(name) = entry.path.strip_prefix(".github/workflows/") else {
                continue;
            };
            if name.is_empty()
                || entry.path.len() > 4_096
                || entry.path.bytes().any(|byte| byte.is_ascii_control())
            {
                return Err(GithubBrokerError::Malformed(
                    "a workflow path is outside the supported review shape".into(),
                ));
            }
            if name.contains('/') {
                continue;
            }
            if !name.ends_with(".yml") && !name.ends_with(".yaml") {
                continue;
            }
            paths.push(entry.path);
        }
        paths.sort();
        if paths.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(GithubBrokerError::Malformed(
                "the source tree contains duplicate workflow paths".into(),
            ));
        }
        Ok(paths)
    }

    async fn resolve_branch_sha(
        &self,
        subject: &SubjectId,
        service_id: &str,
        repository: &RepositoryIdentity,
        selected_ref: &str,
    ) -> Result<String, GithubBrokerError> {
        let components = repository_ref_components(selected_ref)?;
        let mut path = vec![
            "repos",
            repository.canonical_owner.as_str(),
            repository.canonical_name.as_str(),
            "git",
            "ref",
            "heads",
        ];
        path.extend(components);
        let response: Observed<GithubRefWire> = self
            .github_json(subject, service_id, Method::GET, &path, &[], None)
            .await?;
        if response.value.reference != format!("refs/heads/{selected_ref}")
            || response.value.object.kind != "commit"
            || response.value.object.sha.len() != 40
            || !response
                .value
                .object
                .sha
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(GithubBrokerError::AuthorityDrift);
        }
        Ok(response.value.object.sha.to_ascii_lowercase())
    }

    async fn require_service(
        &self,
        subject: &SubjectId,
        service_id: &str,
    ) -> Result<UserServiceWire, GithubBrokerError> {
        validate_opaque_id(service_id, "UserService ID")?;
        let response: Observed<UserServiceListWire> = self
            .nyxid_json(
                subject,
                Method::GET,
                &["api", "v1", "user-services"],
                &[],
                None,
                MAX_MANAGEMENT_BODY_BYTES,
            )
            .await?;
        let service = response
            .value
            .services
            .into_iter()
            .find(|service| service.id == service_id && service.slug == GITHUB_SERVICE_SLUG)
            .ok_or(GithubBrokerError::ConnectionNotFound)?;
        validate_resource_uri(&service.resource_uri)?;
        Ok(service)
    }

    async fn authorization_evidence(
        &self,
        subject: &SubjectId,
        service_id: &str,
    ) -> Result<KeyAuthorizationWire, GithubBrokerError> {
        let response: Observed<KeyAuthorizationWire> = self
            .nyxid_json(
                subject,
                Method::GET,
                &["api", "v1", "keys", service_id, "authorization"],
                &[],
                None,
                MAX_MANAGEMENT_BODY_BYTES,
            )
            .await?;
        Ok(response.value)
    }

    async fn delete_user_service(
        &self,
        subject: &SubjectId,
        service_id: &str,
    ) -> Result<(), GithubBrokerError> {
        validate_opaque_id(service_id, "UserService ID")?;
        let authority = self
            .authority
            .current(subject)
            .await?
            .ok_or(GithubBrokerError::OwnerCredentialUnavailable)?;
        let url = endpoint(
            &self.base_url,
            &["api", "v1", "user-services", service_id],
            &[],
        )?;
        let response = self
            .http
            .delete(url)
            .bearer_auth(&authority.bearer)
            .header("accept", "application/json")
            .send()
            .await
            .map_err(|_| GithubBrokerError::AmbiguousWrite)?;
        let status = response.status();
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|value| value.to_str().ok())
            .filter(|value| value.len() <= 128)
            .map(str::to_string);
        let rate_limit_exhausted = response
            .headers()
            .get("x-ratelimit-remaining")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.trim() == "0");
        if status == StatusCode::NO_CONTENT {
            return Ok(());
        }
        if status.is_success() {
            let _ = bounded_body(response, 16 * 1024).await;
            return Err(GithubBrokerError::Malformed(
                "NyxID returned an unexpected connection-revocation status".into(),
            ));
        }
        let evidence = bounded_body(response, 16 * 1024)
            .await
            .ok()
            .and_then(|bytes| parse_rejection(&bytes));
        let classified = classify_status(status, retry_after, rate_limit_exhausted, evidence);
        Err(
            if matches!(classified, GithubBrokerError::Transient { .. }) {
                GithubBrokerError::AmbiguousWrite
            } else {
                classified
            },
        )
    }

    async fn github_user(
        &self,
        subject: &SubjectId,
        service_id: &str,
    ) -> Result<Observed<GithubUserWire>, GithubBrokerError> {
        self.github_json(subject, service_id, Method::GET, &["user"], &[], None)
            .await
    }

    async fn github_json<T: DeserializeOwned>(
        &self,
        subject: &SubjectId,
        service_id: &str,
        method: Method,
        github_path: &[&str],
        query: &[(&str, &str)],
        body: Option<Value>,
    ) -> Result<Observed<T>, GithubBrokerError> {
        validate_opaque_id(service_id, "UserService ID")?;
        let mut path = vec!["api", "v1", "proxy", "s", GITHUB_SERVICE_SLUG];
        path.extend_from_slice(github_path);
        let mut owned_query: Vec<(&str, &str)> = query.to_vec();
        owned_query.push(("_nyxid_via", service_id));
        self.nyxid_json(
            subject,
            method,
            &path,
            &owned_query,
            body,
            MAX_GITHUB_BODY_BYTES,
        )
        .await
    }

    async fn nyxid_json<T: DeserializeOwned>(
        &self,
        subject: &SubjectId,
        method: Method,
        path: &[&str],
        query: &[(&str, &str)],
        body: Option<Value>,
        max_bytes: usize,
    ) -> Result<Observed<T>, GithubBrokerError> {
        let authority = self
            .authority
            .current(subject)
            .await?
            .ok_or(GithubBrokerError::OwnerCredentialUnavailable)?;
        let url = endpoint(&self.base_url, path, query)?;
        let mut request = self
            .http
            .request(method.clone(), url)
            .bearer_auth(&authority.bearer)
            .header("accept", "application/vnd.github+json")
            .header("x-github-api-version", "2022-11-28");
        if let Some(body) = body {
            request = request.json(&body);
        }
        let mutating = !matches!(method, Method::GET | Method::HEAD);
        let response = request.send().await.map_err(|_| {
            if mutating {
                GithubBrokerError::AmbiguousWrite
            } else {
                GithubBrokerError::Transient { status: None }
            }
        })?;
        let status = response.status();
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|value| value.to_str().ok())
            .filter(|value| value.len() <= 128)
            .map(str::to_string);
        let rate_limit_exhausted = response
            .headers()
            .get("x-ratelimit-remaining")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.trim() == "0");
        if !status.is_success() {
            // Drain only a small bounded body so the connection can be reused;
            // provider text is never copied into Launch state or logs.
            let evidence = bounded_body(response, 16 * 1024)
                .await
                .ok()
                .and_then(|bytes| parse_rejection(&bytes));
            let classified = classify_status(status, retry_after, rate_limit_exhausted, evidence);
            return Err(
                if mutating && matches!(classified, GithubBrokerError::Transient { .. }) {
                    GithubBrokerError::AmbiguousWrite
                } else {
                    classified
                },
            );
        }
        let bytes = bounded_body(response, max_bytes).await?;
        let value = serde_json::from_slice(&bytes).map_err(|_| {
            GithubBrokerError::Malformed("the response is not the declared JSON shape".into())
        })?;
        Ok(Observed {
            value,
            token_generation: authority.token_generation,
        })
    }
}

fn repository_evidence(
    repository: GithubRepositoryWire,
    token_generation: u64,
) -> Result<RepositoryEvidence, GithubBrokerError> {
    let visibility = match repository.visibility.as_deref() {
        Some("public") => RepositoryVisibility::Public,
        Some("private") => RepositoryVisibility::Private,
        Some("internal") => RepositoryVisibility::Internal,
        None if repository.private => RepositoryVisibility::Private,
        None => RepositoryVisibility::Public,
        Some(_) => {
            return Err(GithubBrokerError::Malformed(
                "GitHub returned an unknown repository visibility".into(),
            ))
        }
    };
    let allow_forking = repository
        .allow_forking
        .unwrap_or(visibility == RepositoryVisibility::Public);
    let identity = RepositoryIdentity {
        repository_id: repository.id,
        repository_node_id: required(repository.node_id, "repository node ID")?,
        owner_id: repository.owner.id,
        owner_node_id: required(repository.owner.node_id, "owner node ID")?,
        canonical_owner: required(repository.owner.login, "repository owner login")?,
        canonical_name: required(repository.name, "repository name")?,
        visibility,
        allow_forking,
        default_branch: required(repository.default_branch, "default branch")?,
        selected_ref: String::new(),
        fork_parent_id: repository.parent.as_ref().map(|parent| parent.id),
        fork_source_id: repository.source.as_ref().map(|source| source.id),
    };
    Ok(RepositoryEvidence {
        identity,
        permissions: repository.permissions.unwrap_or_default(),
        archived: repository.archived,
        disabled: repository.disabled,
        html_url: required(repository.html_url, "repository URL")?,
        subject_token_generation: token_generation,
    })
}

#[async_trait]
impl TriggerAuthorityVerifier for GithubBroker {
    async fn verify_publisher(
        &self,
        subject: &SubjectId,
        content: &TriggerRevisionContent,
        selected_service_id: &str,
    ) -> Result<PublisherAuthority, TriggerServiceError> {
        let connection = self
            .preflight_connection(subject, selected_service_id)
            .await?;
        let mut repository = self
            .repository_by_id(
                subject,
                selected_service_id,
                content.upstream.repository.repository_id,
            )
            .await?;
        repository.identity.selected_ref = content.upstream.repository.selected_ref.clone();
        if !repository.identity.allow_forking {
            return Err(GithubBrokerError::ForkingDisabled.into());
        }
        let observed_head = self
            .resolve_branch_sha(
                subject,
                selected_service_id,
                &repository.identity,
                &repository.identity.selected_ref,
            )
            .await?;
        if repository.identity != content.upstream.repository
            || repository.archived
            || repository.disabled
            || (content.upstream.source_mode == SourceMode::PinnedCommit
                && content.upstream.observed_source_sha.as_deref() != Some(observed_head.as_str()))
        {
            return Err(GithubBrokerError::AuthorityDrift.into());
        }
        let role = if repository.identity.owner_id == connection.snapshot.account.account_id {
            PublisherRepositoryRole::Owner
        } else if repository.permissions.admin {
            PublisherRepositoryRole::Admin
        } else if repository.permissions.maintain {
            PublisherRepositoryRole::Maintain
        } else {
            return Err(GithubBrokerError::Forbidden.into());
        };
        Ok(PublisherAuthority {
            subject: subject.clone(),
            subject_token_generation: repository.subject_token_generation,
            connection: connection.snapshot,
            repository: repository.identity,
            role,
            verified_at: layer2_core::rfc3339_at(SystemTime::now()),
        })
    }

    async fn verify_launch(
        &self,
        subject: &SubjectId,
        revision: &TriggerRevision,
        selected_service_id: &str,
        previous: Option<&ConnectionSnapshot>,
    ) -> Result<LaunchAuthority, TriggerServiceError> {
        let connection = self
            .preflight_connection(subject, selected_service_id)
            .await?;
        if previous.is_some_and(|previous| connection_drifted(previous, &connection.snapshot)) {
            return Err(GithubBrokerError::AuthorityDrift.into());
        }
        let mut repository = self
            .repository_by_id(
                subject,
                selected_service_id,
                revision.content.upstream.repository.repository_id,
            )
            .await?;
        repository.identity.selected_ref =
            revision.content.upstream.repository.selected_ref.clone();
        if !repository.identity.allow_forking {
            return Err(GithubBrokerError::ForkingDisabled.into());
        }
        let observed_head = self
            .resolve_branch_sha(
                subject,
                selected_service_id,
                &repository.identity,
                &repository.identity.selected_ref,
            )
            .await?;
        if repository.identity != revision.content.upstream.repository
            || repository.archived
            || repository.disabled
        {
            return Err(GithubBrokerError::AuthorityDrift.into());
        }
        let source_sha = match revision.content.upstream.source_mode {
            SourceMode::PinnedCommit => revision
                .content
                .upstream
                .observed_source_sha
                .clone()
                .ok_or_else(|| {
                    GithubBrokerError::Malformed("the pinned Trigger has no source commit".into())
                })?,
            SourceMode::BranchHeadAtLaunch => observed_head,
        };
        let workflow_paths = self
            .workflow_paths(
                subject,
                selected_service_id,
                &repository.identity,
                &source_sha,
            )
            .await?;
        Ok(LaunchAuthority {
            provenance: AuthorityProvenance {
                subject: subject.clone(),
                subject_token_generation: repository.subject_token_generation,
                service_id: connection.snapshot.service_id.clone(),
                service_state_version: connection.snapshot.service_state_version,
                backing_key_id: connection.snapshot.backing_key_id.clone(),
                session_credential_authority: None,
                state: DelegationState::Available,
                observed_at: layer2_core::rfc3339_at(SystemTime::now()),
            },
            connection: connection.snapshot,
            source_sha,
            workflow_paths,
        })
    }
}

#[async_trait]
impl TriggerConnectionManager for GithubBroker {
    async fn list_connections(
        &self,
        subject: &SubjectId,
    ) -> Result<Vec<TriggerConnection>, TriggerServiceError> {
        let live = GithubBroker::list_connections(self, subject)
            .await?
            .into_iter()
            .map(trigger_connection);
        let mut merged = live
            .map(|connection| (connection.service_id.clone(), connection))
            .collect::<std::collections::BTreeMap<_, _>>();
        for record in self
            .revocations
            .list_settled_connection_revocations(subject)
            .await?
        {
            validate_revocation_record(subject, &record.service_id, &record)?;
            let receipt =
                settled_revocation_receipt(&record)?.ok_or(GithubBrokerError::AuthorityDrift)?;
            match merged.get(&receipt.service_id) {
                Some(live) if live.state_version > receipt.state_version => {}
                _ => {
                    merged.insert(receipt.service_id.clone(), receipt);
                }
            }
        }
        Ok(merged.into_values().collect())
    }

    async fn connection_for_revocation(
        &self,
        subject: &SubjectId,
        service_id: &str,
    ) -> Result<Option<TriggerConnection>, TriggerServiceError> {
        validate_opaque_id(service_id, "UserService ID")?;
        if let Some(record) = self
            .revocations
            .get_connection_revocation(subject, service_id)
            .await?
        {
            validate_revocation_record(subject, service_id, &record)?;
            if let Some(receipt) = settled_revocation_receipt(&record)? {
                if let Ok(Some(current)) = self
                    .connection_for_management_revocation(subject, service_id)
                    .await
                {
                    if current.state_version > receipt.state_version
                        && current.state != ConnectionState::Inactive
                    {
                        return Ok(Some(current));
                    }
                }
                // The durable receipt is sufficient to admit operation-store
                // replay. A new backend execution still performs the live
                // version check above before treating this epoch as current.
                return Ok(Some(receipt));
            }
            return Ok(Some(record.active_snapshot));
        }
        Ok(self
            .connection_for_management_revocation(subject, service_id)
            .await?)
    }

    async fn begin_connection(
        &self,
        subject: &SubjectId,
        intent_id: &str,
    ) -> Result<TriggerConnectionAuthorization, TriggerServiceError> {
        let intent = uuid::Uuid::parse_str(intent_id)
            .map_err(|_| GithubBrokerError::Malformed("connection intent is not valid".into()))?;
        let label = format!("cma-trigger:{}", intent.hyphenated());
        let service_id = self.reconcile_oauth_placeholder(subject, &label).await?;
        Ok(self.begin_oauth(subject, &service_id).await?.into())
    }

    async fn reconnect(
        &self,
        subject: &SubjectId,
        service_id: &str,
    ) -> Result<TriggerConnectionAuthorization, TriggerServiceError> {
        Ok(self.begin_oauth(subject, service_id).await?.into())
    }

    async fn revoke_connection(
        &self,
        subject: &SubjectId,
        service_id: &str,
    ) -> Result<TriggerConnection, TriggerServiceError> {
        self.revoke_connection_durably(subject, service_id).await
    }

    async fn preflight_connection(
        &self,
        subject: &SubjectId,
        service_id: &str,
    ) -> Result<TriggerConnection, TriggerServiceError> {
        let evidence = GithubBroker::preflight_connection(self, subject, service_id).await?;
        Ok(TriggerConnection {
            service_id: evidence.snapshot.service_id,
            state_version: evidence.snapshot.service_state_version,
            state: evidence.snapshot.state,
            account: Some(evidence.snapshot.account),
            granted_scopes: evidence.snapshot.granted_scopes,
            consent_observed_at: Some(evidence.snapshot.consent_observed_at),
        })
    }

    async fn resolve_repository(
        &self,
        subject: &SubjectId,
        service_id: &str,
        repository_id: u64,
        selected_ref: &str,
    ) -> Result<TriggerRepositorySelection, TriggerServiceError> {
        GithubBroker::preflight_connection(self, subject, service_id).await?;
        let mut repository = self
            .repository_by_id(subject, service_id, repository_id)
            .await?;
        repository.identity.selected_ref = selected_ref.to_string();
        repository.identity.validate_public_upstream()?;
        let source_sha = self
            .resolve_branch_sha(subject, service_id, &repository.identity, selected_ref)
            .await?;
        Ok(TriggerRepositorySelection {
            repository: repository.identity,
            source_sha,
        })
    }

    async fn resolve_repository_locator(
        &self,
        subject: &SubjectId,
        service_id: &str,
        owner: &str,
        name: &str,
    ) -> Result<TriggerRepositorySelection, TriggerServiceError> {
        GithubBroker::preflight_connection(self, subject, service_id).await?;
        let mut repository = self
            .repository_by_name(subject, service_id, owner, name)
            .await?;
        let selected_ref = repository.identity.default_branch.clone();
        repository.identity.selected_ref = selected_ref.clone();
        repository.identity.validate_public_upstream()?;
        let source_sha = self
            .resolve_branch_sha(subject, service_id, &repository.identity, &selected_ref)
            .await?;
        Ok(TriggerRepositorySelection {
            repository: repository.identity,
            source_sha,
        })
    }
}

impl From<GithubOAuthStart> for TriggerConnectionAuthorization {
    fn from(value: GithubOAuthStart) -> Self {
        Self {
            service_id: value.service_id,
            authorization_url: value.authorization_url,
            attempt_nonce: value.attempt_nonce,
        }
    }
}

#[derive(Debug)]
struct Observed<T> {
    value: T,
    token_generation: u64,
}

#[derive(Debug, Deserialize)]
struct UserServiceListWire {
    services: Vec<UserServiceWire>,
}

#[derive(Debug, Deserialize)]
struct UserServiceWire {
    id: String,
    slug: String,
    #[serde(default)]
    label: Option<String>,
    resource_uri: String,
    api_key_id: Option<String>,
    is_active: bool,
    state_version: i64,
    credential_source: CredentialSourceWire,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum CredentialSourceWire {
    Personal,
    Org {
        #[allow(dead_code)]
        allowed: bool,
    },
}

#[derive(Debug, Deserialize)]
struct KeyAuthorizationWire {
    id: String,
    api_key_id: Option<String>,
    is_active: bool,
    status: String,
    connection_status: Option<String>,
    granted_scopes: Option<Vec<String>>,
    last_authorized_at: Option<String>,
    state_version: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct CreateKeyWire {
    id: String,
    slug: String,
    credential_source: CredentialSourceWire,
}

#[derive(Debug, Deserialize)]
struct OAuthStartWire {
    authorization_url: String,
    attempt_nonce: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GithubUserWire {
    id: u64,
    node_id: String,
    login: String,
    avatar_url: Option<String>,
}

impl From<GithubUserWire> for ConnectedAccount {
    fn from(value: GithubUserWire) -> Self {
        Self {
            account_id: value.id,
            account_node_id: value.node_id,
            login: value.login,
            avatar_url: value.avatar_url,
        }
    }
}

#[derive(Debug, Deserialize)]
struct GithubOwnerWire {
    id: u64,
    node_id: String,
    login: String,
}

#[derive(Debug, Deserialize)]
struct GithubRepositoryRefWire {
    id: u64,
}

#[derive(Debug, Deserialize)]
struct GithubRepositoryWire {
    id: u64,
    node_id: String,
    name: String,
    owner: GithubOwnerWire,
    #[serde(default)]
    private: bool,
    visibility: Option<String>,
    default_branch: String,
    allow_forking: Option<bool>,
    #[serde(default)]
    archived: bool,
    #[serde(default)]
    disabled: bool,
    html_url: String,
    permissions: Option<RepositoryPermissions>,
    parent: Option<GithubRepositoryRefWire>,
    source: Option<GithubRepositoryRefWire>,
}

#[derive(Debug, Deserialize)]
struct GithubRefWire {
    #[serde(rename = "ref")]
    reference: String,
    object: GithubRefObjectWire,
}

#[derive(Debug, Deserialize)]
struct GithubRefObjectWire {
    sha: String,
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Debug, Deserialize)]
struct GithubCommitWire {
    sha: String,
}

#[derive(Debug, Deserialize)]
struct GithubTreeWire {
    #[serde(default)]
    truncated: bool,
    tree: Vec<GithubTreeEntryWire>,
}

#[derive(Debug, Deserialize)]
struct GithubTreeEntryWire {
    path: String,
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    sha: Option<String>,
    #[serde(default)]
    size: Option<u64>,
}

fn connection_state(service: &UserServiceWire, evidence: &KeyAuthorizationWire) -> ConnectionState {
    if !service.is_active || !evidence.is_active {
        return ConnectionState::Inactive;
    }
    match (
        evidence.status.as_str(),
        evidence.connection_status.as_deref(),
    ) {
        ("active", Some("active")) => ConnectionState::Active,
        ("expired", _) | (_, Some("expired")) => ConnectionState::Expired,
        ("revoked", _) => ConnectionState::Revoked,
        _ => ConnectionState::Disconnected,
    }
}

fn trigger_connection(connection: GithubConnectionView) -> TriggerConnection {
    TriggerConnection {
        service_id: connection.service_id,
        state_version: connection.state_version,
        state: connection.state,
        account: connection.account,
        granted_scopes: connection.granted_scopes,
        consent_observed_at: connection.consent_observed_at,
    }
}

fn validate_revocation_record(
    subject: &SubjectId,
    service_id: &str,
    record: &ConnectionRevocationRecord,
) -> Result<(), TriggerServiceError> {
    let valid_receipt = record.provider_receipt.as_ref().is_none_or(|receipt| {
        receipt.service_id == service_id
            && receipt.state == ConnectionState::Inactive
            && receipt.state_version > record.active_snapshot.state_version
    });
    if &record.owner != subject
        || record.service_id != service_id
        || record.revision == 0
        || record.active_snapshot.service_id != service_id
        || record.active_snapshot.state_version <= 0
        || !valid_receipt
        || (record.settled_at.is_some()
            && (record.provider_receipt.is_none() || record.dispatch_lease.is_some()))
    {
        return Err(GithubBrokerError::AuthorityDrift.into());
    }
    Ok(())
}

fn settled_revocation_receipt(
    record: &ConnectionRevocationRecord,
) -> Result<Option<TriggerConnection>, TriggerServiceError> {
    match (&record.settled_at, &record.provider_receipt) {
        (Some(_), Some(receipt)) if record.dispatch_lease.is_none() => Ok(Some(receipt.clone())),
        (None, _) => Ok(None),
        _ => Err(GithubBrokerError::AuthorityDrift.into()),
    }
}

fn validate_revocation_evidence_identity(
    record: &ConnectionRevocationRecord,
    evidence: &KeyAuthorizationWire,
) -> Result<(), GithubBrokerError> {
    if evidence.id != record.service_id || evidence.state_version.is_none_or(|version| version <= 0)
    {
        return Err(GithubBrokerError::AuthorityDrift);
    }
    Ok(())
}

fn inactive_connection_receipt(
    record: &ConnectionRevocationRecord,
    evidence: KeyAuthorizationWire,
) -> Result<TriggerConnection, GithubBrokerError> {
    validate_revocation_evidence_identity(record, &evidence)?;
    if evidence.is_active {
        return Err(GithubBrokerError::Transient { status: None });
    }
    let state_version = evidence
        .state_version
        .filter(|version| *version > record.active_snapshot.state_version)
        .ok_or(GithubBrokerError::AuthorityDrift)?;
    Ok(TriggerConnection {
        service_id: record.service_id.clone(),
        state_version,
        state: ConnectionState::Inactive,
        account: record.active_snapshot.account.clone(),
        granted_scopes: normalize_scopes(evidence.granted_scopes.unwrap_or_default())?,
        consent_observed_at: evidence
            .last_authorized_at
            .or_else(|| record.active_snapshot.consent_observed_at.clone()),
    })
}

fn repository_ref_components(value: &str) -> Result<Vec<&str>, GithubBrokerError> {
    if value.is_empty()
        || value.len() > 255
        || value.starts_with('/')
        || value.ends_with('/')
        || value.contains("..")
        || value.contains("@{")
        || value.bytes().any(|byte| {
            byte.is_ascii_control()
                || matches!(byte, b' ' | b'~' | b'^' | b':' | b'?' | b'*' | b'[' | b'\\')
        })
    {
        return Err(GithubBrokerError::Malformed(
            "the selected repository ref is not canonical".into(),
        ));
    }
    let components: Vec<_> = value.split('/').collect();
    if components
        .iter()
        .any(|component| component.is_empty() || matches!(*component, "." | ".."))
    {
        return Err(GithubBrokerError::Malformed(
            "the selected repository ref is not canonical".into(),
        ));
    }
    Ok(components)
}

fn validate_repository_locator(value: &str, field: &str) -> Result<(), GithubBrokerError> {
    if value.is_empty()
        || value.len() > 100
        || matches!(value, "." | "..")
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(GithubBrokerError::Malformed(format!(
            "the {field} is not canonical"
        )));
    }
    Ok(())
}

fn validate_commit_sha(value: &str) -> Result<(), GithubBrokerError> {
    if value.len() != 40 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(GithubBrokerError::Malformed(
            "the source commit is invalid".into(),
        ));
    }
    Ok(())
}

fn validate_repository_url(
    value: &str,
    repository: &RepositoryIdentity,
) -> Result<(), GithubBrokerError> {
    let url = Url::parse(value)
        .map_err(|_| GithubBrokerError::Malformed("the repository URL is invalid".into()))?;
    let expected_path = format!(
        "/{}/{}",
        repository.canonical_owner, repository.canonical_name
    );
    if url.scheme() != "https"
        || url.host_str() != Some("github.com")
        || url.port_or_known_default() != Some(443)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path().trim_end_matches(".git") != expected_path
    {
        return Err(GithubBrokerError::Malformed(
            "the repository URL does not match its numeric identity".into(),
        ));
    }
    Ok(())
}

/// Validate the immutable half of a numeric repository binding.
///
/// Canonical owner/name, the default branch, and the selected ref are mutable
/// locators rather than authority. Callers must use the producer-returned
/// values from `observed` for the immediately following request.
pub(super) fn validate_repository_rebinding(
    expected: &RepositoryIdentity,
    observed: &RepositoryEvidence,
) -> Result<(), GithubBrokerError> {
    let actual = &observed.identity;
    if actual.repository_id != expected.repository_id
        || actual.repository_node_id != expected.repository_node_id
        || actual.owner_id != expected.owner_id
        || actual.owner_node_id != expected.owner_node_id
        || actual.visibility != expected.visibility
        || actual.fork_parent_id != expected.fork_parent_id
        || actual.fork_source_id != expected.fork_source_id
        || observed.archived
        || observed.disabled
    {
        return Err(GithubBrokerError::AuthorityDrift);
    }
    Ok(())
}

fn branch_record(
    context: &BranchContext,
    repository: &RepositoryIdentity,
    current_head: String,
) -> Result<BranchRecord, GithubBrokerError> {
    let branch = context.branch_name();
    let mut url = Url::parse("https://github.com/")
        .map_err(|_| GithubBrokerError::Malformed("the GitHub URL is invalid".into()))?;
    {
        let mut segments = url.path_segments_mut().map_err(|_| {
            GithubBrokerError::Malformed("the fork URL cannot carry a branch path".into())
        })?;
        segments.pop_if_empty();
        segments.push(&repository.canonical_owner);
        segments.push(&repository.canonical_name);
        segments.push("tree");
        for component in repository_ref_components(&branch)? {
            segments.push(component);
        }
    }
    Ok(BranchRecord {
        name: branch,
        expected_head: context.source_sha.to_ascii_lowercase(),
        current_head,
        canonical_url: url.into(),
        created_at: layer2_core::rfc3339_at(SystemTime::now()),
    })
}

fn normalize_scopes(scopes: Vec<String>) -> Result<Vec<String>, GithubBrokerError> {
    let mut normalized = BTreeSet::new();
    for scope in scopes {
        let scope = scope.trim();
        if scope.is_empty()
            || scope.len() > 128
            || !scope.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'_' | b'-' | b'.')
            })
        {
            return Err(GithubBrokerError::Malformed(
                "NyxID returned an invalid OAuth scope".into(),
            ));
        }
        normalized.insert(scope.to_string());
    }
    Ok(normalized.into_iter().collect())
}

fn normalized_set(scopes: &[String]) -> BTreeSet<&str> {
    scopes.iter().map(String::as_str).collect()
}

fn connection_drifted(previous: &ConnectionSnapshot, current: &ConnectionSnapshot) -> bool {
    previous.service_id != current.service_id
        || previous.service_state_version != current.service_state_version
        || previous.backing_key_id != current.backing_key_id
        || previous.account.account_id != current.account.account_id
        || previous.account.account_node_id != current.account.account_node_id
        || normalized_set(&previous.granted_scopes) != normalized_set(&current.granted_scopes)
}

fn required(value: String, field: &str) -> Result<String, GithubBrokerError> {
    if value.trim().is_empty() || value.len() > 1_024 {
        Err(GithubBrokerError::Malformed(format!(
            "GitHub omitted {field}"
        )))
    } else {
        Ok(value)
    }
}

fn validate_opaque_id(value: &str, field: &str) -> Result<(), GithubBrokerError> {
    if value.is_empty()
        || value.len() > 256
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(GithubBrokerError::Malformed(format!(
            "{field} is not usable"
        )));
    }
    Ok(())
}

fn validate_resource_uri(value: &str) -> Result<(), GithubBrokerError> {
    let url = Url::parse(value).map_err(|_| {
        GithubBrokerError::Malformed("the UserService resource URI is invalid".into())
    })?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/api/v1/proxy/s/api-github"
    {
        return Err(GithubBrokerError::Malformed(
            "the UserService resource URI is not api-github".into(),
        ));
    }
    Ok(())
}

fn endpoint(base: &Url, path: &[&str], query: &[(&str, &str)]) -> Result<Url, GithubBrokerError> {
    let mut url = base.clone();
    url.set_query(None);
    url.set_fragment(None);
    {
        let mut segments = url.path_segments_mut().map_err(|_| {
            GithubBrokerError::Malformed("NyxID base URL cannot carry paths".into())
        })?;
        segments.clear();
        for segment in path {
            if segment.is_empty()
                || matches!(*segment, "." | "..")
                || segment.contains('/')
                || segment.contains('\\')
            {
                return Err(GithubBrokerError::Malformed(
                    "a provider route segment is not canonical".into(),
                ));
            }
            segments.push(segment);
        }
    }
    if !query.is_empty() {
        let mut pairs = url.query_pairs_mut();
        for (key, value) in query {
            pairs.append_pair(key, value);
        }
    }
    Ok(url)
}

async fn bounded_body(
    mut response: reqwest::Response,
    max_bytes: usize,
) -> Result<Vec<u8>, GithubBrokerError> {
    if response
        .content_length()
        .is_some_and(|length| length > max_bytes as u64)
    {
        return Err(GithubBrokerError::Malformed(
            "the provider response exceeds its bound".into(),
        ));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| GithubBrokerError::Transient { status: None })?
    {
        if bytes.len().saturating_add(chunk.len()) > max_bytes {
            return Err(GithubBrokerError::Malformed(
                "the provider response exceeds its bound".into(),
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn classify_status(
    status: StatusCode,
    retry_after: Option<String>,
    rate_limit_exhausted: bool,
    evidence: Option<GithubRejectionEvidence>,
) -> GithubBrokerError {
    if matches!(
        status,
        StatusCode::FORBIDDEN | StatusCode::TOO_MANY_REQUESTS
    ) && (status == StatusCode::TOO_MANY_REQUESTS
        || retry_after.is_some()
        || rate_limit_exhausted
        || evidence
            .as_ref()
            .is_some_and(|evidence| evidence.rate_limited))
    {
        return GithubBrokerError::RateLimited { retry_after };
    }
    if status == StatusCode::FORBIDDEN
        && evidence
            .as_ref()
            .is_some_and(|evidence| evidence.kind == GithubRejectionKind::WorkflowScope)
    {
        return GithubBrokerError::Rejected {
            status: status.as_u16(),
            kind: GithubRejectionKind::WorkflowScope,
            provider_code: evidence.and_then(|evidence| evidence.provider_code),
        };
    }
    match status {
        StatusCode::UNAUTHORIZED => GithubBrokerError::OwnerCredentialUnavailable,
        StatusCode::FORBIDDEN => GithubBrokerError::Forbidden,
        StatusCode::NOT_FOUND => GithubBrokerError::NotFound,
        status if status.is_server_error() => GithubBrokerError::Transient {
            status: Some(status.as_u16()),
        },
        status => GithubBrokerError::Rejected {
            status: status.as_u16(),
            kind: evidence
                .as_ref()
                .map_or(GithubRejectionKind::Unknown, |evidence| evidence.kind),
            provider_code: evidence.and_then(|evidence| evidence.provider_code),
        },
    }
}

#[derive(Debug)]
struct GithubRejectionEvidence {
    kind: GithubRejectionKind,
    provider_code: Option<String>,
    rate_limited: bool,
}

#[derive(Debug, Deserialize)]
struct GithubErrorWire {
    #[serde(default)]
    message: String,
    #[serde(default)]
    errors: Vec<GithubErrorItemWire>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum GithubErrorItemWire {
    Detail {
        #[serde(default)]
        resource: String,
        #[serde(default)]
        field: String,
        #[serde(default)]
        code: String,
        #[serde(default)]
        message: String,
    },
    Text(String),
}

fn parse_rejection(bytes: &[u8]) -> Option<GithubRejectionEvidence> {
    let wire: GithubErrorWire = serde_json::from_slice(bytes).ok()?;
    let message = wire.message.to_ascii_lowercase();
    let rate_limited = message.contains("rate limit")
        || message.contains("secondary rate")
        || message.contains("abuse detection");
    let workflow_detail = wire.errors.iter().any(|error| match error {
        GithubErrorItemWire::Detail {
            resource,
            field,
            code,
            ..
        } => [resource, field, code]
            .iter()
            .any(|value| value.to_ascii_lowercase().contains("workflow")),
        GithubErrorItemWire::Text(value) => value.to_ascii_lowercase().contains("workflow"),
    });
    let non_fast_forward_detail = wire.errors.iter().any(|error| match error {
        GithubErrorItemWire::Detail { code, .. } => {
            matches!(code.as_str(), "not_fast_forward" | "non_fast_forward")
        }
        GithubErrorItemWire::Text(value) => value.to_ascii_lowercase().contains("fast forward"),
    });
    let duplicate_pull_request_detail = wire.errors.iter().any(|error| match error {
        GithubErrorItemWire::Detail {
            resource,
            code,
            message,
            ..
        } => {
            resource.eq_ignore_ascii_case("PullRequest")
                && (matches!(code.as_str(), "already_exists" | "custom")
                    && message.to_ascii_lowercase().contains("pull request")
                    && message.to_ascii_lowercase().contains("already exists"))
        }
        GithubErrorItemWire::Text(value) => {
            let value = value.to_ascii_lowercase();
            value.contains("pull request") && value.contains("already exists")
        }
    });
    let kind = if duplicate_pull_request_detail
        || (message.contains("pull request") && message.contains("already exists"))
    {
        GithubRejectionKind::DuplicatePullRequest
    } else if workflow_detail
        || (message.contains("workflow")
            && (message.contains("scope")
                || message.contains("oauth")
                || message.contains("token")))
    {
        GithubRejectionKind::WorkflowScope
    } else if non_fast_forward_detail
        || message.contains("fast forward")
        || message.contains("reference update failed")
    {
        GithubRejectionKind::NonFastForward
    } else {
        GithubRejectionKind::Validation
    };
    let provider_code = wire.errors.iter().find_map(|error| match error {
        GithubErrorItemWire::Detail { code, .. }
            if !code.is_empty()
                && code.len() <= 64
                && code.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.')
                }) =>
        {
            Some(code.clone())
        }
        _ => None,
    });
    Some(GithubRejectionEvidence {
        kind,
        provider_code,
        rate_limited,
    })
}

fn validate_github_authorization_url(value: &str, nonce: &str) -> Result<(), GithubBrokerError> {
    let url = Url::parse(value)
        .map_err(|_| GithubBrokerError::Malformed("OAuth URL is invalid".into()))?;
    if url.scheme() != "https"
        || url.host_str() != Some("github.com")
        || url.port_or_known_default() != Some(443)
        || url.path() != "/login/oauth/authorize"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url
            .query_pairs()
            .find(|(key, _)| key == "state")
            .map(|(_, value)| value.into_owned())
            != Some(format!("1cc_{nonce}"))
    {
        return Err(GithubBrokerError::Malformed(
            "NyxID returned an unexpected GitHub authorization URL".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use axum::body::to_bytes;
    use axum::extract::{Request, State};
    use axum::http::header::AUTHORIZATION;
    use axum::response::IntoResponse;
    use axum::routing::any;
    use axum::{Json, Router};
    use layer2_core::{
        CommitIdentity, Confirmation, ConfirmationAction, ContributionId, ContributionRecord,
        DiffArtifactId, DiffChangeKind, LaunchId, PublicationState,
    };
    use serde_json::{json, Value};
    use tokio::sync::Mutex;

    use super::*;
    use sha2::{Digest as _, Sha256};

    const NONCE: &str = "123e4567-e89b-12d3-a456-426614174000";

    #[derive(Clone)]
    struct TestAuthority {
        value: Option<SubjectRequestAuthority>,
    }

    #[async_trait]
    impl SubjectAuthoritySource for TestAuthority {
        async fn current(
            &self,
            _subject: &SubjectId,
        ) -> Result<Option<SubjectRequestAuthority>, GithubBrokerError> {
            Ok(self.value.clone())
        }
    }

    #[derive(Debug)]
    struct RecordedRequest {
        method: String,
        uri: String,
        authorization: Option<String>,
        body: Option<Value>,
    }

    #[derive(Debug, Clone, Copy)]
    enum PublicationPatch {
        Confirmed,
        AmbiguousApplied,
        NonFastForward,
        WorkflowScope,
    }

    #[derive(Clone)]
    struct PublicationFixture {
        head: Arc<Mutex<String>>,
        base_tree_id: String,
        tree_id: String,
        commit_id: String,
        binary_blob_id: String,
        text_blob_id: String,
        mode_blob_id: String,
        symlink_blob_id: String,
        patch: PublicationPatch,
    }

    impl PublicationFixture {
        fn new(patch: PublicationPatch) -> Self {
            Self {
                head: Arc::new(Mutex::new("a".repeat(40))),
                base_tree_id: "b".repeat(40),
                tree_id: "c".repeat(40),
                commit_id: "d".repeat(40),
                binary_blob_id: crate::trigger::workspace_diff::git_blob_id(&[0, 255, 1]),
                text_blob_id: crate::trigger::workspace_diff::git_blob_id(b"text change\n"),
                mode_blob_id: crate::trigger::workspace_diff::git_blob_id(b"#!/bin/sh\n"),
                symlink_blob_id: crate::trigger::workspace_diff::git_blob_id(b"README.md"),
                patch,
            }
        }
    }

    #[derive(Clone)]
    struct RevocationFixture {
        active: Arc<AtomicBool>,
        state_version: Arc<AtomicI64>,
        delete_calls: Arc<AtomicUsize>,
        lose_delete_response_once: Arc<AtomicBool>,
        delete_returns_not_found_once: Arc<AtomicBool>,
        management_available: Arc<AtomicBool>,
        github_user_available: bool,
    }

    impl Default for RevocationFixture {
        fn default() -> Self {
            Self {
                active: Arc::new(AtomicBool::new(true)),
                state_version: Arc::new(AtomicI64::new(4)),
                delete_calls: Arc::new(AtomicUsize::new(0)),
                lose_delete_response_once: Arc::new(AtomicBool::new(false)),
                delete_returns_not_found_once: Arc::new(AtomicBool::new(false)),
                management_available: Arc::new(AtomicBool::new(true)),
                github_user_available: true,
            }
        }
    }

    impl RevocationFixture {
        fn reactivate(&self) {
            self.state_version.fetch_add(1, Ordering::SeqCst);
            self.active.store(true, Ordering::SeqCst);
        }
    }

    #[derive(Clone)]
    struct Fixture {
        requests: Arc<Mutex<Vec<RecordedRequest>>>,
        service_state_version: i64,
        authorization_state_version: i64,
        authority_preflight_reads: Arc<AtomicUsize>,
        authority_drift_after: Option<usize>,
        scopes: Vec<String>,
        resource_uri: String,
        label: Option<String>,
        fork_parent_id: Option<u64>,
        allow_forking: bool,
        tree_truncated: bool,
        workspace_tree: bool,
        workspace_lfs: bool,
        workspace_oversize_blob: bool,
        ambiguous_fork_post: bool,
        fork_appears_after_post: bool,
        post_seen: Arc<AtomicBool>,
        branch_head: Option<String>,
        ambiguous_branch_post: bool,
        branch_appears_after_post: bool,
        branch_post_seen: Arc<AtomicBool>,
        publication: Option<PublicationFixture>,
        upstream_name: String,
        upstream_owner_id: u64,
        upstream_pull: bool,
        fork_repository_lookups: Arc<AtomicUsize>,
        rebound_fork_name: Option<String>,
        rebound_fork_owner_id: Option<u64>,
        revocation: Option<RevocationFixture>,
    }

    impl Default for Fixture {
        fn default() -> Self {
            Self {
                requests: Arc::new(Mutex::new(Vec::new())),
                service_state_version: 4,
                authorization_state_version: 4,
                authority_preflight_reads: Arc::new(AtomicUsize::new(0)),
                authority_drift_after: None,
                scopes: vec![PUBLIC_REPOSITORY_SCOPE.to_string()],
                resource_uri: "https://nyxid.test/api/v1/proxy/s/api-github".to_string(),
                label: None,
                fork_parent_id: Some(10),
                allow_forking: true,
                tree_truncated: false,
                workspace_tree: false,
                workspace_lfs: false,
                workspace_oversize_blob: false,
                ambiguous_fork_post: false,
                fork_appears_after_post: false,
                post_seen: Arc::new(AtomicBool::new(false)),
                branch_head: None,
                ambiguous_branch_post: false,
                branch_appears_after_post: false,
                branch_post_seen: Arc::new(AtomicBool::new(false)),
                publication: None,
                upstream_name: "upstream".into(),
                upstream_owner_id: 11,
                upstream_pull: true,
                fork_repository_lookups: Arc::new(AtomicUsize::new(0)),
                rebound_fork_name: None,
                rebound_fork_owner_id: None,
                revocation: None,
            }
        }
    }

    impl Fixture {
        fn authority_has_drifted(&self, read_index: usize) -> bool {
            self.authority_drift_after
                .is_some_and(|threshold| read_index >= threshold)
        }

        fn observe_service_state_version(&self) -> i64 {
            let read_index = self
                .authority_preflight_reads
                .fetch_add(1, Ordering::SeqCst);
            self.service_state_version + i64::from(self.authority_has_drifted(read_index))
        }

        fn current_authorization_state_version(&self) -> i64 {
            let read_index = self
                .authority_preflight_reads
                .load(Ordering::SeqCst)
                .saturating_sub(1);
            self.authorization_state_version + i64::from(self.authority_has_drifted(read_index))
        }

        fn fork_is_rebound(&self) -> bool {
            self.fork_repository_lookups.load(Ordering::SeqCst) > 1
        }

        fn current_fork_name(&self) -> &str {
            if self.fork_is_rebound() {
                self.rebound_fork_name.as_deref().unwrap_or("upstream")
            } else {
                "upstream"
            }
        }

        fn observe_fork_binding(&self) -> (&str, u64) {
            let prior = self.fork_repository_lookups.fetch_add(1, Ordering::SeqCst);
            if prior > 0 {
                (
                    self.rebound_fork_name.as_deref().unwrap_or("upstream"),
                    self.rebound_fork_owner_id.unwrap_or(21),
                )
            } else {
                ("upstream", 21)
            }
        }
    }

    async fn answer(State(fixture): State<Fixture>, request: Request) -> impl IntoResponse {
        let (parts, body) = request.into_parts();
        let body = to_bytes(body, 1024 * 1024)
            .await
            .expect("fixture request body is bounded");
        let parsed_body = (!body.is_empty())
            .then(|| serde_json::from_slice(&body).expect("fixture receives JSON"));
        let method = parts.method.as_str().to_string();
        let uri = parts.uri.to_string();
        fixture.requests.lock().await.push(RecordedRequest {
            method: method.clone(),
            uri,
            authorization: parts
                .headers
                .get(AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .map(str::to_string),
            body: parsed_body,
        });

        let fork_name = fixture.current_fork_name().to_string();
        let upstream_owner = if fixture.upstream_owner_id == 11 {
            "owner"
        } else {
            "transferred-owner"
        };
        let upstream_owner_node = if fixture.upstream_owner_id == 11 {
            "U_kgDOAQ"
        } else {
            "U_transferred"
        };
        let fork_main_ref_path =
            format!("/api/v1/proxy/s/api-github/repos/contributor/{fork_name}/git/ref/heads/main");
        let fork_launch_ref_path = format!(
            "/api/v1/proxy/s/api-github/repos/contributor/{fork_name}/git/ref/heads/cma/lnch_test"
        );
        let fork_git_path = format!("/api/v1/proxy/s/api-github/repos/contributor/{fork_name}/git");
        let fork_base_commit_path =
            format!("{fork_git_path}/commits/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        let fork_result_commit_path =
            format!("{fork_git_path}/commits/dddddddddddddddddddddddddddddddddddddddd");
        let fork_result_tree_path =
            format!("{fork_git_path}/trees/cccccccccccccccccccccccccccccccccccccccc");
        let fork_blob_write_path = format!("{fork_git_path}/blobs");
        let fork_tree_write_path = format!("{fork_git_path}/trees");
        let fork_commit_write_path = format!("{fork_git_path}/commits");
        let fork_ref_patch_path = format!("{fork_git_path}/refs/heads/cma/lnch_test");
        let fork_ref_create_path = format!("{fork_git_path}/refs");
        let upstream_fork_create_path = format!(
            "/api/v1/proxy/s/api-github/repos/{upstream_owner}/{}/forks",
            fixture.upstream_name
        );

        let response = match (method.as_str(), parts.uri.path()) {
            ("GET", "/api/v1/user-services") => {
                if let Some(revocation) = fixture.revocation.as_ref() {
                    if !revocation.management_available.load(Ordering::SeqCst) {
                        return (
                            StatusCode::SERVICE_UNAVAILABLE,
                            Json(json!({ "error": "management unavailable" })),
                        );
                    }
                    let services = if revocation.active.load(Ordering::SeqCst) {
                        vec![json!({
                            "id": "service-1",
                            "slug": GITHUB_SERVICE_SLUG,
                            "label": fixture.label,
                            "resource_uri": fixture.resource_uri,
                            "api_key_id": "key-1",
                            "is_active": true,
                            "state_version": revocation.state_version.load(Ordering::SeqCst),
                            "credential_source": { "type": "personal" }
                        })]
                    } else {
                        Vec::new()
                    };
                    return (StatusCode::OK, Json(json!({ "services": services })));
                }
                let state_version = fixture.observe_service_state_version();
                json!({
                "services": [{
                    "id": "service-1",
                    "slug": GITHUB_SERVICE_SLUG,
                    "label": fixture.label,
                    "resource_uri": fixture.resource_uri,
                    "api_key_id": "key-1",
                    "is_active": true,
                    "state_version": state_version,
                    "credential_source": { "type": "personal" }
                }]
                })
            }
            ("GET", "/api/v1/keys/service-1/authorization") => {
                if let Some(revocation) = fixture.revocation.as_ref() {
                    if !revocation.management_available.load(Ordering::SeqCst) {
                        return (
                            StatusCode::SERVICE_UNAVAILABLE,
                            Json(json!({ "error": "management unavailable" })),
                        );
                    }
                    let active = revocation.active.load(Ordering::SeqCst);
                    return (
                        StatusCode::OK,
                        Json(json!({
                            "id": "service-1",
                            "api_key_id": "key-1",
                            "is_active": active,
                            "status": if active { "active" } else { "inactive" },
                            "connection_status": if active { "active" } else { "inactive" },
                            "granted_scopes": fixture.scopes,
                            "last_authorized_at": "2026-08-26T01:02:03Z",
                            "state_version": revocation.state_version.load(Ordering::SeqCst)
                        })),
                    );
                }
                let state_version = fixture.current_authorization_state_version();
                json!({
                "id": "service-1",
                "api_key_id": "key-1",
                "is_active": true,
                "status": "active",
                "connection_status": "active",
                "granted_scopes": fixture.scopes,
                "last_authorized_at": "2026-08-26T01:02:03Z",
                "state_version": state_version
                })
            }
            ("DELETE", "/api/v1/user-services/service-1") => {
                let Some(revocation) = fixture.revocation.as_ref() else {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({ "error": "revocation fixture is not configured" })),
                    );
                };
                if !revocation.management_available.load(Ordering::SeqCst) {
                    return (
                        StatusCode::SERVICE_UNAVAILABLE,
                        Json(json!({ "error": "management unavailable" })),
                    );
                }
                revocation.delete_calls.fetch_add(1, Ordering::SeqCst);
                revocation.active.store(false, Ordering::SeqCst);
                revocation.state_version.fetch_add(1, Ordering::SeqCst);
                if revocation
                    .lose_delete_response_once
                    .swap(false, Ordering::SeqCst)
                {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({ "error": "response lost" })),
                    );
                }
                if revocation
                    .delete_returns_not_found_once
                    .swap(false, Ordering::SeqCst)
                {
                    return (
                        StatusCode::NOT_FOUND,
                        Json(json!({ "error": "concurrent deactivate won" })),
                    );
                }
                return (StatusCode::NO_CONTENT, Json(Value::Null));
            }
            ("GET", "/api/v1/proxy/s/api-github/user") => {
                if fixture
                    .revocation
                    .as_ref()
                    .is_some_and(|revocation| !revocation.github_user_available)
                {
                    return (
                        StatusCode::SERVICE_UNAVAILABLE,
                        Json(json!({ "error": "github unavailable" })),
                    );
                }
                json!({
                    "id": 21,
                    "node_id": "U_kgDOBQ",
                    "login": "contributor",
                    "avatar_url": "https://avatars.githubusercontent.com/u/21"
                })
            }
            ("GET", "/api/v1/proxy/s/api-github/repositories/10") => json!({
                "id": 10,
                "node_id": "R_kgDOAA",
                "name": fixture.upstream_name,
                "owner": {
                    "id": fixture.upstream_owner_id,
                    "node_id": upstream_owner_node,
                    "login": upstream_owner
                },
                "private": false,
                "visibility": "public",
                "default_branch": "main",
                "allow_forking": fixture.allow_forking,
                "archived": false,
                "disabled": false,
                "html_url": format!("https://github.com/{upstream_owner}/{}", fixture.upstream_name),
                "permissions": { "pull": fixture.upstream_pull }
            }),
            ("GET", "/api/v1/proxy/s/api-github/repos/owner/upstream/git/ref/heads/main") => {
                json!({
                    "ref": "refs/heads/main",
                    "object": {
                        "sha": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                        "type": "commit"
                    }
                })
            }
            (
                "GET",
                "/api/v1/proxy/s/api-github/repos/owner/upstream/git/trees/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            ) => json!({
                "sha": "tree-sha",
                "truncated": fixture.tree_truncated,
                "tree": [
                    { "path": "README.md", "type": "blob", "sha": "readme-sha" },
                    { "path": ".github/workflows/release.yml", "type": "blob", "sha": "workflow-2" },
                    { "path": ".github/workflows/checks.yaml", "type": "blob", "sha": "workflow-1" },
                    { "path": ".github/workflows/nested/ignored.yml", "type": "blob", "sha": "nested" },
                    { "path": ".github/workflows", "type": "tree", "sha": "directory" }
                ]
            }),
            (
                "GET",
                "/api/v1/proxy/s/api-github/repos/contributor/upstream/git/trees/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            ) if fixture.workspace_tree => {
                let tree = if fixture.workspace_oversize_blob {
                    vec![json!({
                        "path": "oversize.bin",
                        "type": "blob",
                        "mode": "100644",
                        "sha": "1111111111111111111111111111111111111111",
                        "size": layer2_core::MAX_PROVIDER_BLOB_BYTES + 1
                    })]
                } else if fixture.workspace_lfs {
                    vec![json!({
                        "path": ".gitattributes",
                        "type": "blob",
                        "mode": "100644",
                        "sha": "2d1978c67ec2de07e3a068b676596fb7902535e8",
                        "size": 26
                    })]
                } else {
                    vec![
                        json!({
                            "path": "README.md",
                            "type": "blob",
                            "mode": "100755",
                            "sha": "ce013625030ba8dba906f756967f9e9ca394464a",
                            "size": 6
                        }),
                        json!({
                            "path": "bin",
                            "type": "tree",
                            "mode": "040000",
                            "sha": "1111111111111111111111111111111111111111"
                        }),
                        json!({
                            "path": "bin/data.bin",
                            "type": "blob",
                            "mode": "100644",
                            "sha": "046f393ee9d8a6a0754b8ddadce727b4ce35f272",
                            "size": 3
                        }),
                        json!({
                            "path": "readme-link",
                            "type": "blob",
                            "mode": "120000",
                            "sha": "42061c01a1c70097d1e4579f29a5adf40abdec95",
                            "size": 9
                        }),
                    ]
                };
                json!({ "truncated": fixture.tree_truncated, "tree": tree })
            }
            (
                "GET",
                "/api/v1/proxy/s/api-github/repos/contributor/upstream/git/blobs/ce013625030ba8dba906f756967f9e9ca394464a",
            ) => json!({
                "sha": "ce013625030ba8dba906f756967f9e9ca394464a",
                "size": 6,
                "encoding": "base64",
                "content": "aGVsbG8K\n"
            }),
            (
                "GET",
                "/api/v1/proxy/s/api-github/repos/contributor/upstream/git/blobs/046f393ee9d8a6a0754b8ddadce727b4ce35f272",
            ) => json!({
                "sha": "046f393ee9d8a6a0754b8ddadce727b4ce35f272",
                "size": 3,
                "encoding": "base64",
                "content": "AP8B"
            }),
            (
                "GET",
                "/api/v1/proxy/s/api-github/repos/contributor/upstream/git/blobs/42061c01a1c70097d1e4579f29a5adf40abdec95",
            ) => json!({
                "sha": "42061c01a1c70097d1e4579f29a5adf40abdec95",
                "size": 9,
                "encoding": "base64",
                "content": "UkVBRE1FLm1k"
            }),
            (
                "GET",
                "/api/v1/proxy/s/api-github/repos/contributor/upstream/git/blobs/2d1978c67ec2de07e3a068b676596fb7902535e8",
            ) => json!({
                "sha": "2d1978c67ec2de07e3a068b676596fb7902535e8",
                "size": 26,
                "encoding": "base64",
                "content": "Ki5iaW4gZmlsdGVyPWxmcyBkaWZmPWxmcwo="
            }),
            ("GET", "/api/v1/proxy/s/api-github/user/repos") => fixture
                .fork_parent_id
                .or_else(|| {
                    (fixture.fork_appears_after_post
                        && fixture.post_seen.load(Ordering::SeqCst))
                    .then_some(10)
                })
                .map(|parent| json!([fork_repository(parent)]))
                .unwrap_or_else(|| json!([])),
            ("GET", "/api/v1/proxy/s/api-github/repositories/30") => {
                let (name, owner_id) = fixture.observe_fork_binding();
                json!(fork_repository_binding(10, name, owner_id))
            }
            ("GET", actual) if actual == fork_main_ref_path => {
                json!({
                    "ref": "refs/heads/main",
                    "object": {
                        "sha": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                        "type": "commit"
                    }
                })
            }
            ("GET", actual) if actual == fork_base_commit_path => {
                json!({
                    "sha": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    "tree": {
                        "sha": fixture.publication.as_ref().map_or_else(
                            || "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string(),
                            |publication| publication.base_tree_id.clone()
                        )
                    },
                    "parents": []
                })
            }
            (
                "GET",
                actual,
            ) if actual == fork_launch_ref_path => {
                let head = if let Some(publication) = fixture.publication.as_ref() {
                    Some(publication.head.lock().await.clone())
                } else {
                    fixture.branch_head.clone().or_else(|| {
                        (fixture.branch_appears_after_post
                            && fixture.branch_post_seen.load(Ordering::SeqCst))
                        .then(|| "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string())
                    })
                };
                let Some(head) = head else {
                    return (StatusCode::NOT_FOUND, Json(json!({ "message": "not found" })));
                };
                json!({
                    "ref": "refs/heads/cma/lnch_test",
                    "object": { "sha": head, "type": "commit" }
                })
            }
            ("POST", actual)
                if actual == fork_blob_write_path && fixture.publication.is_some() => {
                let publication = fixture.publication.as_ref().unwrap();
                json!({ "sha": publication.binary_blob_id })
            }
            ("POST", actual)
                if actual == fork_tree_write_path && fixture.publication.is_some() => {
                let publication = fixture.publication.as_ref().unwrap();
                json!({ "sha": publication.tree_id })
            }
            ("POST", actual)
                if actual == fork_commit_write_path && fixture.publication.is_some() => {
                let publication = fixture.publication.as_ref().unwrap();
                json!({
                    "sha": publication.commit_id,
                    "tree": { "sha": publication.tree_id },
                    "parents": [{ "sha": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" }]
                })
            }
            ("GET", actual)
                if actual == fork_result_commit_path && fixture.publication.is_some() => {
                let publication = fixture.publication.as_ref().unwrap();
                json!({
                    "sha": publication.commit_id,
                    "tree": { "sha": publication.tree_id },
                    "parents": [{ "sha": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" }]
                })
            }
            ("PATCH", actual)
                if actual == fork_ref_patch_path && fixture.publication.is_some() => {
                let publication = fixture.publication.as_ref().unwrap();
                match publication.patch {
                    PublicationPatch::Confirmed => {
                        *publication.head.lock().await = publication.commit_id.clone();
                    }
                    PublicationPatch::AmbiguousApplied => {
                        *publication.head.lock().await = publication.commit_id.clone();
                        return (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            Json(json!({ "message": "response lost after update" })),
                        );
                    }
                    PublicationPatch::NonFastForward => {
                        return (
                            StatusCode::UNPROCESSABLE_ENTITY,
                            Json(json!({
                                "message": "Reference update failed: not a fast forward",
                                "errors": [{ "resource": "Reference", "field": "sha", "code": "not_fast_forward" }]
                            })),
                        );
                    }
                    PublicationPatch::WorkflowScope => {
                        return (
                            StatusCode::FORBIDDEN,
                            Json(json!({
                                "message": "refusing to allow an OAuth App to create or update workflow without workflow scope",
                                "errors": [{ "resource": "Workflow", "field": "workflow", "code": "missing_workflow_scope" }]
                            })),
                        );
                    }
                }
                json!({
                    "ref": "refs/heads/cma/lnch_test",
                    "object": { "sha": publication.commit_id, "type": "commit" }
                })
            }
            ("GET", actual)
                if actual == fork_result_tree_path && fixture.publication.is_some() => {
                let publication = fixture.publication.as_ref().unwrap();
                json!({
                    "truncated": false,
                    "tree": [
                        { "path": "binary.dat", "mode": "100644", "type": "blob", "sha": publication.binary_blob_id },
                        { "path": "link", "mode": "120000", "type": "blob", "sha": publication.symlink_blob_id },
                        { "path": "mode.sh", "mode": "100755", "type": "blob", "sha": publication.mode_blob_id },
                        { "path": "text.txt", "mode": "100644", "type": "blob", "sha": publication.text_blob_id }
                    ]
                })
            }
            ("POST", actual) if actual == fork_ref_create_path => {
                fixture.branch_post_seen.store(true, Ordering::SeqCst);
                if fixture.ambiguous_branch_post {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({ "error": "response lost after ref create" })),
                    );
                }
                json!({
                    "ref": "refs/heads/cma/lnch_test",
                    "object": {
                        "sha": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                        "type": "commit"
                    }
                })
            }
            ("POST", actual) if actual == upstream_fork_create_path => {
                fixture.post_seen.store(true, Ordering::SeqCst);
                if fixture.ambiguous_fork_post {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({ "error": "response lost after create" })),
                    );
                }
                json!(fork_repository(10))
            }
            ("POST", "/api/v1/keys") => json!({
                "id": "service-created",
                "slug": GITHUB_SERVICE_SLUG,
                "credential_source": { "type": "personal" }
            }),
            ("GET", "/api/v1/providers/github/connect/oauth") => json!({
                "authorization_url": format!(
                    "https://github.com/login/oauth/authorize?client_id=test&state=1cc_{NONCE}"
                ),
                "attempt_nonce": NONCE
            }),
            _ => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "error": "unexpected fixture request" })),
                )
            }
        };
        (StatusCode::OK, Json(response))
    }

    fn fork_repository(parent_id: u64) -> Value {
        fork_repository_binding(parent_id, "upstream", 21)
    }

    fn fork_repository_binding(parent_id: u64, name: &str, owner_id: u64) -> Value {
        let (owner_node_id, owner) = if owner_id == 21 {
            ("U_kgDOBQ", "contributor")
        } else {
            ("U_transferred", "transferred-owner")
        };
        json!({
            "id": 30,
            "node_id": "R_fork",
            "name": name,
            "owner": {
                "id": owner_id,
                "node_id": owner_node_id,
                "login": owner
            },
            "private": false,
            "visibility": "public",
            "default_branch": "main",
            "allow_forking": true,
            "archived": false,
            "disabled": false,
            "html_url": format!("https://github.com/{owner}/{name}"),
            "permissions": { "pull": true, "push": true },
            "parent": { "id": parent_id },
            "source": { "id": 10 }
        })
    }

    async fn fixture_server(
        fixture: Fixture,
    ) -> (
        String,
        Arc<Mutex<Vec<RecordedRequest>>>,
        tokio::task::JoinHandle<()>,
    ) {
        let requests = Arc::clone(&fixture.requests);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind NyxID fixture");
        let address = listener.local_addr().expect("fixture address");
        let app = Router::new().fallback(any(answer)).with_state(fixture);
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve NyxID fixture");
        });
        (format!("http://{address}"), requests, server)
    }

    fn broker(base_url: &str, available: bool) -> GithubBroker {
        let authority = available.then(|| SubjectRequestAuthority {
            bearer: "subject-token".to_string(),
            token_generation: 7,
        });
        GithubBroker::with_authority(base_url, Arc::new(TestAuthority { value: authority }))
            .expect("construct broker")
    }

    fn broker_with_revocations(
        base_url: &str,
        store: Arc<TestConnectionRevocationStore>,
    ) -> GithubBroker {
        GithubBroker::with_ports(
            base_url,
            Arc::new(TestAuthority {
                value: Some(SubjectRequestAuthority {
                    bearer: "subject-token".to_string(),
                    token_generation: 7,
                }),
            }),
            store,
        )
        .expect("construct broker with durable revocations")
    }

    fn subject() -> SubjectId {
        SubjectId("subject-1".to_string())
    }

    fn fork_context() -> ForkContext {
        ForkContext {
            owner: subject(),
            connection: ConnectionSnapshot {
                service_id: "service-1".into(),
                service_state_version: 4,
                backing_key_id: "key-1".into(),
                account: ConnectedAccount {
                    account_id: 21,
                    account_node_id: "U_kgDOBQ".into(),
                    login: "contributor".into(),
                    avatar_url: Some("https://avatars.githubusercontent.com/u/21".into()),
                },
                state: ConnectionState::Active,
                granted_scopes: vec![PUBLIC_REPOSITORY_SCOPE.into()],
                satisfied_scope_classes: vec![ScopeClass::PublicRepository],
                consent_observed_at: "2026-08-26T01:02:03Z".into(),
                preflight_observed_at: "2026-08-26T01:02:04Z".into(),
            },
            upstream: RepositoryIdentity {
                repository_id: 10,
                repository_node_id: "R_kgDOAA".into(),
                owner_id: 11,
                owner_node_id: "U_kgDOAQ".into(),
                canonical_owner: "owner".into(),
                canonical_name: "upstream".into(),
                visibility: RepositoryVisibility::Public,
                allow_forking: true,
                default_branch: "main".into(),
                selected_ref: "main".into(),
                fork_parent_id: None,
                fork_source_id: None,
            },
            source_sha: "a".repeat(40),
        }
    }

    fn branch_context() -> BranchContext {
        let fork = fork_context();
        BranchContext {
            owner: fork.owner.clone(),
            launch_id: "lnch_test".into(),
            connection: fork.connection.clone(),
            fork: ForkRecord {
                repository: RepositoryIdentity {
                    repository_id: 30,
                    repository_node_id: "R_fork".into(),
                    owner_id: 21,
                    owner_node_id: "U_kgDOBQ".into(),
                    canonical_owner: "contributor".into(),
                    canonical_name: "upstream".into(),
                    visibility: RepositoryVisibility::Public,
                    allow_forking: true,
                    default_branch: "main".into(),
                    selected_ref: "main".into(),
                    fork_parent_id: Some(10),
                    fork_source_id: Some(10),
                },
                reused: true,
                source_revision: fork.source_sha.clone(),
                canonical_url: "https://github.com/contributor/upstream".into(),
                ready_at: "2026-08-26T01:02:05Z".into(),
            },
            source_sha: fork.source_sha,
        }
    }

    fn publication_record() -> ContributionRecord {
        ContributionRecord {
            id: ContributionId::new(),
            launch_id: LaunchId("lnch_test".into()),
            diff_artifact_id: DiffArtifactId::new(),
            revision_digest: "1".repeat(64),
            baseline_digest: "2".repeat(64),
            checkpoint_digest: "3".repeat(64),
            diff_digest: "4".repeat(64),
            branch_name: "cma/lnch_test".into(),
            state: PublicationState::IntentRecorded,
            expected_head: "a".repeat(40),
            base_tree_id: None,
            tree_id: None,
            commit_id: None,
            resulting_head: None,
            object_ids: Vec::new(),
            author: CommitIdentity {
                name: "contributor".into(),
                email: "21+contributor@users.noreply.github.com".into(),
            },
            committer: CommitIdentity {
                name: "CMA".into(),
                email: "cma@chrono.ai".into(),
            },
            confirmation: Confirmation {
                action: ConfirmationAction::PublishBranch,
                subject: subject(),
                snapshot_digest: "4".repeat(64),
                confirmed_at: "2026-08-26T12:34:56Z".into(),
                idempotency_key_hash: "confirmation-hash".into(),
            },
            intent_key: "cma-trigger:lnch_test:publish:test".into(),
            created_at: "2026-08-26T12:34:56Z".into(),
            updated_at: "2026-08-26T12:34:56Z".into(),
        }
    }

    fn publication_plan(
        record: &ContributionRecord,
    ) -> crate::trigger::workspace_diff::PublicationPlan {
        use crate::trigger::workspace_diff::{PublicationFile, PublicationPlan};

        let file = |path: &str,
                    change: DiffChangeKind,
                    binary: bool,
                    mode: &str,
                    content: Option<&[u8]>| {
            let content = content.map(<[u8]>::to_vec);
            PublicationFile {
                path: path.into(),
                change,
                binary,
                mode: mode.into(),
                content_sha256: content
                    .as_ref()
                    .map(|content| format!("{:x}", Sha256::digest(content))),
                blob_id: content
                    .as_ref()
                    .map(|content| crate::trigger::workspace_diff::git_blob_id(content)),
                content,
            }
        };
        PublicationPlan {
            launch_id: record.launch_id.clone(),
            artifact_id: record.diff_artifact_id.clone(),
            revision_digest: record.revision_digest.clone(),
            baseline_digest: record.baseline_digest.clone(),
            checkpoint_digest: record.checkpoint_digest.clone(),
            diff_digest: record.diff_digest.clone(),
            expected_head: record.expected_head.clone(),
            author: record.author.clone(),
            committer: record.committer.clone(),
            files: vec![
                file(
                    "binary.dat",
                    DiffChangeKind::Modified,
                    true,
                    "100644",
                    Some(&[0, 255, 1]),
                ),
                file(
                    "deleted.txt",
                    DiffChangeKind::Deleted,
                    false,
                    "100644",
                    None,
                ),
                file(
                    "link",
                    DiffChangeKind::SymlinkChanged,
                    false,
                    "120000",
                    Some(b"README.md"),
                ),
                file(
                    "mode.sh",
                    DiffChangeKind::ModeChanged,
                    false,
                    "100755",
                    Some(b"#!/bin/sh\n"),
                ),
                file(
                    "text.txt",
                    DiffChangeKind::Modified,
                    false,
                    "100644",
                    Some(b"text change\n"),
                ),
            ],
        }
    }

    fn attach_prepared(
        record: &mut ContributionRecord,
        state: PublicationState,
        prepared: &super::publication::PreparedGitData,
    ) {
        record.state = state;
        record.base_tree_id = Some(prepared.base_tree_id.clone());
        record.tree_id = Some(prepared.tree_id.clone());
        record.commit_id = Some(prepared.commit_id.clone());
        record.object_ids = prepared.object_ids.clone();
    }

    fn query(uri: &str, key: &str) -> Option<String> {
        Url::parse(&format!("http://fixture.test{uri}"))
            .expect("recorded URI")
            .query_pairs()
            .find_map(|(name, value)| (name == key).then(|| value.into_owned()))
    }

    #[tokio::test]
    async fn preflight_uses_exact_service_and_current_subject_authority() {
        let (base_url, requests, server) = fixture_server(Fixture::default()).await;
        let evidence = broker(&base_url, true)
            .preflight_connection(&subject(), "service-1")
            .await
            .expect("preflight succeeds");
        server.abort();

        assert_eq!(evidence.subject_token_generation, 7);
        assert_eq!(evidence.snapshot.service_id, "service-1");
        assert_eq!(evidence.snapshot.backing_key_id, "key-1");
        assert_eq!(evidence.snapshot.account.account_id, 21);
        assert_eq!(
            evidence.snapshot.satisfied_scope_classes,
            vec![ScopeClass::PublicRepository]
        );
        let requests = requests.lock().await;
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[0].uri, "/api/v1/user-services");
        assert_eq!(requests[1].uri, "/api/v1/keys/service-1/authorization");
        assert_eq!(
            requests[2].uri,
            "/api/v1/proxy/s/api-github/user?_nyxid_via=service-1"
        );
        assert!(requests
            .iter()
            .all(|request| request.authorization.as_deref() == Some("Bearer subject-token")));
    }

    #[tokio::test]
    async fn preflight_rejects_service_authorization_version_drift() {
        let fixture = Fixture {
            authorization_state_version: 3,
            ..Fixture::default()
        };
        let (base_url, requests, server) = fixture_server(fixture).await;
        let result = broker(&base_url, true)
            .preflight_connection(&subject(), "service-1")
            .await;
        server.abort();

        assert!(matches!(result, Err(GithubBrokerError::AuthorityDrift)));
        assert_eq!(requests.lock().await.len(), 2);
    }

    #[tokio::test]
    async fn preflight_rejects_missing_public_repository_scope() {
        let fixture = Fixture {
            scopes: vec!["read:user".to_string()],
            ..Fixture::default()
        };
        let (base_url, requests, server) = fixture_server(fixture).await;
        let result = broker(&base_url, true)
            .preflight_connection(&subject(), "service-1")
            .await;
        server.abort();

        assert!(matches!(
            result,
            Err(GithubBrokerError::InsufficientScope {
                required: PUBLIC_REPOSITORY_SCOPE
            })
        ));
        assert_eq!(requests.lock().await.len(), 2);
    }

    #[tokio::test]
    async fn oauth_calls_are_typed_and_nonce_bound() {
        let (base_url, requests, server) = fixture_server(Fixture::default()).await;
        let broker = broker(&base_url, true);
        let service_id = broker
            .create_oauth_placeholder(&subject(), "cma-trigger:intent-1")
            .await
            .expect("create placeholder");
        let start = broker
            .begin_oauth(&subject(), "service-1")
            .await
            .expect("begin OAuth");
        server.abort();

        assert_eq!(service_id, "service-created");
        assert_eq!(start.service_id, "service-1");
        assert_eq!(start.attempt_nonce, NONCE);
        assert!(start.authorization_url.starts_with("https://github.com/"));
        let requests = requests.lock().await;
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[0].method, "POST");
        assert_eq!(requests[0].uri, "/api/v1/keys");
        assert_eq!(
            requests[0].body,
            Some(json!({
                "service_slug": GITHUB_SERVICE_SLUG,
                "label": "cma-trigger:intent-1"
            }))
        );
        assert_eq!(requests[1].uri, "/api/v1/user-services");
        assert_eq!(query(&requests[2].uri, "flow").as_deref(), Some("cc"));
        assert_eq!(
            query(&requests[2].uri, "scope_override").as_deref(),
            Some(PUBLIC_REPOSITORY_SCOPE)
        );
        assert_eq!(
            query(&requests[2].uri, "key_id").as_deref(),
            Some("service-1")
        );
    }

    #[tokio::test]
    async fn connect_reconciles_a_prior_placeholder_by_deterministic_label() {
        let intent = "123e4567-e89b-12d3-a456-426614174001";
        let fixture = Fixture {
            label: Some(format!("cma-trigger:{intent}")),
            ..Fixture::default()
        };
        let (base_url, requests, server) = fixture_server(fixture).await;
        let start = TriggerConnectionManager::begin_connection(
            &broker(&base_url, true),
            &subject(),
            intent,
        )
        .await
        .expect("reconcile and begin OAuth");
        server.abort();

        assert_eq!(start.service_id, "service-1");
        let requests = requests.lock().await;
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[0].uri, "/api/v1/user-services");
        assert_eq!(requests[1].uri, "/api/v1/user-services");
        assert!(requests.iter().all(|request| request.method == "GET"));
        assert_eq!(
            query(&requests[2].uri, "key_id").as_deref(),
            Some("service-1")
        );
    }

    #[tokio::test]
    async fn repository_resolution_uses_numeric_identity_and_an_exact_ref() {
        let (base_url, requests, server) = fixture_server(Fixture::default()).await;
        let selection = TriggerConnectionManager::resolve_repository(
            &broker(&base_url, true),
            &subject(),
            "service-1",
            10,
            "main",
        )
        .await
        .expect("resolve repository");
        server.abort();

        assert_eq!(selection.repository.repository_id, 10);
        assert_eq!(selection.repository.owner_id, 11);
        assert_eq!(selection.repository.selected_ref, "main");
        assert_eq!(selection.source_sha, "a".repeat(40));
        let requests = requests.lock().await;
        assert_eq!(requests.len(), 5);
        assert_eq!(
            requests[3].uri,
            "/api/v1/proxy/s/api-github/repositories/10?_nyxid_via=service-1"
        );
        assert_eq!(
            requests[4].uri,
            "/api/v1/proxy/s/api-github/repos/owner/upstream/git/ref/heads/main?_nyxid_via=service-1"
        );
    }

    #[tokio::test]
    async fn workflow_review_uses_the_exact_source_tree_and_stable_paths() {
        let (base_url, requests, server) = fixture_server(Fixture::default()).await;
        let context = fork_context();
        let paths = broker(&base_url, true)
            .workflow_paths(
                &context.owner,
                &context.connection.service_id,
                &context.upstream,
                &context.source_sha,
            )
            .await
            .expect("read exact source workflow paths");
        server.abort();

        assert_eq!(
            paths,
            vec![
                ".github/workflows/checks.yaml",
                ".github/workflows/release.yml"
            ]
        );
        let requests = requests.lock().await;
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].uri,
            "/api/v1/proxy/s/api-github/repos/owner/upstream/git/trees/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa?recursive=1&_nyxid_via=service-1"
        );
    }

    #[tokio::test]
    async fn truncated_workflow_inventory_fails_closed() {
        let fixture = Fixture {
            tree_truncated: true,
            ..Fixture::default()
        };
        let (base_url, _requests, server) = fixture_server(fixture).await;
        let context = fork_context();
        let result = broker(&base_url, true)
            .workflow_paths(
                &context.owner,
                &context.connection.service_id,
                &context.upstream,
                &context.source_sha,
            )
            .await;
        server.abort();

        assert!(matches!(result, Err(GithubBrokerError::Malformed(_))));
    }

    #[tokio::test]
    async fn compatible_fork_is_reused_only_after_network_and_commit_proof() {
        let (base_url, requests, server) = fixture_server(Fixture::default()).await;
        let result = broker(&base_url, true)
            .reconcile_fork(&fork_context())
            .await
            .expect("reconcile fork");
        server.abort();

        let ForkReconciliation::Ready(fork) = result else {
            panic!("expected a ready fork");
        };
        assert!(fork.reused);
        assert_eq!(fork.repository.repository_id, 30);
        assert_eq!(fork.repository.fork_parent_id, Some(10));
        assert_eq!(fork.repository.fork_source_id, Some(10));
        assert_eq!(fork.source_revision, "a".repeat(40));
        let requests = requests.lock().await;
        assert_eq!(requests.len(), 4);
        assert_eq!(requests[0].method, "GET");
        assert_eq!(
            query(&requests[0].uri, "affiliation").as_deref(),
            Some("owner")
        );
        assert_eq!(
            requests[1].uri,
            "/api/v1/proxy/s/api-github/repositories/30?_nyxid_via=service-1"
        );
        assert!(requests[3]
            .uri
            .contains("/repos/contributor/upstream/git/commits/"));
    }

    #[tokio::test]
    async fn sibling_fork_network_slot_is_named_and_never_reused() {
        let fixture = Fixture {
            fork_parent_id: Some(12),
            ..Fixture::default()
        };
        let (base_url, requests, server) = fixture_server(fixture).await;
        let result = broker(&base_url, true)
            .reconcile_fork(&fork_context())
            .await;
        server.abort();

        assert!(matches!(
            result,
            Err(GithubBrokerError::ForkNetworkOccupied)
        ));
        assert_eq!(requests.lock().await.len(), 1);
    }

    #[tokio::test]
    async fn absent_fork_is_created_once_then_proven_ready() {
        let fixture = Fixture {
            fork_parent_id: None,
            ..Fixture::default()
        };
        let (base_url, requests, server) = fixture_server(fixture).await;
        let result = broker(&base_url, true)
            .create_fork(&fork_context())
            .await
            .expect("create fork");
        server.abort();

        let ForkReconciliation::Ready(fork) = result else {
            panic!("expected a ready fork");
        };
        assert!(!fork.reused);
        let requests = requests.lock().await;
        assert_eq!(
            requests
                .iter()
                .filter(|request| request.method == "POST")
                .count(),
            1
        );
        let create = requests
            .iter()
            .find(|request| request.method == "POST")
            .unwrap();
        assert_eq!(
            create.uri,
            "/api/v1/proxy/s/api-github/repos/owner/upstream/forks?_nyxid_via=service-1"
        );
        assert_eq!(create.body, Some(json!({})));
    }

    #[tokio::test]
    async fn fork_create_uses_the_numeric_lookup_canonical_name_after_rename() {
        let fixture = Fixture {
            fork_parent_id: None,
            upstream_name: "upstream-renamed".into(),
            ..Fixture::default()
        };
        let (base_url, requests, server) = fixture_server(fixture).await;

        assert!(matches!(
            broker(&base_url, true).create_fork(&fork_context()).await,
            Ok(ForkReconciliation::Ready(_))
        ));
        server.abort();

        let requests = requests.lock().await;
        let create = requests
            .iter()
            .find(|request| request.method == "POST")
            .expect("fork create request");
        assert_eq!(
            create.uri,
            "/api/v1/proxy/s/api-github/repos/owner/upstream-renamed/forks?_nyxid_via=service-1"
        );
    }

    #[tokio::test]
    async fn fork_create_rejects_numeric_owner_transfer_before_the_write() {
        let fixture = Fixture {
            fork_parent_id: None,
            upstream_owner_id: 99,
            ..Fixture::default()
        };
        let (base_url, requests, server) = fixture_server(fixture).await;

        assert!(matches!(
            broker(&base_url, true).create_fork(&fork_context()).await,
            Err(GithubBrokerError::AuthorityDrift)
        ));
        server.abort();
        assert!(requests
            .lock()
            .await
            .iter()
            .all(|request| request.method == "GET"));
    }

    #[tokio::test]
    async fn fork_create_rejects_missing_current_read_authority_before_the_write() {
        let fixture = Fixture {
            fork_parent_id: None,
            upstream_pull: false,
            ..Fixture::default()
        };
        let (base_url, requests, server) = fixture_server(fixture).await;

        assert!(matches!(
            broker(&base_url, true).create_fork(&fork_context()).await,
            Err(GithubBrokerError::AuthorityDrift)
        ));
        server.abort();
        assert!(requests
            .lock()
            .await
            .iter()
            .all(|request| request.method == "GET"));
    }

    #[tokio::test]
    async fn disabled_forking_fails_before_the_provider_write() {
        let fixture = Fixture {
            fork_parent_id: None,
            allow_forking: false,
            ..Fixture::default()
        };
        let (base_url, requests, server) = fixture_server(fixture).await;
        let result = broker(&base_url, true).create_fork(&fork_context()).await;
        server.abort();

        assert!(matches!(result, Err(GithubBrokerError::ForkingDisabled)));
        assert!(requests
            .lock()
            .await
            .iter()
            .all(|request| request.method == "GET"));
    }

    #[tokio::test]
    async fn stale_connection_authority_blocks_fork_creation_before_the_write() {
        let fixture = Fixture {
            fork_parent_id: None,
            service_state_version: 5,
            authorization_state_version: 5,
            ..Fixture::default()
        };
        let (base_url, requests, server) = fixture_server(fixture).await;
        let result = broker(&base_url, true).create_fork(&fork_context()).await;
        server.abort();

        assert!(matches!(result, Err(GithubBrokerError::AuthorityDrift)));
        assert!(requests
            .lock()
            .await
            .iter()
            .all(|request| request.method == "GET"));
    }

    #[tokio::test]
    async fn ambiguous_create_is_reconciled_by_the_worker_without_a_second_post() {
        use crate::trigger::github_effects::GithubEffectExecutor;
        use crate::trigger::worker::{EffectRunOutcome, EffectWorker};

        let fixture = Fixture {
            fork_parent_id: None,
            ambiguous_fork_post: true,
            fork_appears_after_post: true,
            ..Fixture::default()
        };
        let (base_url, requests, server) = fixture_server(fixture).await;
        let github = Arc::new(broker(&base_url, true));
        let executor = Arc::new(GithubEffectExecutor::new(github));
        let context = fork_context();
        let mut launch = crate::trigger::worker::tests::launch_fixture();
        launch.owner = context.owner.clone();
        launch.connection = Some(context.connection.clone());
        launch.revision.content.upstream.repository = context.upstream.clone();
        launch.source_sha = context.source_sha.clone();
        launch.state = layer2_core::LaunchState::ForkPending;
        launch.effects.clear();
        launch.lease = None;
        let service = crate::trigger::worker::tests::service_for_launch(launch.clone());
        let worker = EffectWorker::new(service.clone(), executor, "fork-integration").unwrap();

        assert_eq!(
            worker
                .run_once(&launch.id, launch.revision_number, "ambiguous-create")
                .await
                .unwrap(),
            EffectRunOutcome::Deferred
        );
        let deferred = service.get_launch(&launch.owner, &launch.id).await.unwrap();
        assert_eq!(
            deferred.effects[0].state,
            layer2_core::EffectState::Reconciling
        );

        tokio::time::sleep(Duration::from_millis(2_100)).await;
        assert_eq!(
            worker
                .run_once(&launch.id, deferred.revision_number, "reconcile-create")
                .await
                .unwrap(),
            EffectRunOutcome::Completed
        );
        let completed = service.get_launch(&launch.owner, &launch.id).await.unwrap();
        assert_eq!(completed.state, layer2_core::LaunchState::ForkReady);
        assert_eq!(completed.fork.unwrap().repository.repository_id, 30);
        assert_eq!(
            requests
                .lock()
                .await
                .iter()
                .filter(|request| request.method == "POST")
                .count(),
            1
        );
        server.abort();
    }

    #[tokio::test]
    async fn launch_branch_is_created_once_at_the_exact_source_without_force() {
        let (base_url, requests, server) = fixture_server(Fixture::default()).await;
        let result = broker(&base_url, true)
            .create_branch(&branch_context())
            .await
            .expect("create branch");
        server.abort();

        let BranchReconciliation::Ready(branch) = result else {
            panic!("expected a ready branch");
        };
        assert_eq!(branch.name, "cma/lnch_test");
        assert_eq!(branch.expected_head, "a".repeat(40));
        assert_eq!(branch.current_head, "a".repeat(40));
        assert_eq!(
            branch.canonical_url,
            "https://github.com/contributor/upstream/tree/cma/lnch_test"
        );
        let requests = requests.lock().await;
        let posts: Vec<_> = requests
            .iter()
            .filter(|request| request.method == "POST")
            .collect();
        assert_eq!(posts.len(), 1);
        assert_eq!(
            posts[0].uri,
            "/api/v1/proxy/s/api-github/repos/contributor/upstream/git/refs?_nyxid_via=service-1"
        );
        assert_eq!(
            posts[0].body,
            Some(json!({
                "ref": "refs/heads/cma/lnch_test",
                "sha": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            }))
        );
        assert!(posts[0]
            .body
            .as_ref()
            .is_some_and(|body| body.get("force").is_none()));
    }

    #[tokio::test]
    async fn branch_create_rebinds_a_rename_between_absence_proof_and_post() {
        let fixture = Fixture {
            rebound_fork_name: Some("upstream-renamed".into()),
            ..Fixture::default()
        };
        let lookups = Arc::clone(&fixture.fork_repository_lookups);
        let (base_url, requests, server) = fixture_server(fixture).await;

        let result = broker(&base_url, true)
            .create_branch(&branch_context())
            .await
            .expect("create branch after canonical rename");
        server.abort();

        let BranchReconciliation::Ready(branch) = result else {
            panic!("expected a ready branch");
        };
        assert_eq!(lookups.load(Ordering::SeqCst), 3);
        assert_eq!(
            branch.canonical_url,
            "https://github.com/contributor/upstream-renamed/tree/cma/lnch_test"
        );
        let requests = requests.lock().await;
        let create = requests
            .iter()
            .find(|request| request.method == "POST")
            .expect("branch create request");
        assert_eq!(
            create.uri,
            "/api/v1/proxy/s/api-github/repos/contributor/upstream-renamed/git/refs?_nyxid_via=service-1"
        );
        let create_index = requests
            .iter()
            .position(|request| request.method == "POST")
            .unwrap();
        assert!(requests[create_index - 1]
            .uri
            .starts_with("/api/v1/proxy/s/api-github/repositories/30?"));
    }

    #[tokio::test]
    async fn branch_create_rejects_transfer_between_absence_proof_and_post() {
        let fixture = Fixture {
            rebound_fork_owner_id: Some(99),
            ..Fixture::default()
        };
        let (base_url, requests, server) = fixture_server(fixture).await;

        assert!(matches!(
            broker(&base_url, true)
                .create_branch(&branch_context())
                .await,
            Err(GithubBrokerError::AuthorityDrift)
        ));
        server.abort();
        assert!(requests
            .lock()
            .await
            .iter()
            .all(|request| request.method == "GET"));
    }

    #[tokio::test]
    async fn ambiguous_branch_create_is_reconciled_without_a_second_post() {
        let fixture = Fixture {
            ambiguous_branch_post: true,
            branch_appears_after_post: true,
            ..Fixture::default()
        };
        let (base_url, requests, server) = fixture_server(fixture).await;
        let broker = broker(&base_url, true);
        let context = branch_context();

        assert!(matches!(
            broker.create_branch(&context).await,
            Err(GithubBrokerError::AmbiguousWrite)
        ));
        assert!(matches!(
            broker.reconcile_branch(&context).await,
            Ok(BranchReconciliation::Ready(_))
        ));
        server.abort();

        assert_eq!(
            requests
                .lock()
                .await
                .iter()
                .filter(|request| request.method == "POST")
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn occupied_launch_ref_is_never_reset_or_recreated() {
        let fixture = Fixture {
            branch_head: Some("b".repeat(40)),
            ..Fixture::default()
        };
        let (base_url, requests, server) = fixture_server(fixture).await;
        let result = broker(&base_url, true)
            .create_branch(&branch_context())
            .await;
        server.abort();

        assert!(matches!(result, Err(GithubBrokerError::BranchHeadConflict)));
        assert!(requests
            .lock()
            .await
            .iter()
            .all(|request| request.method == "GET"));
    }

    #[tokio::test]
    async fn git_data_publication_is_three_phase_exact_and_verified() {
        let provider = PublicationFixture::new(PublicationPatch::Confirmed);
        let fixture = Fixture {
            publication: Some(provider.clone()),
            ..Fixture::default()
        };
        let (base_url, requests, server) = fixture_server(fixture).await;
        let github = broker(&base_url, true);
        let branch = branch_context();
        let mut record = publication_record();
        let plan = publication_plan(&record);

        let prepared = match github
            .publish_branch(&PublicationContext::fixture(&branch, record.clone()), &plan)
            .await
            .expect("write exact reviewed objects")
        {
            PublicationAdvance::ObjectsWritten(prepared) => prepared,
            other => panic!("unexpected first publication phase: {other:?}"),
        };
        assert_eq!(prepared.base_tree_id, provider.base_tree_id);
        assert_eq!(prepared.tree_id, provider.tree_id);
        assert_eq!(prepared.commit_id, provider.commit_id);
        assert_eq!(prepared.object_ids.len(), 4);

        attach_prepared(&mut record, PublicationState::ObjectsWritten, &prepared);
        let pending = match github
            .publish_branch(&PublicationContext::fixture(&branch, record.clone()), &plan)
            .await
            .expect("verify objects before ref intent")
        {
            PublicationAdvance::RefUpdatePending(prepared) => prepared,
            other => panic!("unexpected second publication phase: {other:?}"),
        };
        assert_eq!(pending, prepared);

        attach_prepared(&mut record, PublicationState::RefUpdatePending, &pending);
        let published = match github
            .publish_branch(&PublicationContext::fixture(&branch, record), &plan)
            .await
            .expect("guard and verify ref update")
        {
            PublicationAdvance::Published(published) => published,
            other => panic!("unexpected final publication phase: {other:?}"),
        };
        server.abort();
        assert_eq!(published.commit_id, provider.commit_id);
        assert_eq!(published.resulting_head, provider.commit_id);
        assert_eq!(*provider.head.lock().await, provider.commit_id);

        let requests = requests.lock().await;
        let writes = requests
            .iter()
            .enumerate()
            .filter(|(_, request)| matches!(request.method.as_str(), "POST" | "PATCH"))
            .collect::<Vec<_>>();
        assert_eq!(writes.len(), 4);
        for (index, _) in &writes {
            let index = *index;
            assert!(index >= 4);
            assert_eq!(requests[index - 4].uri, "/api/v1/user-services");
            assert_eq!(
                requests[index - 3].uri,
                "/api/v1/keys/service-1/authorization"
            );
            assert!(requests[index - 2]
                .uri
                .starts_with("/api/v1/proxy/s/api-github/user?"));
            assert!(requests[index - 1]
                .uri
                .starts_with("/api/v1/proxy/s/api-github/repositories/30?"));
        }
        assert_eq!(
            writes[0].1.body,
            Some(json!({ "content": "AP8B", "encoding": "base64" }))
        );
        assert_eq!(
            writes[1].1.body,
            Some(json!({
                "base_tree": provider.base_tree_id,
                "tree": [
                    { "path": "binary.dat", "mode": "100644", "type": "blob", "sha": provider.binary_blob_id },
                    { "path": "deleted.txt", "mode": "100644", "type": "blob", "sha": Value::Null },
                    { "path": "link", "mode": "120000", "type": "blob", "content": "README.md" },
                    { "path": "mode.sh", "mode": "100755", "type": "blob", "sha": provider.mode_blob_id },
                    { "path": "text.txt", "mode": "100644", "type": "blob", "content": "text change\n" }
                ]
            }))
        );
        assert_eq!(
            writes[2].1.body,
            Some(json!({
                "message": "CMA Trigger contribution\n\nLaunch: lnch_test\nReviewed diff: 4444444444444444444444444444444444444444444444444444444444444444",
                "tree": provider.tree_id,
                "parents": ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"],
                "author": {
                    "name": "contributor",
                    "email": "21+contributor@users.noreply.github.com",
                    "date": "2026-08-26T12:34:56Z"
                },
                "committer": {
                    "name": "CMA",
                    "email": "cma@chrono.ai",
                    "date": "2026-08-26T12:34:56Z"
                }
            }))
        );
        assert_eq!(
            writes[3].1.body,
            Some(json!({ "sha": provider.commit_id, "force": false }))
        );
        assert!(requests
            .iter()
            .all(|request| { request.authorization.as_deref() == Some("Bearer subject-token") }));
    }

    #[tokio::test]
    async fn publication_rebinds_each_object_write_after_a_canonical_rename() {
        let provider = PublicationFixture::new(PublicationPatch::Confirmed);
        let fixture = Fixture {
            publication: Some(provider),
            rebound_fork_name: Some("upstream-renamed".into()),
            ..Fixture::default()
        };
        let (base_url, requests, server) = fixture_server(fixture).await;
        let branch = branch_context();
        let record = publication_record();
        let plan = publication_plan(&record);

        assert!(matches!(
            broker(&base_url, true)
                .publish_branch(&PublicationContext::fixture(&branch, record), &plan)
                .await,
            Ok(PublicationAdvance::ObjectsWritten(_))
        ));
        server.abort();

        let requests = requests.lock().await;
        let writes = requests
            .iter()
            .enumerate()
            .filter(|(_, request)| request.method == "POST")
            .collect::<Vec<_>>();
        assert_eq!(writes.len(), 3);
        for (index, request) in writes {
            assert!(request
                .uri
                .contains("/repos/contributor/upstream-renamed/git/"));
            assert!(requests[index - 1]
                .uri
                .starts_with("/api/v1/proxy/s/api-github/repositories/30?"));
        }
    }

    #[tokio::test]
    async fn publication_repreflights_connection_before_each_object_write() {
        // Preflight zero is the entry guard. Subsequent preflights guard the
        // blob, tree, and commit writes respectively.
        for (drift_after, expected_writes) in [(0, 0), (1, 0), (2, 1), (3, 2)] {
            let fixture = Fixture {
                publication: Some(PublicationFixture::new(PublicationPatch::Confirmed)),
                authority_drift_after: Some(drift_after),
                ..Fixture::default()
            };
            let (base_url, requests, server) = fixture_server(fixture).await;
            let branch = branch_context();
            let record = publication_record();
            let plan = publication_plan(&record);

            let result = broker(&base_url, true)
                .publish_branch(&PublicationContext::fixture(&branch, record), &plan)
                .await;
            server.abort();

            assert!(matches!(result, Err(GithubBrokerError::AuthorityDrift)));
            assert_eq!(
                requests
                    .lock()
                    .await
                    .iter()
                    .filter(|request| request.method == "POST")
                    .count(),
                expected_writes,
                "authority drift after preflight {drift_after} admitted an extra object write"
            );
        }
    }

    #[tokio::test]
    async fn publication_rejects_transfer_between_preflight_and_first_object_write() {
        let fixture = Fixture {
            publication: Some(PublicationFixture::new(PublicationPatch::Confirmed)),
            rebound_fork_owner_id: Some(99),
            ..Fixture::default()
        };
        let (base_url, requests, server) = fixture_server(fixture).await;
        let branch = branch_context();
        let record = publication_record();
        let plan = publication_plan(&record);

        assert!(matches!(
            broker(&base_url, true)
                .publish_branch(&PublicationContext::fixture(&branch, record), &plan)
                .await,
            Err(GithubBrokerError::AuthorityDrift)
        ));
        server.abort();
        assert!(requests
            .lock()
            .await
            .iter()
            .all(|request| request.method == "GET"));
    }

    #[tokio::test]
    async fn ambiguous_ref_response_is_reconciled_without_a_second_patch() {
        let provider = PublicationFixture::new(PublicationPatch::AmbiguousApplied);
        let fixture = Fixture {
            publication: Some(provider.clone()),
            rebound_fork_name: Some("upstream-renamed".into()),
            ..Fixture::default()
        };
        let (base_url, requests, server) = fixture_server(fixture).await;
        let github = broker(&base_url, true);
        let branch = branch_context();
        let mut record = publication_record();
        let plan = publication_plan(&record);
        let prepared = match github
            .publish_branch(&PublicationContext::fixture(&branch, record.clone()), &plan)
            .await
            .unwrap()
        {
            PublicationAdvance::ObjectsWritten(prepared) => prepared,
            other => panic!("unexpected publication phase: {other:?}"),
        };
        attach_prepared(&mut record, PublicationState::ObjectsWritten, &prepared);
        let pending = match github
            .publish_branch(&PublicationContext::fixture(&branch, record.clone()), &plan)
            .await
            .unwrap()
        {
            PublicationAdvance::RefUpdatePending(prepared) => prepared,
            other => panic!("unexpected publication phase: {other:?}"),
        };
        attach_prepared(&mut record, PublicationState::RefUpdatePending, &pending);
        assert!(matches!(
            github
                .publish_branch(&PublicationContext::fixture(&branch, record), &plan)
                .await,
            Ok(PublicationAdvance::Published(_))
        ));
        server.abort();
        let requests = requests.lock().await;
        let patch_index = requests
            .iter()
            .position(|request| request.method == "PATCH")
            .expect("ref patch request");
        assert_eq!(
            requests
                .iter()
                .filter(|request| request.method == "PATCH")
                .count(),
            1
        );
        assert!(requests[patch_index]
            .uri
            .contains("/repos/contributor/upstream-renamed/git/refs/"));
        assert!(requests[patch_index + 1]
            .uri
            .starts_with("/api/v1/proxy/s/api-github/repositories/30?"));
        assert!(requests[patch_index + 2]
            .uri
            .contains("/repos/contributor/upstream-renamed/git/ref/"));
    }

    #[tokio::test]
    async fn provider_refusals_remain_named_after_ref_reconciliation() {
        for (patch, expected) in [
            (
                PublicationPatch::NonFastForward,
                GithubRejectionKind::NonFastForward,
            ),
            (
                PublicationPatch::WorkflowScope,
                GithubRejectionKind::WorkflowScope,
            ),
        ] {
            let provider = PublicationFixture::new(patch);
            let fixture = Fixture {
                publication: Some(provider),
                ..Fixture::default()
            };
            let (base_url, _requests, server) = fixture_server(fixture).await;
            let github = broker(&base_url, true);
            let branch = branch_context();
            let mut record = publication_record();
            let plan = publication_plan(&record);
            let prepared = match github
                .publish_branch(&PublicationContext::fixture(&branch, record.clone()), &plan)
                .await
                .unwrap()
            {
                PublicationAdvance::ObjectsWritten(prepared) => prepared,
                other => panic!("unexpected publication phase: {other:?}"),
            };
            attach_prepared(&mut record, PublicationState::ObjectsWritten, &prepared);
            let pending = match github
                .publish_branch(&PublicationContext::fixture(&branch, record.clone()), &plan)
                .await
                .unwrap()
            {
                PublicationAdvance::RefUpdatePending(prepared) => prepared,
                other => panic!("unexpected publication phase: {other:?}"),
            };
            attach_prepared(&mut record, PublicationState::RefUpdatePending, &pending);
            let result = github
                .publish_branch(&PublicationContext::fixture(&branch, record), &plan)
                .await;
            server.abort();
            assert!(matches!(
                result,
                Err(GithubBrokerError::Rejected { kind, .. }) if kind == expected
            ));
        }
    }

    #[test]
    fn forbidden_workflow_and_secondary_rate_limits_are_not_flattened_to_forbidden() {
        let workflow = parse_rejection(
            br#"{"message":"OAuth token cannot update workflow without workflow scope","errors":[{"resource":"Workflow","field":"workflow","code":"missing_workflow_scope"}]}"#,
        )
        .unwrap();
        assert!(matches!(
            classify_status(StatusCode::FORBIDDEN, None, false, Some(workflow)),
            GithubBrokerError::Rejected {
                status: 403,
                kind: GithubRejectionKind::WorkflowScope,
                provider_code: Some(code),
            } if code == "missing_workflow_scope"
        ));

        let secondary =
            parse_rejection(br#"{"message":"You have exceeded a secondary rate limit"}"#).unwrap();
        assert!(matches!(
            classify_status(
                StatusCode::FORBIDDEN,
                Some("17".into()),
                false,
                Some(secondary),
            ),
            GithubBrokerError::RateLimited {
                retry_after: Some(value),
            } if value == "17"
        ));
    }

    #[tokio::test]
    async fn workspace_import_preserves_binary_executable_and_symlink_identity() {
        let fixture = Fixture {
            branch_head: Some("a".repeat(40)),
            workspace_tree: true,
            ..Fixture::default()
        };
        let (base_url, requests, server) = fixture_server(fixture).await;
        let github = broker(&base_url, true);
        let context = WorkspaceContext {
            branch: branch_context(),
        };
        let (_source, manifest, plan, staged) = materialize_workspace(&github, &context)
            .await
            .expect("import workspace");
        server.abort();

        manifest.validate().expect("sealed manifest");
        assert_eq!(manifest.source_revision, "a".repeat(40));
        assert_eq!(manifest.entries.len(), 3);
        assert_eq!(manifest.total_bytes, 18);
        let executable = manifest
            .entries
            .iter()
            .find(|entry| entry.path == "README.md")
            .unwrap();
        assert_eq!(executable.mode, 0o100755);
        let binary = manifest
            .entries
            .iter()
            .find(|entry| entry.path == "bin/data.bin")
            .unwrap();
        assert_eq!(
            binary.provider_object_id.as_deref(),
            Some("046f393ee9d8a6a0754b8ddadce727b4ce35f272")
        );
        let symlink = manifest
            .entries
            .iter()
            .find(|entry| entry.path == "readme-link")
            .unwrap();
        assert_eq!(symlink.kind, layer2_core::WorkspaceEntryKind::Symlink);
        assert_eq!(symlink.symlink_target.as_deref(), Some("README.md"));
        let mut archive = plan.preamble.clone();
        for chunk in &plan.chunks {
            let bytes = staged.get(&chunk.provider_object_id).unwrap();
            let start = chunk.ordinal as usize * layer2_core::BASELINE_IMPORT_CHUNK_BYTES as usize;
            let end = start + chunk.bytes as usize;
            assert_eq!(
                chunk.sha256,
                format!("{:x}", Sha256::digest(&bytes[start..end]))
            );
            archive.extend_from_slice(&bytes[start..end]);
        }
        assert_eq!(archive.len() as u64, plan.archive_bytes);
        assert_eq!(
            &archive[plan.preamble.len()..],
            b"hello\n\x00\xff\x01README.md"
        );
        let requests = requests.lock().await;
        assert_eq!(
            requests
                .iter()
                .filter(|request| request.uri.contains("/git/blobs/"))
                .count(),
            3
        );
        assert!(requests.iter().all(|request| request.method == "GET"));
    }

    #[tokio::test]
    async fn workspace_import_refuses_truncated_tree_before_any_blob_read() {
        let fixture = Fixture {
            branch_head: Some("a".repeat(40)),
            workspace_tree: true,
            tree_truncated: true,
            ..Fixture::default()
        };
        let (base_url, requests, server) = fixture_server(fixture).await;
        let result = broker(&base_url, true)
            .import_workspace(&WorkspaceContext {
                branch: branch_context(),
            })
            .await;
        server.abort();

        assert!(matches!(
            result,
            Err(GithubBrokerError::UnsupportedWorkspace(
                WorkspaceUnsupported::TreeIncomplete
            ))
        ));
        assert!(requests
            .lock()
            .await
            .iter()
            .all(|request| !request.uri.contains("/git/blobs/")));
    }

    #[tokio::test]
    async fn workspace_import_refuses_an_oversize_provider_blob_before_requesting_it() {
        let fixture = Fixture {
            branch_head: Some("a".repeat(40)),
            workspace_tree: true,
            workspace_oversize_blob: true,
            ..Fixture::default()
        };
        let (base_url, requests, server) = fixture_server(fixture).await;
        let result = broker(&base_url, true)
            .import_workspace(&WorkspaceContext {
                branch: branch_context(),
            })
            .await;
        server.abort();

        assert!(matches!(
            result,
            Err(GithubBrokerError::UnsupportedWorkspace(
                WorkspaceUnsupported::FileSize
            ))
        ));
        assert!(requests
            .lock()
            .await
            .iter()
            .all(|request| !request.uri.contains("/git/blobs/")));
    }

    #[tokio::test]
    async fn workspace_import_refuses_lfs_attributes() {
        let fixture = Fixture {
            branch_head: Some("a".repeat(40)),
            workspace_tree: true,
            workspace_lfs: true,
            ..Fixture::default()
        };
        let (base_url, _requests, server) = fixture_server(fixture).await;
        let github = broker(&base_url, true);
        let result = materialize_workspace(
            &github,
            &WorkspaceContext {
                branch: branch_context(),
            },
        )
        .await;
        server.abort();

        assert!(matches!(
            result,
            Err(GithubBrokerError::UnsupportedWorkspace(
                WorkspaceUnsupported::LargeFileStorage
            ))
        ));
    }

    async fn materialize_workspace(
        github: &GithubBroker,
        context: &WorkspaceContext,
    ) -> Result<
        (
            WorkspaceSourcePlan,
            layer2_core::WorkspaceManifest,
            layer2_core::BaselineArchivePlan,
            BTreeMap<String, Vec<u8>>,
        ),
        GithubBrokerError,
    > {
        let source = github.import_workspace(context).await?;
        let mut metadata = BTreeMap::new();
        let mut staged = BTreeMap::new();
        for blob in &source.blobs {
            let bytes = github.read_workspace_blob(context, blob).await?;
            let chunks = bytes
                .chunks(layer2_core::BASELINE_IMPORT_CHUNK_BYTES as usize)
                .enumerate()
                .map(|(ordinal, part)| layer2_core::BaselineImportChunk {
                    provider_object_id: blob.provider_object_id.clone(),
                    ordinal: ordinal as u32,
                    bytes: part.len() as u64,
                    sha256: format!("{:x}", Sha256::digest(part)),
                })
                .collect();
            let needs_symlink_target = source.entries.iter().any(|entry| {
                entry.provider_object_id == blob.provider_object_id && entry.mode == 0o120000
            });
            metadata.insert(
                blob.provider_object_id.clone(),
                workspace_blob_metadata(blob, &bytes, chunks, needs_symlink_target)?,
            );
            staged.insert(blob.provider_object_id.clone(), bytes);
        }
        let (manifest, plan) = build_baseline_archive_plan(&source, &metadata)?;
        Ok((source, manifest, plan, staged))
    }

    #[tokio::test]
    async fn connection_revocation_is_stable_across_replay_restart_and_github_outage() {
        let revocation = RevocationFixture {
            github_user_available: false,
            ..RevocationFixture::default()
        };
        let fixture = Fixture {
            revocation: Some(revocation.clone()),
            ..Fixture::default()
        };
        let (base_url, requests, server) = fixture_server(fixture).await;
        let store = Arc::new(TestConnectionRevocationStore::default());
        let broker = broker_with_revocations(&base_url, Arc::clone(&store));

        let first = TriggerConnectionManager::revoke_connection(&broker, &subject(), "service-1")
            .await
            .expect("first revoke settles");
        assert_eq!(first.service_id, "service-1");
        assert_eq!(first.state, ConnectionState::Inactive);
        assert_eq!(first.state_version, 5);
        assert_eq!(revocation.delete_calls.load(Ordering::SeqCst), 1);
        assert_eq!(store.fenced(), 1);

        let replay = TriggerConnectionManager::revoke_connection(&broker, &subject(), "service-1")
            .await
            .expect("different command key converges at the backend");
        assert_eq!(replay, first);
        assert_eq!(revocation.delete_calls.load(Ordering::SeqCst), 1);

        let restarted = broker_with_revocations(&base_url, Arc::clone(&store));
        let after_restart =
            TriggerConnectionManager::revoke_connection(&restarted, &subject(), "service-1")
                .await
                .expect("process restart reads the durable receipt");
        assert_eq!(after_restart, first);
        let listed = TriggerConnectionManager::list_connections(&restarted, &subject())
            .await
            .expect("successful live listing merges the durable inactive receipt");
        assert_eq!(listed, vec![first.clone()]);

        revocation
            .management_available
            .store(false, Ordering::SeqCst);
        assert!(
            TriggerConnectionManager::list_connections(&restarted, &subject())
                .await
                .is_err()
        );
        let replay_lookup = TriggerConnectionManager::connection_for_revocation(
            &restarted,
            &subject(),
            "service-1",
        )
        .await
        .expect("durable lookup remains available for operation replay");
        assert_eq!(replay_lookup, Some(first));
        assert!(
            TriggerConnectionManager::revoke_connection(&restarted, &subject(), "service-1",)
                .await
                .is_err()
        );
        server.abort();

        assert!(requests
            .lock()
            .await
            .iter()
            .all(|request| request.uri != "/api/v1/proxy/s/api-github/user"));
    }

    #[tokio::test]
    async fn lost_delete_response_reconciles_from_provider_observed_inactive_version() {
        let revocation = RevocationFixture::default();
        revocation
            .lose_delete_response_once
            .store(true, Ordering::SeqCst);
        let fixture = Fixture {
            revocation: Some(revocation.clone()),
            ..Fixture::default()
        };
        let (base_url, _requests, server) = fixture_server(fixture).await;
        let store = Arc::new(TestConnectionRevocationStore::default());
        let receipt = TriggerConnectionManager::revoke_connection(
            &broker_with_revocations(&base_url, store),
            &subject(),
            "service-1",
        )
        .await
        .expect("ambiguous DELETE is reconciled by exact authorization evidence");
        server.abort();

        assert_eq!(receipt.state, ConnectionState::Inactive);
        assert_eq!(receipt.state_version, 5);
        assert_eq!(revocation.delete_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn fencing_failure_after_provider_receipt_releases_the_latest_writer_revision() {
        let revocation = RevocationFixture::default();
        let fixture = Fixture {
            revocation: Some(revocation.clone()),
            ..Fixture::default()
        };
        let (base_url, _requests, server) = fixture_server(fixture).await;
        let store = Arc::new(TestConnectionRevocationStore::default());
        store.fail_fencing_once();
        let broker = broker_with_revocations(&base_url, Arc::clone(&store));

        TriggerConnectionManager::revoke_connection(&broker, &subject(), "service-1")
            .await
            .expect_err("the injected Launch fencing failure is surfaced");
        let recoverable = store
            .get_connection_revocation(&subject(), "service-1")
            .await
            .expect("read persisted revocation")
            .expect("revocation intent remains durable");
        assert!(recoverable.provider_receipt.is_some());
        assert!(recoverable.dispatch_lease.is_none());
        assert!(recoverable.settled_at.is_none());
        assert_eq!(revocation.delete_calls.load(Ordering::SeqCst), 1);

        let settled = TriggerConnectionManager::revoke_connection(&broker, &subject(), "service-1")
            .await
            .expect("retry resumes from the durable provider receipt");
        server.abort();

        assert_eq!(settled.state, ConnectionState::Inactive);
        assert_eq!(settled.state_version, 5);
        assert_eq!(revocation.delete_calls.load(Ordering::SeqCst), 1);
        assert_eq!(store.fenced(), 1);
    }

    #[tokio::test]
    async fn delete_not_found_race_requires_and_accepts_positive_inactive_evidence() {
        let revocation = RevocationFixture::default();
        revocation
            .delete_returns_not_found_once
            .store(true, Ordering::SeqCst);
        let fixture = Fixture {
            revocation: Some(revocation.clone()),
            ..Fixture::default()
        };
        let (base_url, _requests, server) = fixture_server(fixture).await;
        let receipt = TriggerConnectionManager::revoke_connection(
            &broker_with_revocations(
                &base_url,
                Arc::new(TestConnectionRevocationStore::default()),
            ),
            &subject(),
            "service-1",
        )
        .await
        .expect("404 race reconciles only through inactive evidence");
        server.abort();

        assert_eq!(receipt.state, ConnectionState::Inactive);
        assert_eq!(receipt.state_version, 5);
        assert_eq!(revocation.delete_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn reactivated_service_starts_a_new_revocation_epoch() {
        let revocation = RevocationFixture::default();
        let fixture = Fixture {
            revocation: Some(revocation.clone()),
            ..Fixture::default()
        };
        let (base_url, _requests, server) = fixture_server(fixture).await;
        let store = Arc::new(TestConnectionRevocationStore::default());
        let broker = broker_with_revocations(&base_url, Arc::clone(&store));
        let first = TriggerConnectionManager::revoke_connection(&broker, &subject(), "service-1")
            .await
            .expect("first epoch settles");
        assert_eq!(first.state_version, 5);

        revocation.reactivate();
        let current =
            TriggerConnectionManager::connection_for_revocation(&broker, &subject(), "service-1")
                .await
                .expect("lookup succeeds")
                .expect("reactivated service is visible");
        assert_eq!(current.state, ConnectionState::Active);
        assert_eq!(current.state_version, 6);

        let second = TriggerConnectionManager::revoke_connection(&broker, &subject(), "service-1")
            .await
            .expect("new provider epoch is revoked independently");
        server.abort();

        assert_eq!(second.state, ConnectionState::Inactive);
        assert_eq!(second.state_version, 7);
        assert_ne!(second.state_version, first.state_version);
        assert_eq!(revocation.delete_calls.load(Ordering::SeqCst), 2);
        assert_eq!(store.fenced(), 2);
    }

    #[tokio::test]
    async fn concurrent_connection_revokes_converge_on_one_provider_write() {
        let revocation = RevocationFixture::default();
        let fixture = Fixture {
            revocation: Some(revocation.clone()),
            ..Fixture::default()
        };
        let (base_url, _requests, server) = fixture_server(fixture).await;
        let store = Arc::new(TestConnectionRevocationStore::default());
        let left = broker_with_revocations(&base_url, Arc::clone(&store));
        let right = broker_with_revocations(&base_url, store);
        let left_subject = subject();
        let right_subject = subject();
        let (left, right) = tokio::join!(
            TriggerConnectionManager::revoke_connection(&left, &left_subject, "service-1"),
            TriggerConnectionManager::revoke_connection(&right, &right_subject, "service-1"),
        );
        server.abort();

        let left = left.expect("left revoke converges");
        let right = right.expect("right revoke converges");
        assert_eq!(left, right);
        assert_eq!(left.state, ConnectionState::Inactive);
        assert_eq!(revocation.delete_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn active_list_absence_and_unknown_service_never_fabricate_owner_proof() {
        let revocation = RevocationFixture::default();
        revocation.active.store(false, Ordering::SeqCst);
        let fixture = Fixture {
            revocation: Some(revocation.clone()),
            ..Fixture::default()
        };
        let (base_url, _requests, server) = fixture_server(fixture).await;
        let broker = broker_with_revocations(
            &base_url,
            Arc::new(TestConnectionRevocationStore::default()),
        );
        assert_eq!(
            TriggerConnectionManager::connection_for_revocation(&broker, &subject(), "service-1",)
                .await
                .expect("concealed lookup"),
            None
        );
        assert!(
            TriggerConnectionManager::revoke_connection(&broker, &subject(), "service-1",)
                .await
                .is_err()
        );
        assert_eq!(
            TriggerConnectionManager::connection_for_revocation(
                &broker,
                &subject(),
                "foreign-service",
            )
            .await
            .expect("foreign lookup is concealed"),
            None
        );
        server.abort();
        assert_eq!(revocation.delete_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn absent_subject_authority_prevents_any_outbound_request() {
        let (base_url, requests, server) = fixture_server(Fixture::default()).await;
        let result = broker(&base_url, false).list_connections(&subject()).await;
        server.abort();

        assert!(matches!(
            result,
            Err(GithubBrokerError::OwnerCredentialUnavailable)
        ));
        assert!(requests.lock().await.is_empty());
    }

    #[tokio::test]
    async fn listed_github_service_must_have_the_typed_resource_uri() {
        let fixture = Fixture {
            resource_uri: "https://nyxid.test/api/v1/proxy/s/not-github".to_string(),
            ..Fixture::default()
        };
        let (base_url, requests, server) = fixture_server(fixture).await;
        let result = broker(&base_url, true).list_connections(&subject()).await;
        server.abort();

        assert!(matches!(result, Err(GithubBrokerError::Malformed(_))));
        assert_eq!(requests.lock().await.len(), 1);
    }

    #[test]
    fn provider_urls_encode_data_and_reject_noncanonical_segments() {
        let base = Url::parse("https://nyxid.test/a/path?discard=yes#discard").unwrap();
        let url = endpoint(&base, &["api", "space here"], &[("key", "value /?")])
            .expect("build endpoint");
        assert_eq!(url.path(), "/api/space%20here");
        assert_eq!(
            url.query_pairs().collect::<Vec<_>>(),
            vec![("key".into(), "value /?".into())]
        );
        for bad in ["", ".", "..", "a/b", "a\\b"] {
            assert!(endpoint(&base, &[bad], &[]).is_err(), "accepted {bad:?}");
        }
        for bad_ref in ["", "/main", "main/", "feature//x", "../main", "main~1"] {
            assert!(
                repository_ref_components(bad_ref).is_err(),
                "accepted ref {bad_ref:?}"
            );
        }
        assert_eq!(
            repository_ref_components("feature/cma-trigger").unwrap(),
            vec!["feature", "cma-trigger"]
        );
    }

    #[test]
    fn repository_locators_are_single_bounded_provider_segments() {
        for valid in ["ChronoAIProject", "chrono-bot", ".github"] {
            assert!(validate_repository_locator(valid, "repository").is_ok());
        }
        for invalid in ["", ".", "..", "owner/repo", "owner?x", " owner", "repo\n"] {
            assert!(validate_repository_locator(invalid, "repository").is_err());
        }
    }

    #[test]
    fn github_oauth_url_is_pinned_to_the_expected_origin_and_state() {
        let valid =
            format!("https://github.com/login/oauth/authorize?client_id=test&state=1cc_{NONCE}");
        assert!(validate_github_authorization_url(&valid, NONCE).is_ok());
        for invalid in [
            format!("https://example.com/login/oauth/authorize?state=1cc_{NONCE}"),
            format!("https://github.com:8443/login/oauth/authorize?state=1cc_{NONCE}"),
            format!("https://github.com/login/oauth/authorize?state=wrong-{NONCE}"),
            format!("https://github.com/login/oauth/authorize?state=1cc_{NONCE}#fragment"),
        ] {
            assert!(
                validate_github_authorization_url(&invalid, NONCE).is_err(),
                "accepted {invalid}"
            );
        }
    }
}
