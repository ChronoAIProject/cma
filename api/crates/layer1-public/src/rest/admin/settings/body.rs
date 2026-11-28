//! Wire models and confidential decoding for platform settings requests.

use axum::{extract::FromRequest, extract::Request, Json};
use layer2_core::profiles::{AgentProfileRef, SandboxProfileRef, WorkspaceProfileRef};
use layer2_core::{
    platform::UsageReportingSettings, AgentProfileId, InactivitySettings, MarketplaceSettings,
    PlatformDefaults, PlatformLimits, PlatformSettings, RetentionSettings, SandboxProfileId,
    TriggerSettings, WorkspaceProfileId,
};
use serde::{de::DeserializeOwned, Deserialize, Deserializer, Serialize};
use utoipa::ToSchema;

use super::budget::BudgetSettingsBody;
use super::Failure;

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProfileReferenceBody {
    id: String,
    #[schema(minimum = 1)]
    revision: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct PlatformDefaultsBody {
    sandbox_profile: ProfileReferenceBody,
    workspace_profile: ProfileReferenceBody,
    agent_profile: ProfileReferenceBody,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct InactivitySettingsBody {
    #[serde(default)]
    auto_pause_enabled: bool,
    #[schema(minimum = 60, maximum = 86400)]
    pause_after_seconds: u64,
    #[serde(default)]
    grace_started_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct RetentionSettingsBody {
    agent_log_days: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct PlatformLimitsBody {
    max_sandboxes_per_user: u32,
    visitor_max_sandboxes: u32,
    visitor_max_launches_per_day: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct TriggerSettingsBody {
    enabled: bool,
    orchestration_paused: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct MarketplaceSettingsBody {
    enabled: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct UsageReportingSettingsBody {
    measured_visible: bool,
}

impl From<UsageReportingSettingsBody> for UsageReportingSettings {
    fn from(value: UsageReportingSettingsBody) -> Self {
        Self {
            measured_visible: value.measured_visible,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct PlatformSettingsBody {
    #[serde(default, deserialize_with = "super::budget::present")]
    model_budget: Option<BudgetSettingsBody>,
    #[serde(default)]
    usage_reporting: UsageReportingSettingsBody,
    defaults: PlatformDefaultsBody,
    inactivity: InactivitySettingsBody,
    retention: RetentionSettingsBody,
    limits: PlatformLimitsBody,
    triggers: TriggerSettingsBody,
    marketplace: MarketplaceSettingsBody,
}

impl From<PlatformSettingsBody> for PlatformSettings {
    fn from(value: PlatformSettingsBody) -> Self {
        Self {
            model_budget: value.model_budget.map(Into::into).unwrap_or_default(),
            usage_reporting: value.usage_reporting.into(),
            defaults: value.defaults.into(),
            inactivity: value.inactivity.into(),
            checkpoints: Default::default(),
            retention: value.retention.into(),
            limits: value.limits.into(),
            triggers: value.triggers.into(),
            marketplace: value.marketplace.into(),
        }
    }
}

impl PlatformSettingsBody {
    pub(super) fn has_model_budget(&self) -> bool {
        self.model_budget.is_some()
    }
}

impl From<PlatformSettings> for PlatformSettingsBody {
    fn from(value: PlatformSettings) -> Self {
        Self {
            model_budget: Some(value.model_budget.into()),
            usage_reporting: UsageReportingSettingsBody {
                measured_visible: value.usage_reporting.measured_visible,
            },
            defaults: PlatformDefaultsBody {
                sandbox_profile: ProfileReferenceBody {
                    id: value.defaults.sandbox_profile.id.0,
                    revision: value.defaults.sandbox_profile.revision,
                },
                workspace_profile: ProfileReferenceBody {
                    id: value.defaults.workspace_profile.id.0,
                    revision: value.defaults.workspace_profile.revision,
                },
                agent_profile: ProfileReferenceBody {
                    id: value.defaults.agent_profile.id.0,
                    revision: value.defaults.agent_profile.revision,
                },
            },
            inactivity: InactivitySettingsBody {
                auto_pause_enabled: value.inactivity.auto_pause_enabled,
                pause_after_seconds: value.inactivity.pause_after_seconds,
                grace_started_at: value.inactivity.grace_started_at,
            },
            retention: RetentionSettingsBody {
                agent_log_days: value.retention.agent_log_days,
            },
            limits: PlatformLimitsBody {
                max_sandboxes_per_user: value.limits.max_sandboxes_per_user,
                visitor_max_sandboxes: value.limits.visitor_max_sandboxes,
                visitor_max_launches_per_day: value.limits.visitor_max_launches_per_day,
            },
            triggers: TriggerSettingsBody {
                enabled: value.triggers.enabled,
                orchestration_paused: value.triggers.orchestration_paused,
            },
            marketplace: MarketplaceSettingsBody {
                enabled: value.marketplace.enabled,
            },
        }
    }
}

impl From<PlatformDefaultsBody> for PlatformDefaults {
    fn from(value: PlatformDefaultsBody) -> Self {
        Self {
            sandbox_profile: SandboxProfileRef::new(
                SandboxProfileId(value.sandbox_profile.id),
                value.sandbox_profile.revision,
            ),
            workspace_profile: WorkspaceProfileRef::new(
                WorkspaceProfileId(value.workspace_profile.id),
                value.workspace_profile.revision,
            ),
            agent_profile: AgentProfileRef::new(
                AgentProfileId(value.agent_profile.id),
                value.agent_profile.revision,
            ),
        }
    }
}

impl From<InactivitySettingsBody> for InactivitySettings {
    fn from(value: InactivitySettingsBody) -> Self {
        Self {
            auto_pause_enabled: value.auto_pause_enabled,
            pause_after_seconds: value.pause_after_seconds,
            grace_started_at: value.grace_started_at,
        }
    }
}

impl From<RetentionSettingsBody> for RetentionSettings {
    fn from(value: RetentionSettingsBody) -> Self {
        Self {
            checkpoint_grace_days: RetentionSettings::default().checkpoint_grace_days,
            agent_log_days: value.agent_log_days,
        }
    }
}

impl From<PlatformLimitsBody> for PlatformLimits {
    fn from(value: PlatformLimitsBody) -> Self {
        Self {
            max_sandboxes_per_user: value.max_sandboxes_per_user,
            visitor_max_sandboxes: value.visitor_max_sandboxes,
            visitor_max_launches_per_day: value.visitor_max_launches_per_day,
        }
    }
}

impl From<TriggerSettingsBody> for TriggerSettings {
    fn from(value: TriggerSettingsBody) -> Self {
        Self {
            enabled: value.enabled,
            orchestration_paused: value.orchestration_paused,
        }
    }
}

impl From<MarketplaceSettingsBody> for MarketplaceSettings {
    fn from(value: MarketplaceSettingsBody) -> Self {
        Self {
            enabled: value.enabled,
        }
    }
}

#[derive(Debug, Default)]
enum PatchField<T> {
    #[default]
    Missing,
    Null,
    Value(T),
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for PatchField<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(match Option::<T>::deserialize(deserializer)? {
            Some(value) => Self::Value(value),
            None => Self::Null,
        })
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReplaceRequest {
    pub(super) expected_revision: u64,
    pub(super) settings: PlatformSettingsBody,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct PatchRequest {
    #[serde(default)]
    #[schema(required = false, value_type = BudgetSettingsBody)]
    model_budget: PatchField<BudgetSettingsBody>,
    #[serde(default)]
    #[schema(required = false, value_type = UsageReportingSettingsBody)]
    usage_reporting: PatchField<UsageReportingSettingsBody>,
    pub(super) expected_revision: u64,
    #[serde(default)]
    #[schema(required = false, value_type = PlatformDefaultsBody)]
    defaults: PatchField<PlatformDefaultsBody>,
    #[serde(default)]
    #[schema(required = false, value_type = InactivitySettingsBody)]
    inactivity: PatchField<InactivitySettingsBody>,
    #[serde(default)]
    #[schema(required = false, value_type = RetentionSettingsBody)]
    retention: PatchField<RetentionSettingsBody>,
    #[serde(default)]
    #[schema(required = false, value_type = PlatformLimitsBody)]
    limits: PatchField<PlatformLimitsBody>,
    #[serde(default)]
    #[schema(required = false, value_type = TriggerSettingsBody)]
    triggers: PatchField<TriggerSettingsBody>,
    #[serde(default)]
    #[schema(required = false, value_type = MarketplaceSettingsBody)]
    marketplace: PatchField<MarketplaceSettingsBody>,
}

impl PatchRequest {
    pub(super) fn apply(
        self,
        mut settings: PlatformSettings,
    ) -> Result<(u64, PlatformSettings), Failure> {
        let mut changed = false;
        changed |= apply(
            self.model_budget,
            &mut settings.model_budget,
            "model_budget",
        )?;
        changed |= apply(
            self.usage_reporting,
            &mut settings.usage_reporting,
            "usage_reporting",
        )?;
        changed |= apply(self.defaults, &mut settings.defaults, "defaults")?;
        changed |= apply(self.inactivity, &mut settings.inactivity, "inactivity")?;
        changed |= apply(self.retention, &mut settings.retention, "retention")?;
        changed |= apply(self.limits, &mut settings.limits, "limits")?;
        changed |= apply(self.triggers, &mut settings.triggers, "triggers")?;
        changed |= apply(self.marketplace, &mut settings.marketplace, "marketplace")?;
        if !changed {
            return Err(Failure::Invalid(None));
        }
        Ok((self.expected_revision, settings))
    }
}

fn apply<T: Into<U>, U>(
    field: PatchField<T>,
    target: &mut U,
    path: &'static str,
) -> Result<bool, Failure> {
    match field {
        PatchField::Missing => Ok(false),
        PatchField::Null => Err(Failure::Invalid(Some((path.into(), path.into())))),
        PatchField::Value(value) => {
            *target = value.into();
            Ok(true)
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ResetRequest {
    pub(super) expected_revision: u64,
}

struct FieldSpec {
    name: &'static str,
    children: &'static [FieldSpec],
}

const REF: &[FieldSpec] = &[leaf("id"), leaf("revision")];
const DEFAULTS: &[FieldSpec] = &[
    branch("sandbox_profile", REF),
    branch("workspace_profile", REF),
    branch("agent_profile", REF),
];
const INACTIVITY: &[FieldSpec] = &[
    leaf("auto_pause_enabled"),
    leaf("pause_after_seconds"),
    leaf("grace_started_at"),
];
const RETENTION: &[FieldSpec] = &[leaf("agent_log_days")];
const LIMITS: &[FieldSpec] = &[
    leaf("max_sandboxes_per_user"),
    leaf("visitor_max_sandboxes"),
    leaf("visitor_max_launches_per_day"),
];
const TRIGGERS: &[FieldSpec] = &[leaf("enabled"), leaf("orchestration_paused")];
const MARKETPLACE: &[FieldSpec] = &[leaf("enabled")];
const USAGE_REPORTING: &[FieldSpec] = &[leaf("measured_visible")];
const MODEL_BUDGET: &[FieldSpec] = &[
    leaf("request_models"),
    leaf("enabled"),
    leaf("platform"),
    leaf("owners"),
    leaf("default_policy"),
];
const PLATFORM: &[FieldSpec] = &[
    branch("model_budget", MODEL_BUDGET),
    branch("usage_reporting", USAGE_REPORTING),
    branch("defaults", DEFAULTS),
    branch("inactivity", INACTIVITY),
    branch("retention", RETENTION),
    branch("limits", LIMITS),
    branch("triggers", TRIGGERS),
    branch("marketplace", MARKETPLACE),
];
const REPLACE: &[FieldSpec] = &[leaf("expected_revision"), branch("settings", PLATFORM)];
const PATCH: &[FieldSpec] = &[
    branch("model_budget", MODEL_BUDGET),
    branch("usage_reporting", USAGE_REPORTING),
    leaf("expected_revision"),
    branch("defaults", DEFAULTS),
    branch("inactivity", INACTIVITY),
    branch("retention", RETENTION),
    branch("limits", LIMITS),
    branch("triggers", TRIGGERS),
    branch("marketplace", MARKETPLACE),
];
const RESET: &[FieldSpec] = &[leaf("expected_revision")];

const fn leaf(name: &'static str) -> FieldSpec {
    FieldSpec {
        name,
        children: &[],
    }
}

const fn branch(name: &'static str, children: &'static [FieldSpec]) -> FieldSpec {
    FieldSpec { name, children }
}

fn unknown(
    value: &serde_json::Value,
    prefix: &str,
    specs: &[FieldSpec],
) -> Option<(String, String)> {
    let object = value.as_object()?;
    for (name, child) in object {
        let path = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}.{name}")
        };
        let Some(spec) = specs.iter().find(|spec| spec.name == name) else {
            return Some((name.clone(), path));
        };
        if !spec.children.is_empty() {
            if let Some(found) = unknown(child, &path, spec.children) {
                return Some(found);
            }
        }
    }
    None
}

trait SettingsRequest: DeserializeOwned {
    const FIELDS: &'static [FieldSpec];
}

impl SettingsRequest for ReplaceRequest {
    const FIELDS: &'static [FieldSpec] = REPLACE;
}
impl SettingsRequest for PatchRequest {
    const FIELDS: &'static [FieldSpec] = PATCH;
}
impl SettingsRequest for ResetRequest {
    const FIELDS: &'static [FieldSpec] = RESET;
}

pub(crate) struct SettingsJson<T>(pub(crate) T);

impl<S, T> FromRequest<S> for SettingsJson<T>
where
    S: Send + Sync,
    T: SettingsRequest,
{
    type Rejection = Failure;

    async fn from_request(request: Request, state: &S) -> Result<Self, Self::Rejection> {
        let Json(value) = Json::<serde_json::Value>::from_request(request, state)
            .await
            .map_err(Failure::body)?;
        if let Some(field) = unknown(&value, "", T::FIELDS) {
            return Err(Failure::Invalid(Some(field)));
        }
        serde_json::from_value(value)
            .map(Self)
            .map_err(|_| Failure::Invalid(None))
    }
}
