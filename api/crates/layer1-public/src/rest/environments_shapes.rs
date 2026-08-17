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
    AgentConfiguration, CatalogSkill, CredentialMode, EnvironmentProfile,
    EnvironmentProfileService, EnvironmentProfileSpec, EnvironmentVariable, KernelId,
    PermissionMode, ReasoningEffort, ServiceId, SessionMode, SkillId, SoftwarePackage,
};
use serde::{Deserialize, Serialize};

/// A variable as this interface reads and writes it.
#[derive(Debug, Deserialize, Serialize)]
pub struct VariableBody {
    /// Variable name.
    pub key: String,
    /// Value. Opaque to every layer that carries it.
    pub value: String,
}

/// A package as this interface reads and writes it.
#[derive(Debug, Deserialize, Serialize)]
pub struct PackageBody {
    /// Package name.
    pub name: String,
    /// Version to pin, when the caller cares which one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// Request body for creating or replacing a profile.
#[derive(Debug, Deserialize)]
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
}

/// The platform-access half of a profile, as a caller supplies it.
#[derive(Debug, Default, Deserialize)]
pub struct NyxIdBody {
    /// Identifiers of the services to allow.
    #[serde(default)]
    pub services: Vec<String>,
}

/// The agent half of a profile, as a caller supplies it.
///
/// Every field defaults, so a client that cares about none of them sends a
/// name and gets a profile that behaves the way the deployment says it should.
#[derive(Debug, Default, Deserialize)]
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
    /// Skill identifiers to load.
    #[serde(default)]
    pub skills: Vec<String>,
}

impl From<ProfileRequest> for EnvironmentProfileSpec {
    fn from(body: ProfileRequest) -> Self {
        Self {
            name: body.name,
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
                credential: body.agent.credential.unwrap_or_default(),
                skills: body.agent.skills.into_iter().map(SkillId::new).collect(),
            },
            services: body
                .nyxid
                .services
                .into_iter()
                .map(ServiceId::new)
                .collect(),
        }
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
    /// Which entries the platform mandates, so a client can render them as
    /// fixed rather than guessing — and so the rule is visible rather than
    /// only enforced when the client gets it wrong.
    mandatory: MandatoryView,
    created_at: String,
    updated_at: String,
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
                skills: profile.agent.skills.into_iter().map(|s| s.0).collect(),
            },
            nyxid: NyxIdView {
                services: profile.services.into_iter().map(|s| s.0).collect(),
            },
            mandatory: MandatoryView::of(service.baseline()),
            created_at: profile.created_at,
            updated_at: profile.updated_at,
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
