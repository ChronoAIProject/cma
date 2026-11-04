//! Deployment-owned policy for the Sandbox model.

use serde::{Deserialize, Serialize};

use crate::profiles::{AgentProfileRef, SandboxProfileRef, WorkspaceProfileRef};
use crate::{AgentProfileId, CoreError, SandboxProfileId, WorkspaceProfileId};

// Versioned identities preserve historical published defaults during rollout.
pub const DEFAULT_SANDBOX_PROFILE_ID: &str = "sbp_default_v2";
pub const DEFAULT_WORKSPACE_PROFILE_ID: &str = "wkp_default_v2";
pub const DEFAULT_AGENT_PROFILE_ID: &str = "agp_default_v2";
pub const DEFAULT_PROFILE_REVISION: u32 = 1;
pub const MIN_PAUSE_AFTER_SECONDS: u64 = 60;
pub const MAX_PAUSE_AFTER_SECONDS: u64 = 86_400;
pub const DEFAULT_CHECKPOINT_INTERVAL_SECONDS: u64 = 30 * 60;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckpointSettings {
    pub interval_seconds: u64,
}

impl Default for CheckpointSettings {
    fn default() -> Self {
        Self {
            interval_seconds: DEFAULT_CHECKPOINT_INTERVAL_SECONDS,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlatformDefaults {
    pub sandbox_profile: SandboxProfileRef,
    pub workspace_profile: WorkspaceProfileRef,
    pub agent_profile: AgentProfileRef,
}

impl Default for PlatformDefaults {
    fn default() -> Self {
        Self {
            sandbox_profile: SandboxProfileRef::new(
                SandboxProfileId(DEFAULT_SANDBOX_PROFILE_ID.into()),
                DEFAULT_PROFILE_REVISION,
            ),
            workspace_profile: WorkspaceProfileRef::new(
                WorkspaceProfileId(DEFAULT_WORKSPACE_PROFILE_ID.into()),
                DEFAULT_PROFILE_REVISION,
            ),
            agent_profile: AgentProfileRef::new(
                AgentProfileId(DEFAULT_AGENT_PROFILE_ID.into()),
                DEFAULT_PROFILE_REVISION,
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InactivitySettings {
    /// Global automatic pause admission. Kept off for backwards-compatible rollout.
    #[serde(default)]
    pub auto_pause_enabled: bool,
    pub pause_after_seconds: u64,
    /// The instant from which the current policy is allowed to pause. This is
    /// written when enablement or the threshold changes, so existing Sandboxes
    /// receive a complete grace period after an operator change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grace_started_at: Option<String>,
}

impl Default for InactivitySettings {
    fn default() -> Self {
        Self {
            auto_pause_enabled: false,
            pause_after_seconds: 900,
            grace_started_at: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetentionSettings {
    pub checkpoint_grace_days: u32,
    pub agent_log_days: u32,
}

impl Default for RetentionSettings {
    fn default() -> Self {
        Self {
            checkpoint_grace_days: 7,
            agent_log_days: 90,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlatformLimits {
    pub max_sandboxes_per_user: u32,
    pub visitor_max_sandboxes: u32,
    pub visitor_max_launches_per_day: u32,
}

impl Default for PlatformLimits {
    fn default() -> Self {
        Self {
            max_sandboxes_per_user: 10,
            visitor_max_sandboxes: 1,
            visitor_max_launches_per_day: 5,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TriggerSettings {
    pub enabled: bool,
    pub orchestration_paused: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MarketplaceSettings {
    pub enabled: bool,
}

impl Default for MarketplaceSettings {
    fn default() -> Self {
        Self { enabled: true }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlatformSettings {
    pub defaults: PlatformDefaults,
    pub inactivity: InactivitySettings,
    #[serde(default)]
    pub checkpoints: CheckpointSettings,
    pub retention: RetentionSettings,
    pub limits: PlatformLimits,
    pub triggers: TriggerSettings,
    pub marketplace: MarketplaceSettings,
}

impl PlatformSettings {
    pub fn validate(&self) -> Result<(), CoreError> {
        if !(60..=86_400).contains(&self.checkpoints.interval_seconds) {
            return Err(CoreError::InvalidRequest(
                "checkpoints.interval_seconds must be between 60 and 86400".into(),
            ));
        }
        if !(MIN_PAUSE_AFTER_SECONDS..=MAX_PAUSE_AFTER_SECONDS)
            .contains(&self.inactivity.pause_after_seconds)
        {
            return Err(CoreError::InvalidRequest(format!(
                "inactivity.pause_after_seconds must be between {MIN_PAUSE_AFTER_SECONDS} and {MAX_PAUSE_AFTER_SECONDS}"
            )));
        }
        if let Some(started_at) = &self.inactivity.grace_started_at {
            if crate::seconds_from_rfc3339(started_at).is_none() {
                return Err(CoreError::InvalidRequest(
                    "inactivity.grace_started_at must be RFC3339".into(),
                ));
            }
        }
        for (field, revision) in [
            (
                "defaults.sandbox_profile.revision",
                self.defaults.sandbox_profile.revision,
            ),
            (
                "defaults.workspace_profile.revision",
                self.defaults.workspace_profile.revision,
            ),
            (
                "defaults.agent_profile.revision",
                self.defaults.agent_profile.revision,
            ),
        ] {
            if revision == 0 {
                return Err(CoreError::InvalidRequest(format!(
                    "{field} must be greater than zero"
                )));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    use super::*;

    #[test]
    fn shipped_settings_match_the_platform_contract() {
        let settings = PlatformSettings::default();
        assert!(settings.validate().is_ok());
        assert_eq!(
            serde_json::to_value(settings).unwrap(),
            json!({
                "defaults": {
                    "sandbox_profile": {"id": "sbp_default_v2", "revision": 1},
                    "workspace_profile": {"id": "wkp_default_v2", "revision": 1},
                    "agent_profile": {"id": "agp_default_v2", "revision": 1}
                },
                "inactivity": {"auto_pause_enabled": false, "pause_after_seconds": 900},
                "checkpoints": {"interval_seconds": 1800},
                "retention": {"checkpoint_grace_days": 7, "agent_log_days": 90},
                "limits": {
                    "max_sandboxes_per_user": 10,
                    "visitor_max_sandboxes": 1,
                    "visitor_max_launches_per_day": 5
                },
                "triggers": {"enabled": false, "orchestration_paused": false},
                "marketplace": {"enabled": true}
            })
        );
    }

    #[test]
    fn inactivity_accepts_only_the_documented_inclusive_range() {
        for seconds in [MIN_PAUSE_AFTER_SECONDS, MAX_PAUSE_AFTER_SECONDS] {
            let mut settings = PlatformSettings::default();
            settings.inactivity.pause_after_seconds = seconds;
            assert!(settings.validate().is_ok(), "refused {seconds}");
        }
        for seconds in [MIN_PAUSE_AFTER_SECONDS - 1, MAX_PAUSE_AFTER_SECONDS + 1] {
            let mut settings = PlatformSettings::default();
            settings.inactivity.pause_after_seconds = seconds;
            let error = settings.validate().unwrap_err().to_string();
            assert!(error.contains("inactivity.pause_after_seconds"));
        }
    }

    #[test]
    fn checkpoint_interval_defaults_legacy_settings_and_validates_bounds() {
        let mut legacy = serde_json::to_value(PlatformSettings::default()).unwrap();
        legacy.as_object_mut().unwrap().remove("checkpoints");
        let mut settings: PlatformSettings = serde_json::from_value(legacy).unwrap();
        assert_eq!(settings.checkpoints.interval_seconds, 1800);
        for seconds in [60, 1800, 86_400] {
            settings.checkpoints.interval_seconds = seconds;
            settings.validate().unwrap();
        }
        for seconds in [0, 59, 86_401, u64::MAX] {
            settings.checkpoints.interval_seconds = seconds;
            assert!(settings
                .validate()
                .unwrap_err()
                .to_string()
                .contains("checkpoints.interval_seconds"));
        }
    }

    #[test]
    fn every_default_profile_must_name_a_published_revision() {
        let mut settings = PlatformSettings::default();
        settings.defaults.sandbox_profile.revision = 0;
        assert!(settings
            .validate()
            .unwrap_err()
            .to_string()
            .contains("defaults.sandbox_profile.revision"));

        let mut settings = PlatformSettings::default();
        settings.defaults.workspace_profile.revision = 0;
        assert!(settings
            .validate()
            .unwrap_err()
            .to_string()
            .contains("defaults.workspace_profile.revision"));

        let mut settings = PlatformSettings::default();
        settings.defaults.agent_profile.revision = 0;
        assert!(settings
            .validate()
            .unwrap_err()
            .to_string()
            .contains("defaults.agent_profile.revision"));
    }

    #[test]
    fn unknown_fields_are_named_without_echoing_their_values() {
        let original = serde_json::to_value(PlatformSettings::default()).unwrap();
        for path in [
            &[][..],
            &["defaults"][..],
            &["inactivity"][..],
            &["checkpoints"][..],
            &["retention"][..],
            &["limits"][..],
            &["triggers"][..],
            &["marketplace"][..],
        ] {
            let mut candidate = original.clone();
            object_at_mut(&mut candidate, path).insert(
                "unrecognized_control".into(),
                Value::String("sensitive-value-must-not-appear".into()),
            );
            let error = serde_json::from_value::<PlatformSettings>(candidate)
                .unwrap_err()
                .to_string();
            assert!(error.contains("unrecognized_control"), "{error}");
            assert!(
                !error.contains("sensitive-value-must-not-appear"),
                "{error}"
            );
        }
    }

    fn object_at_mut<'a>(
        value: &'a mut Value,
        path: &[&str],
    ) -> &'a mut serde_json::Map<String, Value> {
        let mut current = value;
        for segment in path {
            current = current.get_mut(*segment).unwrap();
        }
        current.as_object_mut().unwrap()
    }
}
