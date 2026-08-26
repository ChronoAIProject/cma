//! Typed NyxID and GitHub authority calls for CMA Trigger.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::SystemTime;

use async_trait::async_trait;
use layer2_core::{
    AuthorityProvenance, ConnectedAccount, ConnectionSnapshot, ConnectionState, DelegationState,
    LaunchAuthority, PublisherAuthority, PublisherRepositoryRole, RepositoryIdentity,
    RepositoryVisibility, ScopeClass, SourceMode, SubjectId, TriggerAuthorityVerifier,
    TriggerConnection, TriggerConnectionAuthorization, TriggerConnectionManager,
    TriggerRepositorySelection, TriggerRevision, TriggerRevisionContent, TriggerServiceError,
};
use reqwest::{Method, StatusCode, Url};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::auth::AuthState;

const GITHUB_SERVICE_SLUG: &str = "api-github";
const GITHUB_PROVIDER_ID: &str = "github";
const PUBLIC_REPOSITORY_SCOPE: &str = "public_repo";
const MAX_MANAGEMENT_BODY_BYTES: usize = 2 * 1024 * 1024;
const MAX_GITHUB_BODY_BYTES: usize = 32 * 1024 * 1024;

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
    #[error("the provider rejected the typed request with HTTP {status}")]
    Rejected { status: u16 },
    #[error("provider data is malformed: {0}")]
    Malformed(String),
}

impl From<GithubBrokerError> for TriggerServiceError {
    fn from(error: GithubBrokerError) -> Self {
        TriggerServiceError::Authority(error.to_string())
    }
}

#[derive(Clone)]
pub(crate) struct GithubBroker {
    base_url: Url,
    http: reqwest::Client,
    authority: Arc<dyn SubjectAuthoritySource>,
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

#[derive(Debug, Clone, Copy, Default, Deserialize)]
pub(crate) struct RepositoryPermissions {
    #[serde(default)]
    pub admin: bool,
    #[serde(default)]
    pub maintain: bool,
    #[serde(default)]
    pub push: bool,
    #[serde(default)]
    pub triage: bool,
    #[serde(default)]
    pub pull: bool,
}

impl GithubBroker {
    pub(crate) fn new(base_url: &str, auth: Arc<AuthState>) -> Result<Self, GithubBrokerError> {
        Self::with_authority(base_url, auth)
    }

    fn with_authority(
        base_url: &str,
        authority: Arc<dyn SubjectAuthoritySource>,
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
        let visibility = match response.value.visibility.as_deref() {
            Some("public") => RepositoryVisibility::Public,
            Some("private") => RepositoryVisibility::Private,
            Some("internal") => RepositoryVisibility::Internal,
            None if response.value.private => RepositoryVisibility::Private,
            None => RepositoryVisibility::Public,
            Some(_) => {
                return Err(GithubBrokerError::Malformed(
                    "GitHub returned an unknown repository visibility".into(),
                ))
            }
        };
        let allow_forking = response
            .value
            .allow_forking
            .unwrap_or(visibility == RepositoryVisibility::Public);
        let identity = RepositoryIdentity {
            repository_id: response.value.id,
            repository_node_id: required(response.value.node_id, "repository node ID")?,
            owner_id: response.value.owner.id,
            owner_node_id: required(response.value.owner.node_id, "owner node ID")?,
            canonical_owner: required(response.value.owner.login, "repository owner login")?,
            canonical_name: required(response.value.name, "repository name")?,
            visibility,
            allow_forking,
            default_branch: required(response.value.default_branch, "default branch")?,
            selected_ref: String::new(),
            fork_parent_id: response.value.parent.as_ref().map(|parent| parent.id),
            fork_source_id: response.value.source.as_ref().map(|source| source.id),
        };
        Ok(RepositoryEvidence {
            identity,
            permissions: response.value.permissions.unwrap_or_default(),
            archived: response.value.archived,
            disabled: response.value.disabled,
            html_url: required(response.value.html_url, "repository URL")?,
            subject_token_generation: response.token_generation,
        })
    }

    async fn resolve_branch_sha(
        &self,
        subject: &SubjectId,
        service_id: &str,
        repository_id: u64,
        selected_ref: &str,
    ) -> Result<String, GithubBrokerError> {
        let components = repository_ref_components(selected_ref)?;
        let repository_id = repository_id.to_string();
        let mut path = vec![
            "repositories",
            repository_id.as_str(),
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
            .request(method, url)
            .bearer_auth(&authority.bearer)
            .header("accept", "application/vnd.github+json")
            .header("x-github-api-version", "2022-11-28");
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request
            .send()
            .await
            .map_err(|_| GithubBrokerError::Transient { status: None })?;
        let status = response.status();
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|value| value.to_str().ok())
            .filter(|value| value.len() <= 128)
            .map(str::to_string);
        if !status.is_success() {
            // Drain only a small bounded body so the connection can be reused;
            // provider text is never copied into Launch state or logs.
            let _ = bounded_body(response, 16 * 1024).await;
            return Err(classify_status(status, retry_after));
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
        let observed_head = self
            .resolve_branch_sha(
                subject,
                selected_service_id,
                repository.identity.repository_id,
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
        if previous.is_some_and(|previous| {
            previous.service_id != connection.snapshot.service_id
                || previous.service_state_version != connection.snapshot.service_state_version
                || previous.backing_key_id != connection.snapshot.backing_key_id
                || previous.account.account_id != connection.snapshot.account.account_id
                || previous.account.account_node_id != connection.snapshot.account.account_node_id
                || normalized_set(&previous.granted_scopes)
                    != normalized_set(&connection.snapshot.granted_scopes)
        }) {
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
        let observed_head = self
            .resolve_branch_sha(
                subject,
                selected_service_id,
                repository.identity.repository_id,
                &repository.identity.selected_ref,
            )
            .await?;
        if repository.identity != revision.content.upstream.repository
            || repository.archived
            || repository.disabled
        {
            return Err(GithubBrokerError::AuthorityDrift.into());
        }
        Ok(LaunchAuthority {
            provenance: AuthorityProvenance {
                subject: subject.clone(),
                subject_token_generation: repository.subject_token_generation,
                service_id: connection.snapshot.service_id.clone(),
                service_state_version: connection.snapshot.service_state_version,
                backing_key_id: connection.snapshot.backing_key_id.clone(),
                session_credential_generation: None,
                state: DelegationState::Available,
                observed_at: layer2_core::rfc3339_at(SystemTime::now()),
            },
            connection: connection.snapshot,
            source_sha: match revision.content.upstream.source_mode {
                SourceMode::PinnedCommit => revision
                    .content
                    .upstream
                    .observed_source_sha
                    .clone()
                    .ok_or_else(|| {
                    GithubBrokerError::Malformed("the pinned Trigger has no source commit".into())
                })?,
                SourceMode::BranchHeadAtLaunch => observed_head,
            },
        })
    }
}

#[async_trait]
impl TriggerConnectionManager for GithubBroker {
    async fn list_connections(
        &self,
        subject: &SubjectId,
    ) -> Result<Vec<TriggerConnection>, TriggerServiceError> {
        Ok(GithubBroker::list_connections(self, subject)
            .await?
            .into_iter()
            .map(|connection| TriggerConnection {
                service_id: connection.service_id,
                state_version: connection.state_version,
                state: connection.state,
                account: connection.account,
                granted_scopes: connection.granted_scopes,
                consent_observed_at: connection.consent_observed_at,
            })
            .collect())
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
            .resolve_branch_sha(subject, service_id, repository_id, selected_ref)
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

fn classify_status(status: StatusCode, retry_after: Option<String>) -> GithubBrokerError {
    match status {
        StatusCode::UNAUTHORIZED => GithubBrokerError::OwnerCredentialUnavailable,
        StatusCode::FORBIDDEN => GithubBrokerError::Forbidden,
        StatusCode::NOT_FOUND => GithubBrokerError::NotFound,
        StatusCode::TOO_MANY_REQUESTS => GithubBrokerError::RateLimited { retry_after },
        status if status.is_server_error() => GithubBrokerError::Transient {
            status: Some(status.as_u16()),
        },
        status => GithubBrokerError::Rejected {
            status: status.as_u16(),
        },
    }
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
    use std::sync::Arc;

    use axum::body::to_bytes;
    use axum::extract::{Request, State};
    use axum::http::header::AUTHORIZATION;
    use axum::response::IntoResponse;
    use axum::routing::any;
    use axum::{Json, Router};
    use serde_json::{json, Value};
    use tokio::sync::Mutex;

    use super::*;

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

    #[derive(Clone)]
    struct Fixture {
        requests: Arc<Mutex<Vec<RecordedRequest>>>,
        service_state_version: i64,
        authorization_state_version: i64,
        scopes: Vec<String>,
        resource_uri: String,
        label: Option<String>,
    }

    impl Default for Fixture {
        fn default() -> Self {
            Self {
                requests: Arc::new(Mutex::new(Vec::new())),
                service_state_version: 4,
                authorization_state_version: 4,
                scopes: vec![PUBLIC_REPOSITORY_SCOPE.to_string()],
                resource_uri: "https://nyxid.test/api/v1/proxy/s/api-github".to_string(),
                label: None,
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

        let response = match (method.as_str(), parts.uri.path()) {
            ("GET", "/api/v1/user-services") => json!({
                "services": [{
                    "id": "service-1",
                    "slug": GITHUB_SERVICE_SLUG,
                    "label": fixture.label,
                    "resource_uri": fixture.resource_uri,
                    "api_key_id": "key-1",
                    "is_active": true,
                    "state_version": fixture.service_state_version,
                    "credential_source": { "type": "personal" }
                }]
            }),
            ("GET", "/api/v1/keys/service-1/authorization") => json!({
                "id": "service-1",
                "api_key_id": "key-1",
                "is_active": true,
                "status": "active",
                "connection_status": "active",
                "granted_scopes": fixture.scopes,
                "last_authorized_at": "2026-08-26T01:02:03Z",
                "state_version": fixture.authorization_state_version
            }),
            ("GET", "/api/v1/proxy/s/api-github/user") => json!({
                "id": 21,
                "node_id": "U_kgDOBQ",
                "login": "contributor",
                "avatar_url": "https://avatars.githubusercontent.com/u/21"
            }),
            ("GET", "/api/v1/proxy/s/api-github/repositories/10") => json!({
                "id": 10,
                "node_id": "R_kgDOAA",
                "name": "upstream",
                "owner": { "id": 11, "node_id": "U_kgDOAQ", "login": "owner" },
                "private": false,
                "visibility": "public",
                "default_branch": "main",
                "allow_forking": true,
                "archived": false,
                "disabled": false,
                "html_url": "https://github.com/owner/upstream",
                "permissions": { "pull": true }
            }),
            ("GET", "/api/v1/proxy/s/api-github/repositories/10/git/ref/heads/main") => {
                json!({
                    "ref": "refs/heads/main",
                    "object": {
                        "sha": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                        "type": "commit"
                    }
                })
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

    fn subject() -> SubjectId {
        SubjectId("subject-1".to_string())
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
            "/api/v1/proxy/s/api-github/repositories/10/git/ref/heads/main?_nyxid_via=service-1"
        );
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
