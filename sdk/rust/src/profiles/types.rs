//! Typed profile drafts and immutable revisions.

use serde::{Deserialize, Serialize};

string_enum! { /// Resource kind represented by a profile.
    pub enum ProfileKind { Agent => "agent", Workspace => "workspace", Sandbox => "sandbox" }
}
string_enum! { /// Discoverability of a profile's published revisions.
    pub enum ProfileVisibility { Private => "private", Public => "public" }
}
string_enum! { /// Requested model reasoning effort.
    pub enum ThinkingEffort { Minimal => "minimal", Low => "low", Medium => "medium", High => "high", XHigh => "x_high" }
}
string_enum! { /// Approval policy granted to an Agent.
    pub enum PermissionLevel { Guarded => "guarded", Autonomous => "autonomous" }
}
string_enum! { /// Source of model credentials.
    pub enum ModelAccess { ChronoaiManaged => "chronoai_managed", OwnCredentials => "own_credentials" }
}
string_enum! { /// Runtime provider accepted by the current REST API.
    pub enum AgentProvider { Codex => "codex" }
}
string_enum! { /// Registry owning a skill reference.
    pub enum SkillSource { Ornn => "ornn" }
}

/// An immutable profile revision reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileReference {
    /// Profile identifier.
    pub id: String,
    /// Positive immutable revision number.
    #[serde(deserialize_with = "crate::resources::deserialize_positive")]
    pub revision: u32,
}

/// An optional-version skill reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillReference {
    /// Registry owning the skill.
    pub source: SkillSource,
    /// Registry-local skill identifier.
    pub id: String,
    /// Pinned registry version, when specified.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// Agent model, policy, skills, and initial instructions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentProfileContent {
    /// Isolate this Agent's process, files and network access; omitted or false retains the default.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub isolated_agent: bool,
    /// Optional explicit model name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Requested reasoning effort.
    pub thinking_effort: ThinkingEffort,
    /// Approval policy.
    pub permission_level: PermissionLevel,
    /// Skills included in the profile.
    pub skills: Vec<SkillReference>,
    /// Additional system instructions.
    pub system_prompt: String,
}

/// Source from which a profile creates a Workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkspaceLocation {
    /// Clone a Git repository.
    GitRepository {
        /// Repository URL.
        url: String,
        /// Optional revision, branch, or tag.
        #[serde(skip_serializing_if = "Option::is_none")]
        reference: Option<String>,
    },
    /// Create an empty named directory.
    Directory {
        /// Directory name.
        name: String,
    },
}

/// Workspace source, setup commands, and automatic Agent profile references.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceProfileContent {
    /// Repository or directory source.
    pub location: WorkspaceLocation,
    /// Workspace display name.
    pub name: String,
    /// Commands executed during Workspace preparation.
    pub setup_commands: Vec<String>,
    /// Immutable Agent profiles to instantiate.
    pub agent_profiles: Vec<ProfileReference>,
}

/// A plain environment variable supplied by a Sandbox profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvironmentVariable {
    /// Variable name.
    pub name: String,
    /// Variable value; do not place credentials in profile content.
    pub value: String,
}

/// Sandbox resources, credentials policy, software, and Workspace references.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxProfileContent {
    /// Positive CPU allocation in millicores.
    #[serde(deserialize_with = "crate::resources::deserialize_positive")]
    pub cpu_millis: u32,
    /// Positive memory allocation in MiB.
    #[serde(deserialize_with = "crate::resources::deserialize_positive")]
    pub memory_mib: u32,
    /// Requested service connections.
    pub nyxid_services: Vec<String>,
    /// Agent runtime provider.
    pub agent_provider: AgentProvider,
    /// Model credential policy.
    pub model_access: ModelAccess,
    /// Positive Workspace admission limit.
    #[serde(deserialize_with = "crate::resources::deserialize_positive")]
    pub max_workspaces: u8,
    /// Positive per-Workspace Agent admission limit.
    #[serde(deserialize_with = "crate::resources::deserialize_positive")]
    pub max_agents_per_workspace: u8,
    /// Immutable Workspace profiles to instantiate.
    pub workspace_profiles: Vec<ProfileReference>,
    /// Software installation commands covered by trust confirmation.
    pub software_install_commands: Vec<String>,
    /// Plain environment variables.
    pub environment_variables: Vec<EnvironmentVariable>,
}

/// Mutable Sandbox scheduling policy, independent of pinned environment content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxProfileAvailability {
    /// Pre-provision credential-free runtimes for this published public Profile.
    pub warmup_enabled: bool,
    /// Desired ready runtime count, from one through five.
    pub warmup_count: u8,
    /// Prevent inactivity pause, while allowing explicit manual pause.
    pub always_keep_alive: bool,
}

/// Metadata and content used to create an owned draft.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileDraft<C> {
    /// Sandbox Profiles only; defaults to no warmup and normal inactivity pause.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub availability: Option<SandboxProfileAvailability>,
    /// Profile display name.
    pub name: String,
    /// Profile description.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// Kind-specific draft content.
    pub content: C,
}

/// Optimistic draft replacement; omitted metadata retains its current value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateProfile<C> {
    /// Sandbox Profiles only; omission preserves the current policy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub availability: Option<SandboxProfileAvailability>,
    /// Positive version of the draft being replaced.
    pub draft_version: u64,
    /// Optional replacement display name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Optional replacement description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Complete replacement content.
    pub content: C,
}

/// Metadata for one published immutable revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileRevisionSummary {
    /// Immutable revision number.
    #[serde(deserialize_with = "crate::resources::deserialize_positive")]
    pub revision: u32,
    /// Content digest.
    pub digest: String,
    /// Publication timestamp.
    pub published_at: String,
}

/// Owner-facing profile, including its mutable draft.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ProfileDocument<C> {
    /// Mutable scheduling policy, present for Sandbox Profiles.
    #[serde(default)]
    pub availability: Option<SandboxProfileAvailability>,
    /// Profile identifier.
    pub id: String,
    /// Owning subject identifier.
    pub owner: String,
    /// Display name.
    pub name: String,
    /// Description.
    pub description: String,
    /// Published revision visibility.
    pub visibility: ProfileVisibility,
    /// Source revision when this profile was forked.
    pub forked_from: Option<ProfileReference>,
    /// Recorded use count.
    pub uses_count: u64,
    /// Current mutable draft.
    pub draft: C,
    /// Draft compare-and-swap version.
    #[serde(deserialize_with = "crate::resources::deserialize_positive")]
    pub draft_version: u64,
    /// Creation timestamp.
    pub created_at: String,
    /// Last update timestamp.
    pub updated_at: String,
    /// Retirement timestamp, when retired.
    pub retired_at: Option<String>,
    /// Newest published revision, when one exists.
    pub latest_revision: Option<ProfileRevisionSummary>,
}

/// Immutable kind-specific profile content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileRevision<C> {
    /// Owning profile identifier.
    pub profile_id: String,
    /// Immutable revision metadata, flattened on the wire.
    #[serde(flatten)]
    pub summary: ProfileRevisionSummary,
    /// Published content.
    pub content: C,
}
