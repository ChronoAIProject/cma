//! Stable usage selections and explicit snapshot freshness. Authority is always live.
use super::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ViewWindow {
    #[default]
    #[serde(rename = "24h")]
    Day,
    #[serde(rename = "7d")]
    Week,
    #[serde(rename = "30d")]
    Month,
    #[serde(rename = "custom")]
    Custom,
}
impl ViewWindow {
    pub fn seconds(self) -> Option<u64> {
        match self {
            Self::Day => Some(86_400),
            Self::Week => Some(604_800),
            Self::Month => Some(30 * 86_400),
            Self::Custom => None,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ViewInput {
    pub window: ViewWindow,
    pub from: Option<u64>,
    pub to: Option<u64>,
    pub step_seconds: Option<u64>,
    pub filters: Filters,
    pub refresh: bool,
}
impl ViewInput {
    pub fn validate(self, now: u64) -> Result<ViewSelection> {
        let (from, to) = if let Some(seconds) = self.window.seconds() {
            if self.from.is_some() || self.to.is_some() {
                return Err(MetricsError::Invalid(
                    "Rolling windows cannot set from/to.".into(),
                ));
            }
            let to = now / 60 * 60;
            (to.saturating_sub(seconds), to)
        } else {
            (
                self.from
                    .ok_or_else(|| MetricsError::Invalid("Custom range needs from/to.".into()))?,
                self.to
                    .ok_or_else(|| MetricsError::Invalid("Custom range needs from/to.".into()))?,
            )
        };
        // Rolling keys must not change resolution as their boundary crosses a
        // bucket. A week at 600 seconds fits every alignment within 2016 points.
        let step_seconds = self.step_seconds.or(match self.window {
            ViewWindow::Day => Some(60),
            ViewWindow::Week => Some(600),
            ViewWindow::Month => Some(3600),
            ViewWindow::Custom => None,
        });
        let query = ReportInput {
            from: Some(from),
            to: Some(to),
            step_seconds,
            filters: self.filters,
        }
        .validate(now)?;
        Ok(ViewSelection {
            window: self.window,
            query,
            refresh: self.refresh,
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ViewSelection {
    pub window: ViewWindow,
    pub query: ReportQuery,
    pub refresh: bool,
}
impl ViewSelection {
    pub fn key(&self, scope: &Scope) -> Result<String> {
        let fixed_range = (self.window == ViewWindow::Custom).then_some(self.query.range());
        let bytes = serde_json::to_vec(&(
            1_u32,
            ALGORITHM_VERSION,
            scope,
            self.window,
            fixed_range,
            self.query.step_seconds,
            &self.query.filters,
        ))
        .map_err(|_| MetricsError::Unavailable)?;
        Ok(format!("usage-view:{:x}", Sha256::digest(bytes)))
    }
    pub fn matches(&self, scope: &Scope, report: &Report) -> Result<bool> {
        let selected = Self {
            window: self.window,
            query: report.query.clone(),
            refresh: false,
        };
        Ok(report.scope == *scope
            && self.key(scope)? == selected.key(&report.scope)?
            && self.window.seconds().is_none_or(|seconds| {
                report.query.to.checked_sub(report.query.from) == Some(seconds)
            }))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefreshState {
    Idle,
    Pending,
    Deferred,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ViewRefresh {
    pub state: RefreshState,
    pub report_id: Option<String>,
    pub retry_after_seconds: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportView {
    pub schema_version: u32,
    pub window: ViewWindow,
    pub requested_range: Range,
    pub report: Report,
    pub generated_at: Option<u64>,
    pub stale: bool,
    pub refresh: ViewRefresh,
}
