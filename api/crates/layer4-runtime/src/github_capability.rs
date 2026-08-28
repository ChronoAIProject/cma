//! Signed, launch-scoped GitHub policy enforced above credential injection.
//!
//! This module deliberately duplicates the wire vocabulary minted by the
//! control plane. Layer 4 is a separately deployed program and cannot depend
//! on a control-plane crate; the tests on both sides pin the protocol instead.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use axum::http::{Method, Uri};
use base64::Engine as _;
use ring::signature;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex, RwLock};

pub const POLICY_VERSION: u32 = 2;
pub const POLICY_AUDIENCE: &str = "cma-layer4-github-capability-v2";
pub const MAX_LIFETIME_SECONDS: u64 = 60;
pub const MAX_CLOCK_SKEW_SECONDS: u64 = 5;
pub const MAX_ENVELOPE_BYTES: usize = 96 * 1024;
pub const MAX_PAYLOAD_BYTES: usize = 64 * 1024;
pub const MAX_PATH_BYTES: usize = 2 * 1024;
pub const MAX_QUERY_BYTES: usize = 2 * 1024;
pub const MAX_QUERY_FIELDS: usize = 8;
pub const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;
pub const GITHUB_SERVICE_SLUG: &str = "api-github";

pub(crate) const AUDIT_EVENT_NAME: &str = "cma_trigger_github_capability_decision";
pub(crate) const AUDIT_SCHEMA_VERSION: u64 = 1;
pub(crate) const AUDIT_ALLOWED_CODE: &str = "allowed";
pub(crate) const AUDIT_UNCLASSIFIED_OPERATION: &str = "unclassified";

const SIGNING_DOMAIN: &[u8] = b"CMA_GITHUB_CAPABILITY_V2\0";
const PLATFORM_PROXY_PREFIX: &[&str] = &["api", "v1", "proxy"];
// Trigger production grants only these non-GitHub baseline services. Keeping
// the list closed prevents an opaque UserService ID in SESSION_REACHES from
// becoming an accidental slug-route bypass before policy installation.
const TRIGGER_PASSTHROUGH_SERVICE_SLUGS: &[&str] = &["chrono-llm", "ornn-api", "ornn"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GithubOperationClass {
    RepositoryRead,
    TreeRead,
    ObjectCreate,
    LaunchRefCreate,
    LaunchRefUpdate,
    StatusRead,
}

impl GithubOperationClass {
    pub const ALL: [Self; 6] = [
        Self::RepositoryRead,
        Self::TreeRead,
        Self::ObjectCreate,
        Self::LaunchRefCreate,
        Self::LaunchRefUpdate,
        Self::StatusRead,
    ];

    pub fn wire_name(self) -> &'static str {
        match self {
            Self::RepositoryRead => "repository_read",
            Self::TreeRead => "tree_read",
            Self::ObjectCreate => "object_create",
            Self::LaunchRefCreate => "launch_ref_create",
            Self::LaunchRefUpdate => "launch_ref_update",
            Self::StatusRead => "status_read",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityRefusal {
    MissingPolicy,
    UnknownVersion,
    Expired,
    StaleGeneration,
    Revoked,
    MalformedRequest,
    ResourceMismatch,
    OperationDenied,
    AudienceMismatch,
    SignatureInvalid,
}

impl CapabilityRefusal {
    pub fn wire_name(self) -> &'static str {
        match self {
            Self::MissingPolicy => "missing_policy",
            Self::UnknownVersion => "unknown_version",
            Self::Expired => "expired",
            Self::StaleGeneration => "stale_generation",
            Self::Revoked => "revoked",
            Self::MalformedRequest => "malformed_request",
            Self::ResourceMismatch => "resource_mismatch",
            Self::OperationDenied => "operation_denied",
            Self::AudienceMismatch => "audience_mismatch",
            Self::SignatureInvalid => "signature_invalid",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SignedEnvelope {
    version: u32,
    key_id: String,
    payload: String,
    signature: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RepositoryBinding {
    repository_id: u64,
    repository_node_id: String,
    owner_id: u64,
    owner_node_id: String,
    canonical_owner: String,
    canonical_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    fork_parent_id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    fork_source_id: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum CapabilityDocument {
    Policy {
        version: u32,
        launch_id: String,
        session_id: String,
        runtime_audience: String,
        service_id: String,
        catalog_service_id: String,
        user_service_slug: String,
        upstream: Box<RepositoryBinding>,
        fork: Box<RepositoryBinding>,
        allowed_ref: String,
        github_operation_classes: Vec<GithubOperationClass>,
        generation: u64,
        fence: u64,
        not_before_epoch_seconds: u64,
        expires_at_epoch_seconds: u64,
        issuer: String,
        audience: String,
        signature_key_id: String,
        numeric_binding_observed_at: String,
    },
    Tombstone {
        version: u32,
        launch_id: String,
        session_id: String,
        runtime_audience: String,
        generation: u64,
        fence: u64,
        issued_at_epoch_seconds: u64,
        issuer: String,
        audience: String,
        signature_key_id: String,
    },
}

impl CapabilityDocument {
    fn version(&self) -> u32 {
        match self {
            Self::Policy { version, .. } | Self::Tombstone { version, .. } => *version,
        }
    }

    fn generation_and_fence(&self) -> (u64, u64) {
        match self {
            Self::Policy {
                generation, fence, ..
            }
            | Self::Tombstone {
                generation, fence, ..
            } => (*generation, *fence),
        }
    }

    fn session_and_runtime(&self) -> (&str, &str) {
        match self {
            Self::Policy {
                session_id,
                runtime_audience,
                ..
            }
            | Self::Tombstone {
                session_id,
                runtime_audience,
                ..
            } => (session_id, runtime_audience),
        }
    }

    fn audience(&self) -> &str {
        match self {
            Self::Policy { audience, .. } | Self::Tombstone { audience, .. } => audience,
        }
    }

    fn signature_key_id(&self) -> &str {
        match self {
            Self::Policy {
                signature_key_id, ..
            }
            | Self::Tombstone {
                signature_key_id, ..
            } => signature_key_id,
        }
    }

    fn launch_id(&self) -> &str {
        match self {
            Self::Policy { launch_id, .. } | Self::Tombstone { launch_id, .. } => launch_id,
        }
    }

    fn is_revoked(&self) -> bool {
        matches!(self, Self::Tombstone { .. })
    }
}

#[derive(Clone)]
struct InstalledDocument {
    document: CapabilityDocument,
    digest: [u8; 32],
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilitySnapshot {
    pub required: bool,
    pub supported: bool,
    pub installed: bool,
    pub revoked: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub launch_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generation: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fence: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at_epoch_seconds: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_refusal: Option<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallationReceipt {
    pub generation: u64,
    pub fence: u64,
    pub revoked: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalGithubRequest {
    pub operation: GithubOperationClass,
    pub path_and_query: String,
    pub body: Vec<u8>,
}

#[derive(Debug, Clone, Copy, Default)]
struct CapabilityAuditResources {
    upstream_repository_id: Option<u64>,
    upstream_owner_id: Option<u64>,
    fork_repository_id: Option<u64>,
    fork_owner_id: Option<u64>,
}

impl CapabilityAuditResources {
    fn from_document(document: &CapabilityDocument) -> Self {
        match document {
            CapabilityDocument::Policy { upstream, fork, .. } => Self {
                upstream_repository_id: Some(upstream.repository_id),
                upstream_owner_id: Some(upstream.owner_id),
                fork_repository_id: Some(fork.repository_id),
                fork_owner_id: Some(fork.owner_id),
            },
            CapabilityDocument::Tombstone { .. } => Self::default(),
        }
    }
}

/// Per-runtime evaluator. Public keys are immutable; policy replacement is one
/// atomic write under a monotonic generation/fence comparison.
#[derive(Clone)]
pub struct GithubCapabilityGate {
    required: bool,
    expected_session_id: String,
    expected_launch_id: String,
    passthrough_service_slugs: Arc<BTreeSet<String>>,
    verification_keys: Arc<BTreeMap<String, Vec<u8>>>,
    installed: Arc<RwLock<Option<InstalledDocument>>>,
    last_refusal: Arc<Mutex<Option<CapabilityRefusal>>>,
}

impl GithubCapabilityGate {
    pub fn from_configuration(
        required: bool,
        expected_session_id: impl Into<String>,
        reaches: impl IntoIterator<Item = String>,
        verification_keys_json: &str,
    ) -> Result<Self, String> {
        let expected_session_id = expected_session_id.into();
        let expected_launch_id = match required {
            true => launch_id_from_trigger_session(&expected_session_id).ok_or_else(|| {
                "a capability-required runtime needs a deterministic Trigger session ID".to_string()
            })?,
            false => String::new(),
        };
        let mut verification_keys = BTreeMap::new();
        if !verification_keys_json.trim().is_empty() {
            let encoded: BTreeMap<String, String> = serde_json::from_str(verification_keys_json)
                .map_err(|_| "CHRONO_GITHUB_CAPABILITY_VERIFY_KEYS must be a JSON object")?;
            if encoded.is_empty() || encoded.len() > 8 {
                return Err(
                    "the GitHub capability verification keyring must contain 1 to 8 keys".into(),
                );
            }
            for (key_id, value) in encoded {
                if !usable_identifier(&key_id, 128) {
                    return Err("a GitHub capability verification key ID is not usable".into());
                }
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(value)
                    .map_err(|_| "a GitHub capability verification key is not base64")?;
                if bytes.len() != 32 {
                    return Err("an Ed25519 verification key must contain exactly 32 bytes".into());
                }
                verification_keys.insert(key_id, bytes);
            }
        }
        if required && verification_keys.is_empty() {
            return Err("a capability-required runtime has no verification key".into());
        }

        let passthrough_service_slugs = reaches
            .into_iter()
            .filter(|slug| !required || TRIGGER_PASSTHROUGH_SERVICE_SLUGS.contains(&slug.as_str()))
            .filter(|slug| usable_slug(slug))
            .collect();
        Ok(Self {
            required,
            expected_session_id,
            expected_launch_id,
            passthrough_service_slugs: Arc::new(passthrough_service_slugs),
            verification_keys: Arc::new(verification_keys),
            installed: Arc::new(RwLock::new(None)),
            last_refusal: Arc::new(Mutex::new(None)),
        })
    }

    #[cfg(test)]
    pub(crate) fn disabled() -> Self {
        Self::from_configuration(false, "", Vec::new(), "").expect("disabled gate")
    }

    pub fn supported(&self) -> bool {
        !self.verification_keys.is_empty()
    }

    pub fn required(&self) -> bool {
        self.required
    }

    pub async fn install(
        &self,
        runtime_audience: &str,
        envelope_json: &str,
        now_epoch_seconds: u64,
    ) -> Result<InstallationReceipt, CapabilityRefusal> {
        let (document, digest) = match self.verify(envelope_json) {
            Ok(verified) => verified,
            Err(refusal) => return self.refuse(refusal).await,
        };
        if let Err(refusal) = self.validate_document(&document, runtime_audience, now_epoch_seconds)
        {
            return self.refuse(refusal).await;
        }
        let incoming_order = document.generation_and_fence();

        let mut installed = self.installed.write().await;
        if let Some(current) = installed.as_ref() {
            let current_order = current.document.generation_and_fence();
            if incoming_order < current_order
                || (incoming_order == current_order && digest != current.digest)
            {
                return self.refuse(CapabilityRefusal::StaleGeneration).await;
            }
            if incoming_order == current_order {
                return Ok(InstallationReceipt {
                    generation: incoming_order.0,
                    fence: incoming_order.1,
                    revoked: current.document.is_revoked(),
                });
            }
        }
        let revoked = document.is_revoked();
        *installed = Some(InstalledDocument { document, digest });
        drop(installed);
        *self.last_refusal.lock().await = None;
        Ok(InstallationReceipt {
            generation: incoming_order.0,
            fence: incoming_order.1,
            revoked,
        })
    }

    pub async fn evaluate(
        &self,
        method: &Method,
        uri: &Uri,
        body: &[u8],
        now_epoch_seconds: u64,
    ) -> Result<Option<CanonicalGithubRequest>, CapabilityRefusal> {
        if !self.required {
            return Ok(None);
        }
        let platform_route =
            match classify_platform_route(uri.path(), &self.passthrough_service_slugs) {
                Ok(route) => route,
                Err(refusal) => {
                    return self
                        .refuse_request(refusal, None, self.current_audit_resources().await)
                        .await;
                }
            };
        let installed = self.installed.read().await.clone();
        if let PlatformRoute::Passthrough { slug } = &platform_route {
            let is_signed_github_selector = installed.as_ref().is_some_and(|installed| {
                matches!(
                    &installed.document,
                    CapabilityDocument::Policy {
                        service_id,
                        catalog_service_id,
                        user_service_slug,
                        ..
                    } if service_id == slug
                        || catalog_service_id == slug
                        || user_service_slug == slug
                )
            });
            if !is_signed_github_selector {
                return Ok(None);
            }
            return self
                .refuse_request(
                    CapabilityRefusal::OperationDenied,
                    None,
                    installed
                        .as_ref()
                        .map(|installed| {
                            CapabilityAuditResources::from_document(&installed.document)
                        })
                        .unwrap_or_default(),
                )
                .await;
        }
        let PlatformRoute::Github {
            catalog_service_id: requested_catalog_service_id,
            path: route,
        } = platform_route
        else {
            unreachable!("passthrough routes return above")
        };
        let Some(installed) = installed.as_ref() else {
            return self
                .refuse_request(
                    CapabilityRefusal::MissingPolicy,
                    None,
                    CapabilityAuditResources::default(),
                )
                .await;
        };
        let audit_resources = CapabilityAuditResources::from_document(&installed.document);
        let CapabilityDocument::Policy {
            service_id,
            catalog_service_id,
            fork,
            allowed_ref,
            github_operation_classes,
            not_before_epoch_seconds,
            expires_at_epoch_seconds,
            ..
        } = &installed.document
        else {
            return self
                .refuse_request(CapabilityRefusal::Revoked, None, audit_resources)
                .await;
        };
        if requested_catalog_service_id != *catalog_service_id {
            return self
                .refuse_request(CapabilityRefusal::ResourceMismatch, None, audit_resources)
                .await;
        }
        if now_epoch_seconds.saturating_add(MAX_CLOCK_SKEW_SECONDS) < *not_before_epoch_seconds
            || now_epoch_seconds > expires_at_epoch_seconds.saturating_add(MAX_CLOCK_SKEW_SECONDS)
        {
            return self
                .refuse_request(CapabilityRefusal::Expired, None, audit_resources)
                .await;
        }
        let query = match parse_query(uri.query()) {
            Ok(query) => query,
            Err(refusal) => {
                return self.refuse_request(refusal, None, audit_resources).await;
            }
        };
        if query.get("_nyxid_via").map(String::as_str) != Some(service_id.as_str()) {
            return self
                .refuse_request(CapabilityRefusal::ResourceMismatch, None, audit_resources)
                .await;
        }
        if body.len() > MAX_BODY_BYTES {
            return self
                .refuse_request(CapabilityRefusal::MalformedRequest, None, audit_resources)
                .await;
        }
        let allowed: BTreeSet<_> = github_operation_classes.iter().copied().collect();
        let canonical = match validate_github_request(
            method,
            &route,
            query,
            body,
            GithubRequestAuthority {
                fork,
                allowed_ref,
                service_id,
                catalog_service_id,
            },
        ) {
            Ok(request) if allowed.contains(&request.operation) => request,
            Ok(request) => {
                return self
                    .refuse_request(
                        CapabilityRefusal::OperationDenied,
                        Some(request.operation),
                        audit_resources,
                    )
                    .await;
            }
            Err(refusal) => {
                return self.refuse_request(refusal, None, audit_resources).await;
            }
        };
        self.audit_allowed(canonical.operation, audit_resources);
        Ok(Some(canonical))
    }

    /// Record a body failure that occurred before the normal policy evaluator
    /// could run. Entitled non-GitHub services remain outside this audit stream.
    pub async fn refuse_unreadable_body(&self, uri: &Uri) -> Option<CapabilityRefusal> {
        if !self.required
            || matches!(
                classify_platform_route(uri.path(), &self.passthrough_service_slugs),
                Ok(PlatformRoute::Passthrough { .. })
            )
        {
            return None;
        }
        let refusal = CapabilityRefusal::MalformedRequest;
        self.record_request_refusal(refusal, None, self.current_audit_resources().await)
            .await;
        Some(refusal)
    }

    pub async fn snapshot(&self) -> CapabilitySnapshot {
        let installed = self.installed.read().await;
        let last_refusal = *self.last_refusal.lock().await;
        let Some(value) = installed.as_ref() else {
            return CapabilitySnapshot {
                required: self.required,
                supported: self.supported(),
                installed: false,
                revoked: false,
                launch_id: None,
                generation: None,
                fence: None,
                expires_at_epoch_seconds: None,
                last_refusal: last_refusal.map(CapabilityRefusal::wire_name),
            };
        };
        let (generation, fence) = value.document.generation_and_fence();
        let expires_at_epoch_seconds = match &value.document {
            CapabilityDocument::Policy {
                expires_at_epoch_seconds,
                ..
            } => Some(*expires_at_epoch_seconds),
            CapabilityDocument::Tombstone { .. } => None,
        };
        CapabilitySnapshot {
            required: self.required,
            supported: self.supported(),
            installed: true,
            revoked: value.document.is_revoked(),
            launch_id: Some(value.document.launch_id().to_string()),
            generation: Some(generation),
            fence: Some(fence),
            expires_at_epoch_seconds,
            last_refusal: last_refusal.map(CapabilityRefusal::wire_name),
        }
    }

    pub async fn matches_probe_binding(
        &self,
        service_id: &str,
        catalog_service_id: &str,
        user_service_slug: &str,
        repository_id: u64,
        now_epoch_seconds: u64,
    ) -> bool {
        if !self.required {
            return false;
        }
        let installed = self.installed.read().await;
        matches!(
            installed.as_ref().map(|installed| &installed.document),
            Some(CapabilityDocument::Policy {
                service_id: signed_service_id,
                catalog_service_id: signed_catalog_service_id,
                user_service_slug: signed_user_service_slug,
                fork,
                not_before_epoch_seconds,
                expires_at_epoch_seconds,
                ..
            }) if signed_service_id == service_id
                && signed_catalog_service_id == catalog_service_id
                && signed_user_service_slug == user_service_slug
                && fork.repository_id == repository_id
                && now_epoch_seconds.saturating_add(MAX_CLOCK_SKEW_SECONDS)
                    >= *not_before_epoch_seconds
                && now_epoch_seconds
                    <= expires_at_epoch_seconds.saturating_add(MAX_CLOCK_SKEW_SECONDS)
        )
    }

    fn verify(
        &self,
        envelope_json: &str,
    ) -> Result<(CapabilityDocument, [u8; 32]), CapabilityRefusal> {
        if envelope_json.is_empty() || envelope_json.len() > MAX_ENVELOPE_BYTES {
            return Err(CapabilityRefusal::SignatureInvalid);
        }
        let envelope: SignedEnvelope =
            serde_json::from_str(envelope_json).map_err(|_| CapabilityRefusal::SignatureInvalid)?;
        if envelope.version != POLICY_VERSION || !usable_identifier(&envelope.key_id, 128) {
            return Err(CapabilityRefusal::UnknownVersion);
        }
        let key = self
            .verification_keys
            .get(&envelope.key_id)
            .ok_or(CapabilityRefusal::SignatureInvalid)?;
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(&envelope.payload)
            .map_err(|_| CapabilityRefusal::SignatureInvalid)?;
        let signature_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(&envelope.signature)
            .map_err(|_| CapabilityRefusal::SignatureInvalid)?;
        if payload.is_empty() || payload.len() > MAX_PAYLOAD_BYTES || signature_bytes.len() != 64 {
            return Err(CapabilityRefusal::SignatureInvalid);
        }
        let mut message = Vec::with_capacity(SIGNING_DOMAIN.len() + payload.len());
        message.extend_from_slice(SIGNING_DOMAIN);
        message.extend_from_slice(&payload);
        signature::UnparsedPublicKey::new(&signature::ED25519, key)
            .verify(&message, &signature_bytes)
            .map_err(|_| CapabilityRefusal::SignatureInvalid)?;
        let document: CapabilityDocument =
            serde_json::from_slice(&payload).map_err(|_| CapabilityRefusal::UnknownVersion)?;
        if document.signature_key_id() != envelope.key_id {
            return Err(CapabilityRefusal::SignatureInvalid);
        }
        let digest: [u8; 32] = Sha256::digest(&payload).into();
        Ok((document, digest))
    }

    fn validate_document(
        &self,
        document: &CapabilityDocument,
        runtime_audience: &str,
        now_epoch_seconds: u64,
    ) -> Result<(), CapabilityRefusal> {
        if document.version() != POLICY_VERSION {
            return Err(CapabilityRefusal::UnknownVersion);
        }
        let (generation, fence) = document.generation_and_fence();
        if generation == 0 || fence == 0 {
            return Err(CapabilityRefusal::StaleGeneration);
        }
        let (session_id, audience) = document.session_and_runtime();
        if session_id != self.expected_session_id
            || document.launch_id() != self.expected_launch_id
            || audience != runtime_audience
            || document.audience() != POLICY_AUDIENCE
        {
            return Err(CapabilityRefusal::AudienceMismatch);
        }
        match document {
            CapabilityDocument::Policy {
                launch_id,
                service_id,
                catalog_service_id,
                user_service_slug,
                upstream,
                fork,
                allowed_ref,
                github_operation_classes,
                not_before_epoch_seconds,
                expires_at_epoch_seconds,
                issuer,
                numeric_binding_observed_at,
                ..
            } => {
                if !usable_identifier(launch_id, 128)
                    || !usable_identifier(service_id, 256)
                    || !usable_identifier(catalog_service_id, 256)
                    || catalog_service_id == "s"
                    || !usable_user_service_slug(user_service_slug)
                    || !usable_identifier(issuer, 256)
                    || numeric_binding_observed_at.is_empty()
                    || !valid_head_ref(allowed_ref)
                    || !valid_repository_binding(upstream, false)
                    || !valid_repository_binding(fork, true)
                    || fork.fork_parent_id != Some(upstream.repository_id)
                    || fork.fork_source_id
                        != Some(upstream.fork_source_id.unwrap_or(upstream.repository_id))
                    || github_operation_classes.is_empty()
                    || github_operation_classes.len() > GithubOperationClass::ALL.len()
                    || github_operation_classes
                        .iter()
                        .copied()
                        .collect::<BTreeSet<_>>()
                        .len()
                        != github_operation_classes.len()
                {
                    return Err(CapabilityRefusal::ResourceMismatch);
                }
                let lifetime = expires_at_epoch_seconds
                    .checked_sub(*not_before_epoch_seconds)
                    .filter(|lifetime| *lifetime > 0 && *lifetime <= MAX_LIFETIME_SECONDS)
                    .ok_or(CapabilityRefusal::Expired)?;
                let _ = lifetime;
                if now_epoch_seconds.saturating_add(MAX_CLOCK_SKEW_SECONDS)
                    < *not_before_epoch_seconds
                    || now_epoch_seconds
                        > expires_at_epoch_seconds.saturating_add(MAX_CLOCK_SKEW_SECONDS)
                {
                    return Err(CapabilityRefusal::Expired);
                }
            }
            CapabilityDocument::Tombstone {
                launch_id,
                issued_at_epoch_seconds,
                issuer,
                ..
            } => {
                if !usable_identifier(launch_id, 128)
                    || !usable_identifier(issuer, 256)
                    || *issued_at_epoch_seconds
                        > now_epoch_seconds.saturating_add(MAX_CLOCK_SKEW_SECONDS)
                {
                    return Err(CapabilityRefusal::AudienceMismatch);
                }
            }
        }
        Ok(())
    }

    async fn refuse<T>(&self, refusal: CapabilityRefusal) -> Result<T, CapabilityRefusal> {
        *self.last_refusal.lock().await = Some(refusal);
        Err(refusal)
    }

    async fn current_audit_resources(&self) -> CapabilityAuditResources {
        self.installed
            .read()
            .await
            .as_ref()
            .map(|installed| CapabilityAuditResources::from_document(&installed.document))
            .unwrap_or_default()
    }

    async fn refuse_request<T>(
        &self,
        refusal: CapabilityRefusal,
        operation: Option<GithubOperationClass>,
        resources: CapabilityAuditResources,
    ) -> Result<T, CapabilityRefusal> {
        self.record_request_refusal(refusal, operation, resources)
            .await;
        Err(refusal)
    }

    async fn record_request_refusal(
        &self,
        refusal: CapabilityRefusal,
        operation: Option<GithubOperationClass>,
        resources: CapabilityAuditResources,
    ) {
        *self.last_refusal.lock().await = Some(refusal);
        let operation = operation.map_or(AUDIT_UNCLASSIFIED_OPERATION, |value| value.wire_name());
        tracing::warn!(
            target: "cma_trigger::github_capability_audit",
            event_name = AUDIT_EVENT_NAME,
            schema_version = AUDIT_SCHEMA_VERSION,
            session_id = %self.expected_session_id,
            launch_id = %self.expected_launch_id,
            operation_class = operation,
            decision = "deny",
            decision_code = refusal.wire_name(),
            upstream_repository_id = resources.upstream_repository_id,
            upstream_owner_id = resources.upstream_owner_id,
            fork_repository_id = resources.fork_repository_id,
            fork_owner_id = resources.fork_owner_id,
            "CMA Trigger GitHub capability decision"
        );
    }

    fn audit_allowed(&self, operation: GithubOperationClass, resources: CapabilityAuditResources) {
        tracing::info!(
            target: "cma_trigger::github_capability_audit",
            event_name = AUDIT_EVENT_NAME,
            schema_version = AUDIT_SCHEMA_VERSION,
            session_id = %self.expected_session_id,
            launch_id = %self.expected_launch_id,
            operation_class = operation.wire_name(),
            decision = "allow",
            decision_code = AUDIT_ALLOWED_CODE,
            upstream_repository_id = resources.upstream_repository_id,
            upstream_owner_id = resources.upstream_owner_id,
            fork_repository_id = resources.fork_repository_id,
            fork_owner_id = resources.fork_owner_id,
            "CMA Trigger GitHub capability decision"
        );
    }
}

enum PlatformRoute {
    Passthrough {
        slug: String,
    },
    Github {
        catalog_service_id: String,
        path: Vec<String>,
    },
}

fn classify_platform_route(
    path: &str,
    passthrough: &BTreeSet<String>,
) -> Result<PlatformRoute, CapabilityRefusal> {
    if path.is_empty()
        || path.len() > MAX_PATH_BYTES
        || path.contains('%')
        || path.contains('\\')
        || path.contains("//")
        || path.bytes().any(|byte| byte <= b' ' || byte == 0x7f)
    {
        return Err(CapabilityRefusal::MalformedRequest);
    }
    let segments: Vec<String> = path
        .trim_start_matches('/')
        .split('/')
        .map(str::to_string)
        .collect();
    if segments.iter().any(|segment| {
        segment.is_empty() || *segment == "." || *segment == ".." || segment.len() > 255
    }) {
        return Err(CapabilityRefusal::MalformedRequest);
    }
    if segments.len() < PLATFORM_PROXY_PREFIX.len()
        || !segments
            .iter()
            .zip(PLATFORM_PROXY_PREFIX)
            .all(|(actual, expected)| actual == expected)
    {
        return Err(CapabilityRefusal::OperationDenied);
    }
    let Some(selector) = segments.get(3) else {
        return Err(CapabilityRefusal::MalformedRequest);
    };
    if selector == "s" {
        let Some(slug) = segments.get(4) else {
            return Err(CapabilityRefusal::MalformedRequest);
        };
        if !usable_slug(slug) {
            return Err(CapabilityRefusal::MalformedRequest);
        }
        // GitHub authority is catalog-ID routed. Neither the catalog slug nor
        // the captured, user-level slug is accepted as a credential route.
        if slug == GITHUB_SERVICE_SLUG {
            return Err(CapabilityRefusal::OperationDenied);
        }
        if passthrough.contains(slug) {
            return Ok(PlatformRoute::Passthrough { slug: slug.clone() });
        }
        return Err(CapabilityRefusal::OperationDenied);
    }
    if !usable_identifier(selector, 256) {
        return Err(CapabilityRefusal::MalformedRequest);
    }
    if segments.len() <= 4 {
        return Err(CapabilityRefusal::OperationDenied);
    }
    Ok(PlatformRoute::Github {
        catalog_service_id: selector.clone(),
        path: segments[4..].to_vec(),
    })
}

fn parse_query(raw: Option<&str>) -> Result<BTreeMap<String, String>, CapabilityRefusal> {
    let raw = raw.unwrap_or_default();
    if raw.len() > MAX_QUERY_BYTES || raw.contains('%') || raw.contains('#') {
        return Err(CapabilityRefusal::MalformedRequest);
    }
    let mut query = BTreeMap::new();
    if raw.is_empty() {
        return Ok(query);
    }
    for pair in raw.split('&') {
        let (name, value) = pair
            .split_once('=')
            .ok_or(CapabilityRefusal::MalformedRequest)?;
        if name.is_empty()
            || value.is_empty()
            || name.len() > 64
            || value.len() > 512
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
            || value
                .bytes()
                .any(|byte| byte <= b' ' || byte == 0x7f || matches!(byte, b'&' | b'='))
            || query.insert(name.to_string(), value.to_string()).is_some()
            || query.len() > MAX_QUERY_FIELDS
        {
            return Err(CapabilityRefusal::MalformedRequest);
        }
    }
    Ok(query)
}

struct GithubRequestAuthority<'a> {
    fork: &'a RepositoryBinding,
    allowed_ref: &'a str,
    service_id: &'a str,
    catalog_service_id: &'a str,
}

fn validate_github_request(
    method: &Method,
    route: &[String],
    mut query: BTreeMap<String, String>,
    body: &[u8],
    authority: GithubRequestAuthority<'_>,
) -> Result<CanonicalGithubRequest, CapabilityRefusal> {
    let GithubRequestAuthority {
        fork,
        allowed_ref,
        service_id,
        catalog_service_id,
    } = authority;
    let allowed_branch = allowed_branch(allowed_ref)?;
    query.remove("_nyxid_via");
    if route.len() == 2 && route[0] == "repositories" {
        if method != Method::GET
            || route[1] != fork.repository_id.to_string()
            || !query.is_empty()
            || !body.is_empty()
        {
            return Err(CapabilityRefusal::ResourceMismatch);
        }
        return canonical(
            GithubOperationClass::RepositoryRead,
            route,
            &query,
            Vec::new(),
            service_id,
            catalog_service_id,
        );
    }
    if route.len() < 3
        || route[0] != "repos"
        || route[1] != fork.canonical_owner
        || route[2] != fork.canonical_name
    {
        return Err(CapabilityRefusal::ResourceMismatch);
    }
    let tail = &route[3..];
    if tail.is_empty() {
        if method != Method::GET || !query.is_empty() || !body.is_empty() {
            return Err(CapabilityRefusal::OperationDenied);
        }
        return canonical(
            GithubOperationClass::RepositoryRead,
            route,
            &query,
            Vec::new(),
            service_id,
            catalog_service_id,
        );
    }

    if method == Method::GET && body.is_empty() {
        if let Some(operation) = validate_read_route(tail, &query, allowed_branch)? {
            return canonical(
                operation,
                route,
                &query,
                Vec::new(),
                service_id,
                catalog_service_id,
            );
        }
    }
    if method == Method::POST && tail == ["git", "blobs"] && query.is_empty() {
        let canonical_body = canonical_json::<BlobCreate>(body, BlobCreate::validate)?;
        return canonical(
            GithubOperationClass::ObjectCreate,
            route,
            &query,
            canonical_body,
            service_id,
            catalog_service_id,
        );
    }
    if method == Method::POST && tail == ["git", "trees"] && query.is_empty() {
        let canonical_body = canonical_json::<TreeCreate>(body, TreeCreate::validate)?;
        return canonical(
            GithubOperationClass::ObjectCreate,
            route,
            &query,
            canonical_body,
            service_id,
            catalog_service_id,
        );
    }
    if method == Method::POST && tail == ["git", "commits"] && query.is_empty() {
        let canonical_body = canonical_json::<CommitCreate>(body, CommitCreate::validate)?;
        return canonical(
            GithubOperationClass::ObjectCreate,
            route,
            &query,
            canonical_body,
            service_id,
            catalog_service_id,
        );
    }
    if method == Method::POST && tail == ["git", "refs"] && query.is_empty() {
        let canonical_body =
            canonical_json::<RefCreate>(body, |value| value.validate(allowed_ref))?;
        return canonical(
            GithubOperationClass::LaunchRefCreate,
            route,
            &query,
            canonical_body,
            service_id,
            catalog_service_id,
        );
    }
    if method == Method::PATCH
        && tail.len() >= 4
        && tail[..3] == ["git", "refs", "heads"]
        && tail[3..].join("/") == allowed_branch
        && query.is_empty()
    {
        let canonical_body = canonical_json::<RefUpdate>(body, RefUpdate::validate)?;
        return canonical(
            GithubOperationClass::LaunchRefUpdate,
            route,
            &query,
            canonical_body,
            service_id,
            catalog_service_id,
        );
    }
    Err(CapabilityRefusal::OperationDenied)
}

fn validate_read_route(
    tail: &[String],
    query: &BTreeMap<String, String>,
    allowed_branch: &str,
) -> Result<Option<GithubOperationClass>, CapabilityRefusal> {
    if tail.len() == 3 && tail[0] == "git" && matches!(tail[1].as_str(), "blobs" | "commits") {
        return if valid_sha(&tail[2]) && query.is_empty() {
            Ok(Some(GithubOperationClass::TreeRead))
        } else {
            Err(CapabilityRefusal::MalformedRequest)
        };
    }
    if tail.len() == 3 && tail[..2] == ["git", "trees"] {
        let query_ok = query.is_empty()
            || (query.len() == 1 && query.get("recursive").map(String::as_str) == Some("1"));
        return if valid_sha(&tail[2]) && query_ok {
            Ok(Some(GithubOperationClass::TreeRead))
        } else {
            Err(CapabilityRefusal::MalformedRequest)
        };
    }
    if tail.len() >= 4
        && tail[..3] == ["git", "ref", "heads"]
        && tail[3..].join("/") == allowed_branch
        && query.is_empty()
    {
        return Ok(Some(GithubOperationClass::StatusRead));
    }
    if tail.len() >= 2
        && tail[0] == "branches"
        && tail[1..].join("/") == allowed_branch
        && query.is_empty()
    {
        return Ok(Some(GithubOperationClass::StatusRead));
    }
    if tail.len() == 3
        && tail[0] == "commits"
        && matches!(tail[2].as_str(), "status" | "check-runs" | "check-suites")
    {
        return if valid_sha(&tail[1]) && query.is_empty() {
            Ok(Some(GithubOperationClass::StatusRead))
        } else {
            Err(CapabilityRefusal::MalformedRequest)
        };
    }
    Ok(None)
}

fn canonical(
    operation: GithubOperationClass,
    route: &[String],
    query: &BTreeMap<String, String>,
    body: Vec<u8>,
    service_id: &str,
    catalog_service_id: &str,
) -> Result<CanonicalGithubRequest, CapabilityRefusal> {
    let mut query = query.clone();
    query.insert("_nyxid_via".to_string(), service_id.to_string());
    let mut path = format!("/api/v1/proxy/{catalog_service_id}/{}", route.join("/"));
    if !query.is_empty() {
        path.push('?');
        path.push_str(
            &query
                .iter()
                .map(|(name, value)| format!("{name}={value}"))
                .collect::<Vec<_>>()
                .join("&"),
        );
    }
    if path.len() > MAX_PATH_BYTES + MAX_QUERY_BYTES + 1 {
        return Err(CapabilityRefusal::MalformedRequest);
    }
    Ok(CanonicalGithubRequest {
        operation,
        path_and_query: path,
        body,
    })
}

fn canonical_json<T>(
    body: &[u8],
    validate: impl FnOnce(&T) -> Result<(), CapabilityRefusal>,
) -> Result<Vec<u8>, CapabilityRefusal>
where
    T: for<'de> Deserialize<'de> + Serialize,
{
    if body.is_empty() || body.len() > MAX_BODY_BYTES {
        return Err(CapabilityRefusal::MalformedRequest);
    }
    let value: T = serde_json::from_slice(body).map_err(|_| CapabilityRefusal::MalformedRequest)?;
    validate(&value)?;
    serde_json::to_vec(&value).map_err(|_| CapabilityRefusal::MalformedRequest)
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BlobCreate {
    content: String,
    encoding: BlobEncoding,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum BlobEncoding {
    Utf8,
    Base64,
}

impl BlobCreate {
    fn validate(&self) -> Result<(), CapabilityRefusal> {
        if self.content.len() > 1024 * 1024 {
            return Err(CapabilityRefusal::MalformedRequest);
        }
        if matches!(self.encoding, BlobEncoding::Base64)
            && base64::engine::general_purpose::STANDARD
                .decode(&self.content)
                .is_err()
        {
            return Err(CapabilityRefusal::MalformedRequest);
        }
        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TreeCreate {
    base_tree: String,
    tree: Vec<TreeEntry>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TreeEntry {
    path: String,
    mode: String,
    #[serde(rename = "type")]
    kind: TreeEntryKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sha: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    content: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum TreeEntryKind {
    Blob,
    Tree,
}

impl TreeCreate {
    fn validate(&self) -> Result<(), CapabilityRefusal> {
        if !valid_sha(&self.base_tree) || self.tree.is_empty() || self.tree.len() > 10_000 {
            return Err(CapabilityRefusal::MalformedRequest);
        }
        let mut paths = BTreeSet::new();
        for entry in &self.tree {
            let source_count =
                usize::from(entry.sha.is_some()) + usize::from(entry.content.is_some());
            if !valid_repo_path(&entry.path)
                || !paths.insert(entry.path.as_str())
                || source_count != 1
                || !matches!(entry.mode.as_str(), "100644" | "100755" | "040000")
                || (entry.kind == TreeEntryKind::Tree && entry.mode != "040000")
                || (entry.kind == TreeEntryKind::Blob && entry.mode == "040000")
                || entry.sha.as_deref().is_some_and(|sha| !valid_sha(sha))
                || entry.content.as_ref().is_some_and(|content| {
                    content.len() > 1024 * 1024 || entry.kind != TreeEntryKind::Blob
                })
            {
                return Err(CapabilityRefusal::MalformedRequest);
            }
        }
        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CommitCreate {
    message: String,
    tree: String,
    parents: Vec<String>,
}

impl CommitCreate {
    fn validate(&self) -> Result<(), CapabilityRefusal> {
        if self.message.trim().is_empty()
            || self.message.len() > 64 * 1024
            || !valid_sha(&self.tree)
            || self.parents.len() != 1
            || !valid_sha(&self.parents[0])
        {
            return Err(CapabilityRefusal::MalformedRequest);
        }
        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RefCreate {
    #[serde(rename = "ref")]
    reference: String,
    sha: String,
}

impl RefCreate {
    fn validate(&self, allowed_ref: &str) -> Result<(), CapabilityRefusal> {
        if self.reference != allowed_ref || !valid_sha(&self.sha) {
            return Err(CapabilityRefusal::ResourceMismatch);
        }
        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RefUpdate {
    sha: String,
    force: bool,
}

impl RefUpdate {
    fn validate(&self) -> Result<(), CapabilityRefusal> {
        if !valid_sha(&self.sha) || self.force {
            return Err(CapabilityRefusal::OperationDenied);
        }
        Ok(())
    }
}

fn valid_repository_binding(binding: &RepositoryBinding, fork: bool) -> bool {
    binding.repository_id > 0
        && binding.owner_id > 0
        && usable_identifier(&binding.repository_node_id, 256)
        && usable_identifier(&binding.owner_node_id, 256)
        && usable_slug(&binding.canonical_owner)
        && usable_slug(&binding.canonical_name)
        && if fork {
            binding.fork_parent_id.is_some() && binding.fork_source_id.is_some()
        } else {
            binding.fork_parent_id.is_none() && binding.fork_source_id.is_none()
        }
}

fn launch_id_from_trigger_session(session_id: &str) -> Option<String> {
    let simple = session_id.strip_prefix("sesn_trg_")?;
    if simple.len() != 32
        || !simple
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return None;
    }
    Some(format!(
        "{}-{}-{}-{}-{}",
        &simple[..8],
        &simple[8..12],
        &simple[12..16],
        &simple[16..20],
        &simple[20..]
    ))
}

fn usable_identifier(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
}

fn usable_slug(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 100
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn usable_user_service_slug(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 80
        && !value.starts_with('-')
        && !value.ends_with('-')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn valid_ref(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && !value.starts_with('/')
        && !value.ends_with('/')
        && !value.contains("..")
        && !value.contains("//")
        && !value.contains('%')
        && !value.contains('\\')
        && value.split('/').all(usable_slug)
}

fn valid_head_ref(value: &str) -> bool {
    allowed_branch(value).is_ok()
}

fn allowed_branch(value: &str) -> Result<&str, CapabilityRefusal> {
    let branch = value
        .strip_prefix("refs/heads/")
        .ok_or(CapabilityRefusal::ResourceMismatch)?;
    if valid_ref(branch) {
        Ok(branch)
    } else {
        Err(CapabilityRefusal::ResourceMismatch)
    }
}

fn valid_sha(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn valid_repo_path(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 4096
        && !value.starts_with('/')
        && !value.ends_with('/')
        && !value.contains("//")
        && !value.contains('\\')
        && !value.contains('%')
        && value.split('/').all(|segment| {
            !segment.is_empty()
                && segment != "."
                && segment != ".."
                && segment.len() <= 255
                && !segment.bytes().any(|byte| byte <= b' ' || byte == 0x7f)
        })
}

#[cfg(test)]
#[path = "github_capability_tests.rs"]
mod tests;
