//! Revision-CAS platform settings, retaining unknown additive fields on round trips.

use crate::{Error, Result, profiles::ProfileReference};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Typed settings section with additive server fields preserved on replacement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Section<T> {
    /// Known section fields.
    #[serde(flatten)]
    pub values: T,
    /// Unknown additive fields returned by a newer server.
    #[serde(flatten)]
    pub extensions: BTreeMap<String, serde_json::Value>,
}

impl<T> Section<T> {
    /// Construct a section with no additional server fields.
    pub fn new(values: T) -> Self {
        Self {
            values,
            extensions: BTreeMap::new(),
        }
    }
}

/// Deployment profile choices, retaining additive fields on each reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingsDefaults {
    /// Default Sandbox profile revision.
    pub sandbox_profile: Section<ProfileReference>,
    /// Default Workspace profile revision.
    pub workspace_profile: Section<ProfileReference>,
    /// Default Agent profile revision.
    pub agent_profile: Section<ProfileReference>,
}

/// Sandbox inactivity policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Inactivity {
    /// Idle threshold from 60 through 86400 seconds.
    pub pause_after_seconds: u32,
}

/// Periodic snapshot capture policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checkpoints {
    /// Minimum age between completed snapshots, from 60 through 86400 seconds.
    pub interval_seconds: u32,
}

/// Durable checkpoint and conversation retention.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Retention {
    /// Checkpoint retirement grace period in days.
    pub checkpoint_grace_days: u32,
    /// Agent durable log retention in days.
    pub agent_log_days: u32,
}

/// Count-based admission limits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Limits {
    /// Maximum owned Sandboxes per ordinary user.
    pub max_sandboxes_per_user: u32,
    /// Maximum owned Sandboxes per visitor.
    pub visitor_max_sandboxes: u32,
    /// Maximum daily Launches per visitor.
    pub visitor_max_launches_per_day: u32,
}

/// Trigger admission and worker controls.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriggerSettings {
    /// Whether new Trigger Launches are admitted.
    pub enabled: bool,
    /// Whether orchestration of admitted Launches is paused.
    pub orchestration_paused: bool,
}

/// Marketplace availability.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarketplaceSettings {
    /// Whether marketplace reads and forks are available.
    pub enabled: bool,
}

/// Complete platform settings with additive fields retained at every boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlatformSettings {
    /// Default profiles.
    pub defaults: Section<SettingsDefaults>,
    /// Inactivity controls.
    pub inactivity: Section<Inactivity>,
    /// Snapshot interval, absent only on older servers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoints: Option<Section<Checkpoints>>,
    /// Retention controls.
    pub retention: Section<Retention>,
    /// Admission limits.
    pub limits: Section<Limits>,
    /// Trigger controls.
    pub triggers: Section<TriggerSettings>,
    /// Marketplace controls.
    pub marketplace: Section<MarketplaceSettings>,
    /// Unknown additive top-level settings.
    #[serde(flatten)]
    pub extensions: BTreeMap<String, serde_json::Value>,
}

impl PlatformSettings {
    pub(crate) fn validate(&self) -> Result<()> {
        validate_defaults(&self.defaults.values)?;
        validate_inactivity(&self.inactivity.values)?;
        if let Some(section) = &self.checkpoints {
            validate_checkpoints(&section.values)?;
        }
        Ok(())
    }
}

/// Persisted settings and exact replacement revision.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct StoredSettings {
    /// Current CAS revision; zero is valid for unpersisted defaults.
    pub revision: u64,
    /// Full settings document.
    pub settings: PlatformSettings,
}

/// Full settings replacement against a known revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplaceSettingsRequest {
    /// Exact revision being replaced.
    pub expected_revision: u64,
    /// Complete settings, including fields preserved from a previous read.
    pub settings: PlatformSettings,
}

/// Supplied sections replace whole sections; omitted sections are unchanged.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PatchSettingsRequest {
    /// Exact settings revision being patched.
    pub expected_revision: u64,
    /// Optional replacement default profiles.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub defaults: Option<Section<SettingsDefaults>>,
    /// Optional replacement inactivity section.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inactivity: Option<Section<Inactivity>>,
    /// Optional replacement snapshot policy.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checkpoints: Option<Section<Checkpoints>>,
    /// Optional replacement retention section.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retention: Option<Section<Retention>>,
    /// Optional replacement limits section.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limits: Option<Section<Limits>>,
    /// Optional replacement Trigger section.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub triggers: Option<Section<TriggerSettings>>,
    /// Optional replacement marketplace section.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub marketplace: Option<Section<MarketplaceSettings>>,
}

impl PatchSettingsRequest {
    pub(crate) fn validate(&self) -> Result<()> {
        if self.defaults.is_none()
            && self.inactivity.is_none()
            && self.checkpoints.is_none()
            && self.retention.is_none()
            && self.limits.is_none()
            && self.triggers.is_none()
            && self.marketplace.is_none()
        {
            return Err(Error::validation(
                "at least one settings section is required",
            ));
        }
        if let Some(section) = &self.defaults {
            validate_defaults(&section.values)?;
        }
        if let Some(section) = &self.inactivity {
            validate_inactivity(&section.values)?;
        }
        if let Some(section) = &self.checkpoints {
            validate_checkpoints(&section.values)?;
        }
        Ok(())
    }
}

fn validate_inactivity(value: &Inactivity) -> Result<()> {
    if !(60..=86_400).contains(&value.pause_after_seconds) {
        return Err(Error::validation(
            "pause_after_seconds must be between 60 and 86400",
        ));
    }
    Ok(())
}

fn validate_checkpoints(value: &Checkpoints) -> Result<()> {
    if !(60..=86_400).contains(&value.interval_seconds) {
        return Err(Error::validation(
            "interval_seconds must be between 60 and 86400",
        ));
    }
    Ok(())
}

fn validate_defaults(value: &SettingsDefaults) -> Result<()> {
    for reference in [
        &value.sandbox_profile,
        &value.workspace_profile,
        &value.agent_profile,
    ] {
        crate::resources::positive(u64::from(reference.values.revision))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checkpoint_patch_enforces_bounds_and_preserves_additive_fields() {
        for seconds in [60, 1800, 86400] {
            let patch: PatchSettingsRequest = serde_json::from_value(serde_json::json!({"expected_revision": 4, "checkpoints": {"interval_seconds": seconds, "future_policy": "preserved"}})).unwrap();
            patch.validate().unwrap();
            assert_eq!(
                serde_json::to_value(patch).unwrap()["checkpoints"]["future_policy"],
                "preserved"
            );
        }
        for seconds in [0, 59, 86401] {
            let patch = PatchSettingsRequest {
                checkpoints: Some(Section::new(Checkpoints {
                    interval_seconds: seconds,
                })),
                ..Default::default()
            };
            assert!(patch.validate().is_err());
        }
    }
}
