//! What a profile looks like on the wire, in and out.
//!
//! Split from the routes so that file is about handlers and this one about the
//! shapes they translate. The division is where the thinking is: every type
//! here exists because a JSON body and a core type are not the same thing, and
//! each conversion is the one place a field could be dropped or renamed
//! without anything failing.
//!
//! Variable values travel in and out because a client that could not read back
//! what it stored could not offer an edit form. They are never logged here, and
//! the core's own `Debug` keeps them out of a log elsewhere.

use layer2_core::{
    AgentConfiguration, CatalogSkill, CoreError, CredentialMode, EnvironmentProfile,
    EnvironmentProfileService, EnvironmentProfileSpec, EnvironmentVariable, KernelId,
    PermissionMode, ReasoningEffort, ServiceId, SessionMode, SessionSize, SkillId, SoftwarePackage,
};
use serde::{Deserialize, Serialize};

/// A variable as this interface reads and writes it.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct VariableBody {
    /// Variable name.
    pub key: String,
    /// Value. Opaque to every layer that carries it.
    pub value: String,
}

/// A package as this interface reads and writes it.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PackageBody {
    /// Package name.
    pub name: String,
    /// Version to pin, when the caller cares which one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// Request body for creating or replacing a profile.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileRequest {
    /// Name the owner recognises it by.
    pub name: String,
    /// Variables to set in the session.
    #[serde(default)]
    pub variables: Vec<VariableBody>,
    /// Software the session should carry.
    #[serde(default)]
    pub packages: Vec<PackageBody>,
    /// How the agent behaves in sessions started from this profile.
    #[serde(default)]
    pub agent: AgentBody,
    /// Which of the owner's platform services the session may reach.
    #[serde(default)]
    pub nyxid: NyxIdBody,
    /// Standing instructions the owner wants their sessions to start with.
    ///
    /// Theirs alone. What this platform tells an agent about itself is
    /// composed separately and is not editable from here — a session talked
    /// out of knowing it is already authenticated spends its first minutes
    /// rediscovering that badly.
    #[serde(default)]
    pub agent_notes: String,
}

/// The platform-access half of a profile, as a caller supplies it.
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NyxIdBody {
    /// Identifiers of the services to allow.
    #[serde(default)]
    pub services: Vec<String>,
}

/// The agent half of a profile, as a caller supplies it.
///
/// Every field defaults, so a client that cares about none of them sends a
/// name and gets a profile that behaves the way the deployment says it should.
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentBody {
    /// Which runtime implementation backs the session.
    #[serde(default)]
    pub kernel: Option<String>,
    /// Which plane sessions from this profile are driven on.
    #[serde(default)]
    pub mode: Option<SessionMode>,
    /// Model the agent selects.
    #[serde(default)]
    pub model: Option<String>,
    /// How hard it thinks.
    #[serde(default)]
    pub reasoning_effort: Option<ReasoningEffort>,
    /// How often it stops to ask.
    #[serde(default)]
    pub permission: Option<PermissionMode>,
    /// Whose model access it spends.
    #[serde(default)]
    pub credential: Option<CredentialMode>,
    /// How much a session from this profile reserves.
    #[serde(default)]
    pub size: Option<SizeBody>,
    /// Whether a session's tool bodies are recorded in its durable log.
    #[serde(default)]
    pub record_tool_bodies: Option<bool>,
    /// Skill identifiers to load.
    #[serde(default)]
    pub skills: Vec<String>,
}

/// A requested session size, as a caller supplies it.
///
/// Two integers with their units in their names. A field called `cpu` carrying
/// `1` is ambiguous between one core and one millicore, and the two differ by a
/// factor of a thousand.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SizeBody {
    pub cpu_millis: u32,
    pub memory_mib: u32,
}

impl TryFrom<ProfileRequest> for EnvironmentProfileSpec {
    type Error = CoreError;

    fn try_from(body: ProfileRequest) -> Result<Self, Self::Error> {
        Ok(Self {
            name: body.name,
            agent_notes: body.agent_notes,
            variables: body
                .variables
                .into_iter()
                .map(|v| EnvironmentVariable::new(v.key, v.value))
                .collect(),
            packages: body
                .packages
                .into_iter()
                .map(|p| SoftwarePackage {
                    name: p.name,
                    version: p.version,
                })
                .collect(),
            agent: AgentConfiguration {
                // Left unset when a client says nothing, and settled by the
                // core from what this deployment configured. An absent field
                // means "whatever is ordinary here", never "empty" — and this
                // layer is not the one that knows what ordinary is.
                kernel: body.agent.kernel.map(KernelId::new).unwrap_or_default(),
                mode: body.agent.mode.unwrap_or_default(),
                model: body.agent.model,
                reasoning_effort: body.agent.reasoning_effort.unwrap_or_default(),
                permission: body.agent.permission.unwrap_or_default(),
                record_tool_bodies: body.agent.record_tool_bodies.unwrap_or_default(),
                credential: body.agent.credential.unwrap_or_default(),
                size: body
                    .agent
                    .size
                    .map(|s| SessionSize::new(s.cpu_millis, s.memory_mib))
                    .transpose()?,
                skills: body.agent.skills.into_iter().map(SkillId::new).collect(),
            },
            services: body
                .nyxid
                .services
                .into_iter()
                .map(ServiceId::new)
                .collect(),
        })
    }
}

/// The agent half of a profile, as this interface renders it.
///
/// Every field is present rather than skipped when it matches a default: this
/// is what a client draws its controls from, and a form cannot preselect a
/// value it was not sent.
#[derive(Debug, Serialize)]
pub struct AgentView {
    kernel: String,
    mode: SessionMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    reasoning_effort: ReasoningEffort,
    permission: PermissionMode,
    credential: CredentialMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    size: Option<SessionSize>,
    record_tool_bodies: bool,
    skills: Vec<String>,
}

/// The platform-access half of a profile, as this interface renders it.
#[derive(Debug, Serialize)]
pub struct NyxIdView {
    services: Vec<String>,
}

/// A profile as this interface renders it.
#[derive(Debug, Serialize)]
pub struct ProfileView {
    id: String,
    name: String,
    variables: Vec<VariableBody>,
    packages: Vec<PackageBody>,
    agent: AgentView,
    nyxid: NyxIdView,
    /// Standing instructions the owner wrote for their own sessions.
    ///
    /// Only theirs. What the platform tells an agent about itself is composed
    /// when a session starts and is never returned here, because a client that
    /// showed it as editable would be offering to edit something it cannot.
    agent_notes: String,
    /// Which entries the platform mandates, so a client can render them as
    /// fixed rather than guessing — and so the rule is visible rather than
    /// only enforced when the client gets it wrong.
    mandatory: MandatoryView,
    created_at: String,
    updated_at: String,
    /// Whether its owner's sessions start from this profile unless told
    /// otherwise. At most one of theirs carries it.
    preferred: bool,
}

/// The parts of a profile a caller may not remove.
#[derive(Debug, Default, Serialize)]
pub struct MandatoryView {
    packages: Vec<String>,
    skills: Vec<String>,
}

impl MandatoryView {
    pub(super) fn of(baseline: &layer2_core::Baseline) -> Self {
        Self {
            packages: baseline.packages().iter().map(|p| p.name.clone()).collect(),
            skills: baseline.skills().iter().map(|s| s.0.clone()).collect(),
        }
    }
}

impl ProfileView {
    pub(super) fn render(profile: EnvironmentProfile, service: &EnvironmentProfileService) -> Self {
        Self {
            id: profile.id.0,
            name: profile.name,
            variables: profile
                .variables
                .into_iter()
                .map(|v| VariableBody {
                    key: v.key,
                    value: v.value,
                })
                .collect(),
            packages: profile
                .packages
                .into_iter()
                .map(|p| PackageBody {
                    name: p.name,
                    version: p.version,
                })
                .collect(),
            agent: AgentView {
                kernel: profile.agent.kernel.0,
                mode: profile.agent.mode,
                model: profile.agent.model,
                reasoning_effort: profile.agent.reasoning_effort,
                permission: profile.agent.permission,
                credential: profile.agent.credential,
                size: profile.agent.size,
                record_tool_bodies: profile.agent.record_tool_bodies,
                skills: profile.agent.skills.into_iter().map(|s| s.0).collect(),
            },
            nyxid: NyxIdView {
                services: profile.services.into_iter().map(|s| s.0).collect(),
            },
            agent_notes: profile.agent_notes,
            mandatory: MandatoryView::of(service.baseline()),
            created_at: profile.created_at,
            updated_at: profile.updated_at,
            preferred: profile.preferred,
        }
    }
}

/// What a client needs to render the profile editor before it has a profile.
///
/// The mandated entries ride along with the catalogue rather than only with a
/// stored profile: a client authoring its *first* profile has no profile to
/// read them from, and would otherwise offer a form that quietly acquires
/// entries the moment it is saved.
#[derive(Debug, Serialize)]
pub struct SkillCatalogView {
    pub(super) skills: Vec<SkillView>,
    pub(super) mandatory: MandatoryView,
}

/// A selectable skill as this interface renders it.
#[derive(Debug, Serialize)]
pub struct SkillView {
    id: String,
    name: String,
    description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    access: Option<String>,
}

impl From<CatalogSkill> for SkillView {
    fn from(s: CatalogSkill) -> Self {
        Self {
            id: s.id.0,
            name: s.name,
            description: s.description,
            access: s.access,
        }
    }
}
