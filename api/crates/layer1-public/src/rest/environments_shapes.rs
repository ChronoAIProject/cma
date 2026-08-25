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
use serde_json::{Map, Value};
use utoipa::ToSchema;

use super::openapi_shapes;

const MAX_DIAGNOSTIC_FIELD_BYTES: usize = 64;

const PROFILE_FIELDS: &[&str] = &[
    "name",
    "variables",
    "packages",
    "agent",
    "nyxid",
    "agent_notes",
];
const PROFILE_VIEW_FIELDS: &[&str] = &[
    "name",
    "variables",
    "packages",
    "agent",
    "nyxid",
    "agent_notes",
    "id",
    "mandatory",
    "created_at",
    "updated_at",
    "preferred",
];
const PROFILE_SERVER_FIELDS: &[&str] =
    &["id", "mandatory", "created_at", "updated_at", "preferred"];
const VARIABLE_FIELDS: &[&str] = &["key", "value"];
const PACKAGE_FIELDS: &[&str] = &["name", "version"];
const AGENT_FIELDS: &[&str] = &[
    "kernel",
    "mode",
    "model",
    "reasoning_effort",
    "permission",
    "credential",
    "size",
    "record_tool_bodies",
    "skills",
];
const SIZE_FIELDS: &[&str] = &["cpu_millis", "memory_mib"];
const NYXID_FIELDS: &[&str] = &["services"];
const MANDATORY_FIELDS: &[&str] = &["packages", "skills"];

/// A variable as this interface reads and writes it.
#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct VariableBody {
    /// Variable name.
    pub key: String,
    /// Value. Opaque to every layer that carries it.
    pub value: String,
}

/// A package as this interface reads and writes it.
#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PackageBody {
    /// Package name.
    pub name: String,
    /// Version to pin, when the caller cares which one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// Request body for creating or replacing a profile.
#[derive(Debug, Deserialize, Serialize, ToSchema)]
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
#[derive(Debug, Default, Deserialize, Serialize, ToSchema)]
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
#[derive(Debug, Default, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentBody {
    /// Which runtime implementation backs the session.
    #[serde(default)]
    pub kernel: Option<String>,
    /// Which plane sessions from this profile are driven on.
    #[serde(default)]
    #[schema(value_type = Option<openapi_shapes::SessionMode>)]
    pub mode: Option<SessionMode>,
    /// Model the agent selects.
    #[serde(default)]
    pub model: Option<String>,
    /// How hard it thinks.
    #[serde(default)]
    #[schema(value_type = Option<openapi_shapes::ReasoningEffort>)]
    pub reasoning_effort: Option<ReasoningEffort>,
    /// How often it stops to ask.
    #[serde(default)]
    #[schema(value_type = Option<openapi_shapes::PermissionMode>)]
    pub permission: Option<PermissionMode>,
    /// Whose model access it spends.
    #[serde(default)]
    #[schema(value_type = Option<openapi_shapes::CredentialMode>)]
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
#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SizeBody {
    pub cpu_millis: u32,
    pub memory_mib: u32,
}

/// Schema metadata safe to add to an unknown-field problem document.
///
/// JSON object keys are still caller input. Only a short identifier-shaped key
/// is returned; arbitrary prose, control characters, and unbounded keys stay
/// behind the same fixed-detail refusal as every other invalid body.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct UnknownProfileField {
    pub(super) field: Option<String>,
    pub(super) path: Option<String>,
}

/// Locate an unknown field structurally, without inspecting or formatting any
/// submitted scalar value.
pub(super) fn unknown_profile_field(
    body: &Value,
    accepts_profile_view: bool,
) -> Option<UnknownProfileField> {
    let profile = body.as_object()?;
    let accepted = if accepts_profile_view {
        PROFILE_VIEW_FIELDS
    } else {
        PROFILE_FIELDS
    };
    if let Some(unknown) = unknown_in(profile, accepted, "") {
        return Some(unknown);
    }

    for (index, variable) in profile
        .get("variables")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        if let Some(unknown) = variable
            .as_object()
            .and_then(|object| unknown_in(object, VARIABLE_FIELDS, &format!("variables[{index}]")))
        {
            return Some(unknown);
        }
    }

    for (index, package) in profile
        .get("packages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        if let Some(unknown) = package
            .as_object()
            .and_then(|object| unknown_in(object, PACKAGE_FIELDS, &format!("packages[{index}]")))
        {
            return Some(unknown);
        }
    }

    if let Some(agent) = profile.get("agent").and_then(Value::as_object) {
        if let Some(unknown) = unknown_in(agent, AGENT_FIELDS, "agent") {
            return Some(unknown);
        }
        if let Some(unknown) = agent
            .get("size")
            .and_then(Value::as_object)
            .and_then(|size| unknown_in(size, SIZE_FIELDS, "agent.size"))
        {
            return Some(unknown);
        }
    }

    if accepts_profile_view {
        if let Some(unknown) = profile
            .get("mandatory")
            .and_then(Value::as_object)
            .and_then(|mandatory| unknown_in(mandatory, MANDATORY_FIELDS, "mandatory"))
        {
            return Some(unknown);
        }
    }

    profile
        .get("nyxid")
        .and_then(Value::as_object)
        .and_then(|nyxid| unknown_in(nyxid, NYXID_FIELDS, "nyxid"))
}

/// Decode a create body or the writable contents of a replacement body.
///
/// A replacement may be the complete `ProfileView` returned by GET. Its five
/// server-owned fields are checked as part of that closed schema and removed
/// before the writable request is decoded; they can therefore round-trip but
/// can never overwrite identity, timestamps, the deployment baseline, or the
/// current preference.
pub(super) fn decode_profile_request(
    mut body: Value,
    accepts_profile_view: bool,
) -> Result<(ProfileRequest, Option<String>), serde_json::Error> {
    let echoed_id = if accepts_profile_view {
        let mut server_fields = Map::new();
        if let Some(profile) = body.as_object_mut() {
            for field in PROFILE_SERVER_FIELDS {
                if let Some(value) = profile.remove(*field) {
                    server_fields.insert((*field).to_owned(), value);
                }
            }
        }
        serde_json::from_value::<ProfileViewEcho>(Value::Object(server_fields))?.into_id()
    } else {
        None
    };
    Ok((serde_json::from_value(body)?, echoed_id))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileViewEcho {
    #[serde(default, deserialize_with = "present_non_null")]
    id: Option<String>,
    #[serde(default, deserialize_with = "present_non_null")]
    mandatory: Option<MandatoryEcho>,
    #[serde(default, deserialize_with = "present_non_null")]
    created_at: Option<String>,
    #[serde(default, deserialize_with = "present_non_null")]
    updated_at: Option<String>,
    #[serde(default, deserialize_with = "present_non_null")]
    preferred: Option<bool>,
}

fn present_non_null<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

impl ProfileViewEcho {
    fn into_id(self) -> Option<String> {
        let Self {
            id,
            mandatory,
            created_at,
            updated_at,
            preferred,
        } = self;
        if let Some(MandatoryEcho { packages, skills }) = mandatory {
            drop((packages, skills));
        }
        drop((created_at, updated_at, preferred));
        id
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MandatoryEcho {
    packages: Vec<String>,
    skills: Vec<String>,
}

fn unknown_in(
    object: &Map<String, Value>,
    accepted: &[&str],
    parent: &str,
) -> Option<UnknownProfileField> {
    object
        .keys()
        .find(|key| !accepted.contains(&key.as_str()))
        .map(|field| {
            if reportable_field(field) {
                let path = if parent.is_empty() {
                    field.clone()
                } else {
                    format!("{parent}.{field}")
                };
                UnknownProfileField {
                    field: Some(field.clone()),
                    path: Some(path),
                }
            } else {
                UnknownProfileField {
                    field: None,
                    path: None,
                }
            }
        })
}

fn reportable_field(field: &str) -> bool {
    !field.is_empty()
        && field.len() <= MAX_DIAGNOSTIC_FIELD_BYTES
        && field
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
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
#[derive(Debug, Serialize, ToSchema)]
#[schema(as = ProfileAgentView)]
pub struct AgentView {
    kernel: String,
    #[schema(value_type = openapi_shapes::SessionMode)]
    mode: SessionMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    #[schema(value_type = openapi_shapes::ReasoningEffort)]
    reasoning_effort: ReasoningEffort,
    #[schema(value_type = openapi_shapes::PermissionMode)]
    permission: PermissionMode,
    #[schema(value_type = openapi_shapes::CredentialMode)]
    credential: CredentialMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<openapi_shapes::SessionSize>)]
    size: Option<SessionSize>,
    record_tool_bodies: bool,
    skills: Vec<String>,
}

/// The platform-access half of a profile, as this interface renders it.
#[derive(Debug, Serialize, ToSchema)]
pub struct NyxIdView {
    services: Vec<String>,
}

/// A profile as this interface renders it.
#[derive(Debug, Serialize, ToSchema)]
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
    /// Whether clients should preselect this profile for its owner. At most one
    /// of theirs carries it; the server does not use it for session creation.
    preferred: bool,
}

/// The parts of a profile a caller may not remove.
#[derive(Debug, Default, Serialize, ToSchema)]
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
#[derive(Debug, Serialize, ToSchema)]
pub struct SkillCatalogView {
    pub(super) skills: Vec<SkillView>,
    pub(super) mandatory: MandatoryView,
}

/// A selectable skill as this interface renders it.
#[derive(Debug, Serialize, ToSchema)]
#[schema(as = CatalogSkillView)]
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
