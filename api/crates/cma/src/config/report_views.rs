//! Retained snapshots and operator-scoped warming require separate activation.
use layer2_core::sandbox_metrics::{query::opaque_id, Scope, ViewWindow};
use serde::Deserialize;

#[derive(Clone, Debug)]
pub(crate) struct ViewPolicy {
    pub enabled: bool,
    pub aggregate_projections_enabled: bool,
    pub allocation_rollups_enabled: bool,
    pub freshness_seconds: u64,
    pub retain_seconds: u64,
    pub warm_enabled: bool,
    pub warm_seconds: u64,
    pub warm_scopes: Vec<WarmSelection>,
    pub view_scopes: Vec<WarmScope>,
}
impl Default for ViewPolicy {
    fn default() -> Self {
        Self {
            enabled: false,
            aggregate_projections_enabled: false,
            allocation_rollups_enabled: false,
            freshness_seconds: 300,
            retain_seconds: 86_400,
            warm_enabled: false,
            warm_seconds: 300,
            warm_scopes: Vec::new(),
            view_scopes: Vec::new(),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WarmScope {
    pub principal: String,
    pub admin: bool,
    /// Canary can name one of the owner's test Sandboxes. Empty means the
    /// explicitly approved default Everyone/own-resources scope.
    pub sandbox_id: Option<String>,
}
/// Existing configurations keep warming 24 hours; other windows are explicit.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WarmSelection {
    pub principal: String,
    pub admin: bool,
    pub sandbox_id: Option<String>,
    #[serde(default)]
    pub window: ViewWindow,
}
impl WarmSelection {
    pub fn scope(&self) -> Scope {
        Scope {
            principal: self.principal.clone(),
            admin: self.admin,
        }
    }
}
#[cfg(test)]
impl From<WarmScope> for WarmSelection {
    fn from(scope: WarmScope) -> Self {
        Self {
            principal: scope.principal,
            admin: scope.admin,
            sandbox_id: scope.sandbox_id,
            window: ViewWindow::Day,
        }
    }
}
impl WarmScope {
    pub fn allows(&self, scope: &Scope, sandbox: Option<&str>) -> bool {
        self.principal == scope.principal
            && self.admin == scope.admin
            && self
                .sandbox_id
                .as_deref()
                .is_none_or(|id| Some(id) == sandbox)
    }
}
impl ViewPolicy {
    pub(crate) fn from_lookup(
        lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
    ) -> Result<Self, String> {
        fn boolean(
            lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
            key: &str,
        ) -> Result<bool, String> {
            match lookup(key)?.as_deref() {
                None | Some("false") => Ok(false),
                Some("true") => Ok(true),
                _ => Err(format!("{key} must be true or false")),
            }
        }
        fn number(
            lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
            key: &str,
            default: u64,
            min: u64,
            max: u64,
        ) -> Result<u64, String> {
            let value = lookup(key)?
                .map(|v| v.parse::<u64>().map_err(|_| format!("invalid {key}")))
                .transpose()?
                .unwrap_or(default);
            if !(min..=max).contains(&value) {
                return Err(format!("{key} must be in {min}..={max}"));
            }
            Ok(value)
        }
        let enabled = boolean(lookup, "CHRONO_USAGE_REPORT_VIEWS_ENABLED")?;
        let aggregate_projections_enabled =
            boolean(lookup, "CHRONO_USAGE_REPORT_AGGREGATE_PROJECTIONS_ENABLED")?;
        let allocation_rollups_enabled =
            boolean(lookup, "CHRONO_USAGE_ALLOCATION_ROLLUPS_ENABLED")?;
        let warm_enabled = boolean(lookup, "CHRONO_USAGE_REPORT_WARMER_ENABLED")?;
        let freshness_seconds = number(lookup, "CHRONO_USAGE_REPORT_FRESH_SECONDS", 300, 60, 3600)?;
        let retain_seconds = number(
            lookup,
            "CHRONO_USAGE_REPORT_RETAIN_SECONDS",
            86_400,
            3600,
            86_400,
        )?;
        let warm_seconds = number(lookup, "CHRONO_USAGE_REPORT_WARM_SECONDS", 300, 60, 3600)?;
        let scopes = lookup("CHRONO_USAGE_REPORT_WARM_SCOPES")?.unwrap_or_else(|| "[]".into());
        if scopes.len() > 8192 {
            return Err("usage warm scope configuration exceeds 8192 bytes".into());
        }
        let warm_scopes: Vec<WarmSelection> =
            serde_json::from_str(&scopes).map_err(|_| "invalid usage warm scopes".to_string())?;
        if warm_scopes.len() > 8
            || warm_scopes.iter().enumerate().any(|(index, scope)| {
                scope.window == ViewWindow::Custom
                    || warm_scopes[..index].contains(scope)
                    || scope.principal.is_empty()
                    || scope.principal.len() > 256
                    || scope.principal.chars().any(char::is_control)
                    || scope
                        .sandbox_id
                        .as_ref()
                        .is_some_and(|id| !opaque_id(id, "sbx_"))
            })
        {
            return Err(
                "usage warm scopes require at most 8 valid principals and optional Sandbox IDs"
                    .into(),
            );
        }
        let scopes = lookup("CHRONO_USAGE_REPORT_VIEW_SCOPES")?.unwrap_or_else(|| "[]".into());
        if scopes.len() > 8192 {
            return Err("usage view scope configuration exceeds 8192 bytes".into());
        }
        let view_scopes: Vec<WarmScope> =
            serde_json::from_str(&scopes).map_err(|_| "invalid usage view scopes".to_string())?;
        if view_scopes.len() > 8
            || view_scopes.iter().any(|scope| {
                scope.principal.is_empty()
                    || scope.principal.len() > 256
                    || scope.principal.chars().any(char::is_control)
                    || scope
                        .sandbox_id
                        .as_ref()
                        .is_some_and(|id| !opaque_id(id, "sbx_"))
            })
        {
            return Err(
                "usage view scopes require at most 8 valid principals and optional Sandbox IDs"
                    .into(),
            );
        }
        if (enabled || aggregate_projections_enabled) && view_scopes.is_empty() {
            return Err("retained views require an explicit approved scope".into());
        }
        if warm_scopes.iter().any(|warm| {
            !view_scopes
                .iter()
                .any(|allowed| allowed.allows(&warm.scope(), warm.sandbox_id.as_deref()))
        }) {
            return Err("usage warm scope is outside the approved retained-view cohort".into());
        }
        if warm_enabled && (!enabled || warm_scopes.is_empty()) {
            return Err("usage warmer requires enabled views and explicit approved scopes".into());
        }
        if freshness_seconds >= retain_seconds {
            return Err("usage freshness must be shorter than retention".into());
        }
        Ok(Self {
            enabled,
            aggregate_projections_enabled,
            allocation_rollups_enabled,
            freshness_seconds,
            retain_seconds,
            warm_enabled,
            warm_seconds,
            warm_scopes,
            view_scopes,
        })
    }
}

#[cfg(test)]
#[path = "report_views_tests.rs"]
mod tests;
