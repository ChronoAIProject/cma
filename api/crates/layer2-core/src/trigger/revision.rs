use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::ConnectionSnapshot;
use crate::{PermissionMode, SessionSize, SubjectId};

pub const TRIGGER_SCHEMA_VERSION: u32 = 1;
pub const PROMPT_RENDERER_VERSION: u32 = 1;
pub const MAX_PUBLIC_SUMMARY_BYTES: usize = 2_000;
pub const MAX_PROMPT_TEMPLATE_BYTES: usize = 32 * 1024;
pub const MAX_RENDERED_PROMPT_BYTES: usize = 64 * 1024;
pub const MAX_INPUT_FIELDS: usize = 32;
pub const MAX_FIELD_KEY_BYTES: usize = 64;
pub const MAX_FIELD_TEXT_BYTES: usize = 8 * 1024;
pub const MAX_CAPABILITIES: usize = 32;

macro_rules! opaque_id {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            pub fn new() -> Self {
                Self(Uuid::new_v4().to_string())
            }

            pub fn parse(value: impl Into<String>) -> Result<Self, TriggerValidationError> {
                let value = value.into();
                Uuid::parse_str(&value).map_err(|_| {
                    TriggerValidationError::new(concat!(stringify!($name), " is not valid"))
                })?;
                Ok(Self(value))
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }
    };
}

opaque_id!(TriggerId);
opaque_id!(TriggerRevisionId);
opaque_id!(PublicTriggerId);
opaque_id!(LaunchId);
opaque_id!(CheckpointId);
opaque_id!(DiffArtifactId);
opaque_id!(ContributionId);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriggerValidationError {
    pub field: Option<String>,
    pub detail: String,
}

impl TriggerValidationError {
    pub fn new(detail: impl Into<String>) -> Self {
        Self {
            field: None,
            detail: detail.into(),
        }
    }

    pub fn field(field: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            field: Some(field.into()),
            detail: detail.into(),
        }
    }
}

impl fmt::Display for TriggerValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.field.as_deref() {
            Some(field) => write!(formatter, "{field}: {}", self.detail),
            None => formatter.write_str(&self.detail),
        }
    }
}

impl std::error::Error for TriggerValidationError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepositoryVisibility {
    Public,
    Private,
    Internal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceMode {
    PinnedCommit,
    BranchHeadAtLaunch,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepositoryIdentity {
    pub repository_id: u64,
    pub repository_node_id: String,
    pub owner_id: u64,
    pub owner_node_id: String,
    pub canonical_owner: String,
    pub canonical_name: String,
    pub visibility: RepositoryVisibility,
    pub allow_forking: bool,
    pub default_branch: String,
    pub selected_ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fork_parent_id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fork_source_id: Option<u64>,
}

impl RepositoryIdentity {
    pub fn validate_public_upstream(&self) -> Result<(), TriggerValidationError> {
        if self.repository_id == 0 || self.owner_id == 0 {
            return Err(TriggerValidationError::field(
                "upstream.repository_id",
                "numeric repository and owner identities are required",
            ));
        }
        for (field, value) in [
            (
                "upstream.repository_node_id",
                self.repository_node_id.as_str(),
            ),
            ("upstream.owner_node_id", self.owner_node_id.as_str()),
            ("upstream.canonical_owner", self.canonical_owner.as_str()),
            ("upstream.canonical_name", self.canonical_name.as_str()),
            ("upstream.default_branch", self.default_branch.as_str()),
            ("upstream.selected_ref", self.selected_ref.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(TriggerValidationError::field(field, "must not be empty"));
            }
            if value.len() > 255 {
                return Err(TriggerValidationError::field(field, "is too long"));
            }
        }
        if self.visibility != RepositoryVisibility::Public || !self.allow_forking {
            return Err(TriggerValidationError::field(
                "upstream",
                "the public MVP requires a public repository whose policy permits forking",
            ));
        }
        if self.fork_parent_id.is_some() || self.fork_source_id.is_some() {
            return Err(TriggerValidationError::field(
                "upstream",
                "a Trigger upstream must be the selected source repository, not a fork",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpstreamRevision {
    pub repository: RepositoryIdentity,
    pub source_mode: SourceMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_source_sha: Option<String>,
    pub contribution_base_ref: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputFieldKind {
    Text { min_bytes: u32, max_bytes: u32 },
    Integer { minimum: i64, maximum: i64 },
    Boolean,
    Choice { values: Vec<String> },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InputFieldDefinition {
    pub key: String,
    pub kind: InputFieldKind,
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    pub editable: bool,
}

impl InputFieldDefinition {
    pub fn validate_value(&self, value: &Value) -> Result<(), TriggerValidationError> {
        let invalid = |detail: String| {
            TriggerValidationError::field(format!("task.input_fields.{}", self.key), detail)
        };
        match (&self.kind, value) {
            (
                InputFieldKind::Text {
                    min_bytes,
                    max_bytes,
                },
                Value::String(value),
            ) => {
                let len = value.len() as u32;
                if len < *min_bytes || len > *max_bytes {
                    return Err(invalid(format!(
                        "must contain between {min_bytes} and {max_bytes} bytes"
                    )));
                }
            }
            (InputFieldKind::Integer { minimum, maximum }, Value::Number(value)) => {
                let Some(value) = value.as_i64() else {
                    return Err(invalid("must be an integer".into()));
                };
                if value < *minimum || value > *maximum {
                    return Err(invalid(format!("must be between {minimum} and {maximum}")));
                }
            }
            (InputFieldKind::Boolean, Value::Bool(_)) => {}
            (InputFieldKind::Choice { values }, Value::String(value))
                if values.iter().any(|allowed| allowed == value) => {}
            (InputFieldKind::Choice { .. }, Value::String(_)) => {
                return Err(invalid("is not one of the declared choices".into()));
            }
            _ => return Err(invalid("has the wrong type".into())),
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TriggerTask {
    pub public_summary: String,
    pub prompt_template: String,
    pub renderer_version: u32,
    #[serde(default)]
    pub input_fields: Vec<InputFieldDefinition>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequiredCapability {
    Conversation,
    DurableConversation,
    WorkspaceCheckpoint,
    RestrictedServiceGateway,
    CredentialRefresh,
    CredentialRevocation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourcePolicy {
    pub default: SessionSize,
    pub ceiling: SessionSize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriggerSessionPolicy {
    #[serde(default)]
    pub required_capabilities: Vec<RequiredCapability>,
    pub permission_ceiling: PermissionMode,
    pub sandbox_profile: String,
    pub resources: ResourcePolicy,
    pub timeout_default_seconds: u32,
    pub timeout_ceiling_seconds: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeClass {
    PublicRepository,
    Workflow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepositoryOperationClass {
    RepositoryRead,
    TreeRead,
    ObjectCreate,
    LaunchRefCreate,
    LaunchRefUpdate,
    StatusRead,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialRoute {
    ContributorConnection,
    SponsoredGrant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowChangePolicy {
    Refuse,
    RequireScopeAndConfirmation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepositoryAccessPolicy {
    pub credential_route: CredentialRoute,
    #[serde(default)]
    pub required_scope_classes: Vec<ScopeClass>,
    #[serde(default)]
    pub allowed_operation_classes: Vec<RepositoryOperationClass>,
    pub workflow_change_policy: WorkflowChangePolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutorPolicy {
    Disabled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupPolicy {
    pub executor_policy: ExecutorPolicy,
    #[serde(default)]
    pub steps: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommitAttributionPolicy {
    ContributorNoReply,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullRequestDefaults {
    pub title_template: String,
    pub body_template: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContributionPolicy {
    pub branch_prefix: String,
    pub commit_attribution: CommitAttributionPolicy,
    pub pull_request: PullRequestDefaults,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BillingMode {
    Contributor,
    ZeroCost,
    Sponsored,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommercialPolicy {
    pub billing_mode: BillingMode,
    pub policy_reference: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EditableField {
    TaskInput,
    BranchSuffix,
    ResourceSize,
    Timeout,
    OptionalChecks,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OverridePolicy {
    #[serde(default)]
    pub editable_fields: Vec<EditableField>,
    #[serde(default)]
    pub optional_checks: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicProjectionPolicy {
    pub version: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TriggerRevisionContent {
    pub schema_version: u32,
    pub trigger_id: TriggerId,
    pub revision_number: u32,
    pub publisher_subject_id: SubjectId,
    pub publisher_display_identity: String,
    pub upstream: UpstreamRevision,
    pub task: TriggerTask,
    pub session: TriggerSessionPolicy,
    pub repository_access: RepositoryAccessPolicy,
    pub setup: SetupPolicy,
    pub contribution: ContributionPolicy,
    pub commercial_policy: CommercialPolicy,
    pub override_policy: OverridePolicy,
    pub public_projection: PublicProjectionPolicy,
}

impl TriggerRevisionContent {
    pub fn validate(&self) -> Result<(), TriggerValidationError> {
        if self.schema_version != TRIGGER_SCHEMA_VERSION {
            return Err(TriggerValidationError::field(
                "schema_version",
                "is not supported by this deployment",
            ));
        }
        if self.revision_number == 0 {
            return Err(TriggerValidationError::field(
                "revision_number",
                "must be greater than zero",
            ));
        }
        if self.publisher_subject_id.0.trim().is_empty()
            || self.publisher_display_identity.trim().is_empty()
        {
            return Err(TriggerValidationError::field(
                "publisher",
                "publisher identity is required",
            ));
        }
        self.upstream.repository.validate_public_upstream()?;
        validate_ref(
            "upstream.contribution_base_ref",
            &self.upstream.contribution_base_ref,
        )?;
        match self.upstream.source_mode {
            SourceMode::PinnedCommit => {
                let sha = self
                    .upstream
                    .observed_source_sha
                    .as_deref()
                    .unwrap_or_default();
                validate_sha("upstream.observed_source_sha", sha)?;
            }
            SourceMode::BranchHeadAtLaunch => {
                if let Some(sha) = self.upstream.observed_source_sha.as_deref() {
                    validate_sha("upstream.observed_source_sha", sha)?;
                }
            }
        }
        validate_task(&self.task)?;
        validate_session(&self.session)?;
        validate_repository_access(&self.repository_access)?;
        if self.setup.executor_policy != ExecutorPolicy::Disabled || !self.setup.steps.is_empty() {
            return Err(TriggerValidationError::field(
                "setup",
                "publisher-authored setup is disabled in the public MVP",
            ));
        }
        if self.contribution.branch_prefix != "cma/" {
            return Err(TriggerValidationError::field(
                "contribution.branch_prefix",
                "the public MVP requires the server-owned cma/ prefix",
            ));
        }
        for (field, value) in [
            (
                "contribution.pull_request.title_template",
                self.contribution.pull_request.title_template.as_str(),
            ),
            (
                "contribution.pull_request.body_template",
                self.contribution.pull_request.body_template.as_str(),
            ),
        ] {
            if value.trim().is_empty() || value.len() > MAX_PROMPT_TEMPLATE_BYTES {
                return Err(TriggerValidationError::field(
                    field,
                    "must be non-empty and within the configured size bound",
                ));
            }
        }
        if self.commercial_policy.billing_mode == BillingMode::Sponsored
            || self.repository_access.credential_route == CredentialRoute::SponsoredGrant
        {
            return Err(TriggerValidationError::field(
                "commercial_policy",
                "sponsored credentials are disabled in the public MVP",
            ));
        }
        if self.commercial_policy.policy_reference.trim().is_empty() {
            return Err(TriggerValidationError::field(
                "commercial_policy.policy_reference",
                "a server-owned pricing policy reference is required",
            ));
        }
        let editable: BTreeSet<_> = self
            .override_policy
            .editable_fields
            .iter()
            .copied()
            .collect();
        if editable.len() != self.override_policy.editable_fields.len() {
            return Err(TriggerValidationError::field(
                "override_policy.editable_fields",
                "contains a duplicate field",
            ));
        }
        if self.override_policy.optional_checks.len() > 16
            || self
                .override_policy
                .optional_checks
                .iter()
                .any(|check| check.trim().is_empty() || check.len() > 128)
        {
            return Err(TriggerValidationError::field(
                "override_policy.optional_checks",
                "contains too many or invalid check identifiers",
            ));
        }
        if self.public_projection.version != 1 {
            return Err(TriggerValidationError::field(
                "public_projection.version",
                "is not supported",
            ));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, TriggerValidationError> {
        self.validate()?;
        serde_json::to_vec(self)
            .map_err(|_| TriggerValidationError::new("revision cannot be canonically encoded"))
    }

    pub fn digest(&self) -> Result<String, TriggerValidationError> {
        let bytes = self.canonical_bytes()?;
        let digest = Sha256::digest(bytes);
        Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
    }

    pub fn render_prompt(
        &self,
        supplied: &BTreeMap<String, Value>,
    ) -> Result<String, TriggerValidationError> {
        super::template::render(&self.task, supplied)
    }
}

fn validate_task(task: &TriggerTask) -> Result<(), TriggerValidationError> {
    if task.public_summary.trim().is_empty() || task.public_summary.len() > MAX_PUBLIC_SUMMARY_BYTES
    {
        return Err(TriggerValidationError::field(
            "task.public_summary",
            format!("must be non-empty and at most {MAX_PUBLIC_SUMMARY_BYTES} bytes"),
        ));
    }
    if task.prompt_template.trim().is_empty()
        || task.prompt_template.len() > MAX_PROMPT_TEMPLATE_BYTES
    {
        return Err(TriggerValidationError::field(
            "task.prompt_template",
            format!("must be non-empty and at most {MAX_PROMPT_TEMPLATE_BYTES} bytes"),
        ));
    }
    if task.renderer_version != PROMPT_RENDERER_VERSION {
        return Err(TriggerValidationError::field(
            "task.renderer_version",
            "is not supported",
        ));
    }
    if task.input_fields.len() > MAX_INPUT_FIELDS {
        return Err(TriggerValidationError::field(
            "task.input_fields",
            format!("at most {MAX_INPUT_FIELDS} fields are allowed"),
        ));
    }
    let mut keys = BTreeSet::new();
    for field in &task.input_fields {
        if field.key.is_empty()
            || field.key.len() > MAX_FIELD_KEY_BYTES
            || !field
                .key
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        {
            return Err(TriggerValidationError::field(
                "task.input_fields.key",
                "must contain only ASCII letters, numbers, or underscore",
            ));
        }
        if !keys.insert(field.key.as_str()) {
            return Err(TriggerValidationError::field(
                "task.input_fields",
                format!("{} is declared twice", field.key),
            ));
        }
        match &field.kind {
            InputFieldKind::Text {
                min_bytes,
                max_bytes,
            } => {
                if min_bytes > max_bytes || *max_bytes as usize > MAX_FIELD_TEXT_BYTES {
                    return Err(TriggerValidationError::field(
                        format!("task.input_fields.{}", field.key),
                        "has invalid text bounds",
                    ));
                }
            }
            InputFieldKind::Integer { minimum, maximum } if minimum > maximum => {
                return Err(TriggerValidationError::field(
                    format!("task.input_fields.{}", field.key),
                    "has invalid integer bounds",
                ));
            }
            InputFieldKind::Choice { values } => {
                let unique: BTreeSet<_> = values.iter().collect();
                if values.is_empty()
                    || values.len() > 64
                    || unique.len() != values.len()
                    || values
                        .iter()
                        .any(|value| value.is_empty() || value.len() > 256)
                {
                    return Err(TriggerValidationError::field(
                        format!("task.input_fields.{}", field.key),
                        "has invalid choice values",
                    ));
                }
            }
            _ => {}
        }
        if let Some(default) = field.default.as_ref() {
            field.validate_value(default)?;
        } else if field.required {
            return Err(TriggerValidationError::field(
                format!("task.input_fields.{}.default", field.key),
                "a required field needs a default for a stable README revision",
            ));
        }
    }
    super::template::validate(task)?;
    Ok(())
}

fn validate_session(session: &TriggerSessionPolicy) -> Result<(), TriggerValidationError> {
    if session.permission_ceiling == PermissionMode::Unrestricted {
        return Err(TriggerValidationError::field(
            "session.permission_ceiling",
            "unrestricted permission is outside the public MVP",
        ));
    }
    if session.sandbox_profile.trim().is_empty() || session.sandbox_profile.len() > 128 {
        return Err(TriggerValidationError::field(
            "session.sandbox_profile",
            "must name a deployment-owned profile",
        ));
    }
    if session.resources.default.cpu_millis() > session.resources.ceiling.cpu_millis()
        || session.resources.default.memory_mib() > session.resources.ceiling.memory_mib()
    {
        return Err(TriggerValidationError::field(
            "session.resources",
            "the default must fit within the ceiling",
        ));
    }
    if session.timeout_default_seconds == 0
        || session.timeout_default_seconds > session.timeout_ceiling_seconds
        || session.timeout_ceiling_seconds > 86_400
    {
        return Err(TriggerValidationError::field(
            "session.timeout",
            "the default and ceiling must be ordered and at most one day",
        ));
    }
    if session.required_capabilities.len() > MAX_CAPABILITIES
        || session
            .required_capabilities
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
            .len()
            != session.required_capabilities.len()
    {
        return Err(TriggerValidationError::field(
            "session.required_capabilities",
            "contains too many or duplicate capabilities",
        ));
    }
    let required: BTreeSet<_> = session.required_capabilities.iter().copied().collect();
    for capability in [
        RequiredCapability::Conversation,
        RequiredCapability::DurableConversation,
        RequiredCapability::WorkspaceCheckpoint,
        RequiredCapability::RestrictedServiceGateway,
        RequiredCapability::CredentialRefresh,
        RequiredCapability::CredentialRevocation,
    ] {
        if !required.contains(&capability) {
            return Err(TriggerValidationError::field(
                "session.required_capabilities",
                format!("the public MVP requires {capability:?}"),
            ));
        }
    }
    Ok(())
}

fn validate_repository_access(
    access: &RepositoryAccessPolicy,
) -> Result<(), TriggerValidationError> {
    if access.credential_route != CredentialRoute::ContributorConnection {
        return Err(TriggerValidationError::field(
            "repository_access.credential_route",
            "the public MVP uses the contributor connection",
        ));
    }
    let scopes: BTreeSet<_> = access.required_scope_classes.iter().copied().collect();
    if !scopes.contains(&ScopeClass::PublicRepository) {
        return Err(TriggerValidationError::field(
            "repository_access.required_scope_classes",
            "public_repository is required",
        ));
    }
    if scopes.len() != access.required_scope_classes.len() {
        return Err(TriggerValidationError::field(
            "repository_access.required_scope_classes",
            "contains a duplicate scope class",
        ));
    }
    let operations: BTreeSet<_> = access.allowed_operation_classes.iter().copied().collect();
    if operations.is_empty() || operations.len() != access.allowed_operation_classes.len() {
        return Err(TriggerValidationError::field(
            "repository_access.allowed_operation_classes",
            "must be a non-empty set without duplicates",
        ));
    }
    Ok(())
}

pub(crate) fn validate_ref(field: &str, value: &str) -> Result<(), TriggerValidationError> {
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
        return Err(TriggerValidationError::field(
            field,
            "is not a safe ref name",
        ));
    }
    Ok(())
}

pub(crate) fn validate_sha(field: &str, value: &str) -> Result<(), TriggerValidationError> {
    if value.len() != 40 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(TriggerValidationError::field(
            field,
            "must be a 40-character hexadecimal commit identifier",
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerAvailability {
    Published,
    Retired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PublisherRepositoryRole {
    Owner,
    Admin,
    Maintain,
}

/// Provider-neutral evidence captured at the immutable publication boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublisherAuthority {
    pub subject: SubjectId,
    pub subject_token_generation: u64,
    pub connection: ConnectionSnapshot,
    pub repository: RepositoryIdentity,
    pub role: PublisherRepositoryRole,
    pub verified_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TriggerDraft {
    pub id: TriggerId,
    pub owner: SubjectId,
    pub version: u64,
    pub next_revision_number: u32,
    pub content: TriggerRevisionContent,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TriggerRevision {
    pub id: TriggerRevisionId,
    pub public_id: PublicTriggerId,
    pub content: TriggerRevisionContent,
    pub publisher_authority: PublisherAuthority,
    pub digest: String,
    pub availability: TriggerAvailability,
    pub published_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retired_at: Option<String>,
}

impl TriggerRevision {
    pub fn validate(&self) -> Result<(), TriggerValidationError> {
        self.content.validate()?;
        if self.digest != self.content.digest()? {
            return Err(TriggerValidationError::field(
                "revision.digest",
                "does not match the immutable content",
            ));
        }
        let authority = &self.publisher_authority;
        let expected = &self.content.upstream.repository;
        if authority.subject != self.content.publisher_subject_id
            || authority.subject_token_generation == 0
            || authority.repository != *expected
            || authority.verified_at.trim().is_empty()
        {
            return Err(TriggerValidationError::field(
                "revision.publisher_authority",
                "does not authorize the immutable publisher and repository",
            ));
        }
        authority
            .connection
            .validate_for(&self.content.repository_access.required_scope_classes)?;
        match self.availability {
            TriggerAvailability::Published if self.retired_at.is_some() => {
                Err(TriggerValidationError::field(
                    "revision.retired_at",
                    "must be absent while the revision is published",
                ))
            }
            TriggerAvailability::Retired
                if self
                    .retired_at
                    .as_deref()
                    .is_none_or(|value| value.trim().is_empty()) =>
            {
                Err(TriggerValidationError::field(
                    "revision.retired_at",
                    "is required for a retired revision",
                ))
            }
            _ => Ok(()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TriggerPreview {
    pub public_id: PublicTriggerId,
    pub availability: TriggerAvailability,
    pub publisher_display_identity: String,
    pub repository_id: u64,
    pub repository_owner: String,
    pub repository_name: String,
    pub visibility: RepositoryVisibility,
    pub allow_forking: bool,
    pub public_summary: String,
    pub permission_ceiling: PermissionMode,
    pub required_capabilities: Vec<RequiredCapability>,
    pub resource_default: SessionSize,
    pub resource_ceiling: SessionSize,
    pub timeout_default_seconds: u32,
    pub timeout_ceiling_seconds: u32,
    pub billing_mode: BillingMode,
    pub pricing_policy_reference: String,
    pub revision_digest: String,
}

impl From<&TriggerRevision> for TriggerPreview {
    fn from(revision: &TriggerRevision) -> Self {
        let content = &revision.content;
        Self {
            public_id: revision.public_id.clone(),
            availability: revision.availability,
            publisher_display_identity: content.publisher_display_identity.clone(),
            repository_id: content.upstream.repository.repository_id,
            repository_owner: content.upstream.repository.canonical_owner.clone(),
            repository_name: content.upstream.repository.canonical_name.clone(),
            visibility: content.upstream.repository.visibility,
            allow_forking: content.upstream.repository.allow_forking,
            public_summary: content.task.public_summary.clone(),
            permission_ceiling: content.session.permission_ceiling,
            required_capabilities: content.session.required_capabilities.clone(),
            resource_default: content.session.resources.default,
            resource_ceiling: content.session.resources.ceiling,
            timeout_default_seconds: content.session.timeout_default_seconds,
            timeout_ceiling_seconds: content.session.timeout_ceiling_seconds,
            billing_mode: content.commercial_policy.billing_mode,
            pricing_policy_reference: content.commercial_policy.policy_reference.clone(),
            revision_digest: revision.digest.clone(),
        }
    }
}
