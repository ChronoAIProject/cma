//! Publisher and anonymous Trigger wire documents.

use crate::profiles::{
    AgentProvider, EnvironmentVariable, PermissionLevel, ProfileReference, SkillReference,
    ThinkingEffort,
};
use serde::{Deserialize, Serialize};

string_enum! { /// Approved external button label.
    pub enum ButtonLabel { OpenInCma => "open_in_cma", ChatWithThis => "chat_with_this", RunWithCma => "run_with_cma", TryIt => "try_it" }
}
string_enum! { /// External button color mode.
    pub enum Theme { Auto => "auto", Light => "light", Dark => "dark" }
}
string_enum! { /// Trigger publication lifecycle.
    pub enum Availability { Draft => "draft", Published => "published", Retired => "retired" }
}
string_enum! { /// Sandbox or Workspace reuse policy.
    pub enum ReusePolicy { ReuseExisting => "reuse_existing", AlwaysNew => "always_new" }
}
string_enum! { /// Agent repeat-launch policy.
    pub enum AgentReusePolicy { ResumeExisting => "resume_existing", AlwaysNew => "always_new", Ask => "ask" }
}

/// Public presentation chosen by the publisher.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Presentation {
    /// Trigger title.
    pub title: String,
    /// Visitor-facing description.
    pub description: String,
    /// Approved button label.
    pub button_label: ButtonLabel,
    /// External color mode.
    pub theme: Theme,
    /// Optional registration context.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signup_hint: Option<String>,
}

/// Inline Agent configuration or legacy immutable profile revisions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Target {
    /// Sandbox profile revision.
    pub sandbox_profile: ProfileReference,
    /// Workspace profile revision.
    pub workspace_profile: ProfileReference,
    /// Agent profile revision.
    pub agent_profile: ProfileReference,
    /// Inline image and Agent configuration for new Trigger launches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inline: Option<TriggerInlineTarget>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
/// Retains historical inline Trigger image selections; new custom image selection is unsupported.
pub struct TriggerImage {
    /// The retained historical custom image identifier.
    pub id: String,
    /// The optional historical custom image version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// The retained immutable OCI image reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
/// Optional GitHub workspace source for an inline Trigger.
pub struct TriggerRepository {
    /// HTTPS GitHub repository URL.
    pub url: String,
    /// Optional branch, tag, or commit reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
/// Inline workspace settings.
pub struct TriggerInlineWorkspace {
    /// Optional repository source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<TriggerRepository>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
/// Inline model, permission, skill, and service scope.
pub struct TriggerInlineAgent {
    /// Optional model override.
    pub model: Option<String>,
    /// Requested reasoning effort.
    pub reasoning_effort: ThinkingEffort,
    /// Guarded or autonomous execution policy.
    pub permission_mode: PermissionLevel,
    /// Skills selected from Ornn.
    pub skills: Vec<SkillReference>,
    /// Initial system instructions.
    pub system_prompt: String,
    /// NyxID service identifiers granted to the Agent.
    pub services: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
/// Inline Trigger target configuration.
pub struct TriggerInlineTarget {
    /// Retained for compatibility; new image selections are rejected with
    /// `trigger_image_selection_unsupported`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<TriggerImage>,
    /// Inline Agent configuration.
    pub agent: TriggerInlineAgent,
    /// Optional workspace configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<TriggerInlineWorkspace>,
}

/// Reuse decisions recorded in a Trigger revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reuse {
    /// Sandbox policy.
    pub sandbox: ReusePolicy,
    /// Workspace policy.
    pub workspace: ReusePolicy,
    /// Agent policy.
    pub agent: AgentReusePolicy,
}

/// Publisher's explicit review requirement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Consent {
    /// Require the visitor review even when all other requirements are satisfied.
    pub always_review: bool,
}

/// Complete editable content of a Trigger draft.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriggerContent {
    /// Public presentation.
    pub presentation: Presentation,
    /// Immutable target revisions.
    pub target: Target,
    /// Visitor resource reuse.
    pub reuse: Reuse,
    /// Text delivered once the visitor's own Agent is ready.
    pub first_message: String,
    /// Explicit visitor message shortcuts from the published revision.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub quick_messages: Vec<TriggerQuickMessage>,
    /// Review policy.
    pub consent: Consent,
}

/// A publisher-defined message sent only after an explicit visitor click.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriggerQuickMessage {
    /// Button label.
    pub name: String,
    /// Hover and focus preview.
    pub description: String,
    /// Exact message text.
    pub text: String,
}

/// Registered external embed placement.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Placement {
    /// Placement identity.
    pub id: String,
    /// Registered source URL.
    pub source_url: String,
    /// Publisher's placement label.
    pub label: String,
    /// Retirement timestamp, when retired.
    pub retired_at: Option<String>,
}

/// Aggregated launch outcomes, without visitor identities.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Totals {
    /// Admitted launches.
    pub launches_total: u64,
    /// Launches that reached an Agent.
    pub agents_ready_total: u64,
}

/// Publisher-only Trigger record.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Trigger {
    /// Publisher identity for this Trigger.
    pub id: String,
    /// Current editable draft.
    pub draft: TriggerContent,
    /// Positive compare-and-swap draft version.
    #[serde(deserialize_with = "crate::resources::deserialize_positive")]
    pub draft_version: u64,
    /// Public identifier allocated on first publication.
    pub public_id: Option<String>,
    /// Publication lifecycle.
    pub availability: Availability,
    /// Number of immutable revisions.
    pub revision_count: u32,
    /// Registered placements.
    pub placements: Vec<Placement>,
    /// Aggregate outcomes.
    pub stats: Totals,
    /// Creation timestamp.
    pub created_at: String,
    /// Last update timestamp.
    pub updated_at: String,
    /// Retirement timestamp.
    pub retired_at: Option<String>,
}

/// Immutable published Trigger content.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct TriggerRevision {
    /// Positive immutable revision number.
    #[serde(deserialize_with = "crate::resources::deserialize_positive")]
    pub revision: u32,
    /// Exact published content.
    pub content: TriggerContent,
    /// Content digest.
    pub digest: String,
    /// Publication timestamp.
    pub published_at: String,
}

/// Publication result with both owner and public identities.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PublishedTriggerRevision {
    /// Owner-facing identifier.
    pub trigger_id: String,
    /// Anonymous public identifier.
    pub public_id: String,
    /// Current lifecycle.
    pub availability: Availability,
    /// Published revision fields.
    #[serde(flatten)]
    pub publication: TriggerRevision,
}

/// Statistics for one registered or unattributed placement.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PlacementTotals {
    /// Placement identity; absent for unattributed launches.
    pub placement_id: Option<String>,
    /// Outcome counters.
    #[serde(flatten)]
    pub totals: Totals,
}

/// Trigger totals and placement breakdown.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Statistics {
    /// Overall counters.
    pub totals: Totals,
    /// Per-placement counters.
    pub placements: Vec<PlacementTotals>,
}

/// Server-generated CSP source directives.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ContentSecurityPolicy {
    /// Image source directive.
    pub img_src: String,
    /// Script source directive.
    pub script_src: String,
}

/// Server-authored integration snippets; copy without client reconstruction.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct EmbedSnippets {
    /// Markdown anchor and badge.
    pub markdown: String,
    /// HTML anchor and badge.
    pub html: String,
    /// Integrity-pinned script integration.
    pub javascript: String,
    /// TypeScript React helper integration.
    pub react: String,
    /// Direct public URL.
    pub url: String,
}

/// Complete server-generated embed descriptor.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct EmbedDescriptor {
    /// Visitor entry URL.
    pub public_url: String,
    /// Server-rendered badge URL.
    pub badge_url: String,
    /// Accessible button label.
    pub label_text: String,
    /// Pinned script integrity value.
    pub javascript_sri: String,
    /// Required source directives.
    pub content_security_policy: ContentSecurityPolicy,
    /// Copy-ready integrations.
    pub snippets: EmbedSnippets,
}

/// Commands disclosed before a visitor admits work.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "resource", rename_all = "snake_case")]
pub enum TrustDisclosure {
    /// Global Sandbox software and environment.
    Sandbox {
        /// Digest of reviewed commands.
        commands_digest: String,
        /// Installation commands.
        commands: Vec<String>,
        /// Public plain environment values.
        environment_variables: Vec<EnvironmentVariable>,
    },
    /// Workspace setup commands.
    Workspace {
        /// Workspace name.
        name: String,
        /// Digest of reviewed commands.
        commands_digest: String,
        /// Setup commands.
        commands: Vec<String>,
    },
}

/// Public trust summary.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PreviewTrust {
    /// Every setup command group to review.
    pub disclosures: Vec<TrustDisclosure>,
}

/// Requested resource capacity.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Size {
    /// CPU allocation in millicores.
    pub cpu_millis: u32,
    /// Memory allocation in MiB.
    pub memory_mib: u32,
}

/// Public runtime and connection requirements.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Technical {
    /// Runtime provider.
    pub provider: AgentProvider,
    /// Optional explicit model name.
    pub model: Option<String>,
    /// Reasoning effort.
    pub effort: ThinkingEffort,
    /// Requested capacity.
    pub size: Size,
    /// Required service identifiers.
    pub services: Vec<String>,
}

/// Credential-suppressed public preview, which never starts work.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct TriggerPreview {
    /// Whether the publisher requires a separate review before Launch admission.
    #[serde(default)]
    pub always_review: Option<bool>,
    /// Public presentation fields.
    #[serde(flatten)]
    pub presentation: Presentation,
    /// Publisher display name, never an owner credential.
    pub publisher_display_name: String,
    /// Visitor-facing resource behavior.
    pub what_happens: Vec<String>,
    /// Current availability.
    pub availability: Availability,
    /// Setup command disclosure.
    pub trust: PreviewTrust,
    /// Technical requirements.
    pub technical: Technical,
    /// Initial Agent message.
    pub first_message: String,
}
