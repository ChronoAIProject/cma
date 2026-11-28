//! Generic, secret-safe activity records.

use serde::{Deserialize, Serialize};

use crate::{now_rfc3339, ActivityDetail, DetailKey, ServiceId, SkillId, SubjectId};

pub const DEFAULT_ACTIVITY_LIMIT: u32 = 50;
pub const MAX_ACTIVITY_LIMIT: u32 = 200;
pub const ACTIVITY_OUTCOME_UNCONFIRMED: &str = "outcome_unconfirmed";

tokio::task_local! {
    static REQUEST_ID: String;
}

pub async fn with_request_id<F: std::future::Future>(id: String, future: F) -> F::Output {
    REQUEST_ID.scope(id, future).await
}

/// Opaque activity-record identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ActivityId(pub String);

impl ActivityId {
    pub fn generate() -> Self {
        Self(format!("act_{}", uuid::Uuid::new_v4().simple()))
    }
}

/// High-level action recorded without carrying a resource-specific command model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityAction {
    Authenticate,
    Create,
    Read,
    List,
    Update,
    Delete,
    Configure,
    GatewayRecordWrite,
    GatewayRecordRead,
    GatewayRecordDelete,
}

/// Safe identity of the thing an action reached.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum ActivityTarget {
    Subject(SubjectId),
    Service(ServiceId),
    Skill(SkillId),
    Resource(String),
}

/// Whether the request completed and the stable refusal code when it did not.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", content = "code", rename_all = "snake_case")]
pub enum ActivityOutcome {
    Succeeded,
    Refused(String),
    Failed(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityResult {
    Succeeded,
    Refused,
    Failed,
}

impl ActivityResult {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Refused => "refused",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityChannel {
    Web,
    Api,
    Cli,
    Sdk,
    Service,
    Unknown,
}

impl ActivityChannel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Web => "web",
            Self::Api => "api",
            Self::Cli => "cli",
            Self::Sdk => "sdk",
            Self::Service => "service",
            Self::Unknown => "unknown",
        }
    }
}

/// One bounded activity record. Request bodies and credentials never appear here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActivityRecord {
    pub id: ActivityId,
    pub at: String,
    pub actor: Option<SubjectId>,
    pub action: ActivityAction,
    pub target: Option<ActivityTarget>,
    pub outcome: ActivityOutcome,
    pub detail: ActivityDetail,
}

impl ActivityRecord {
    pub fn new(action: ActivityAction, outcome: ActivityOutcome) -> Self {
        let mut detail = ActivityDetail::new();
        if let Ok(id) = REQUEST_ID.try_with(Clone::clone) {
            detail.record(DetailKey::RequestId, id);
        }
        Self {
            id: ActivityId::generate(),
            at: now_rfc3339(),
            actor: None,
            action,
            target: None,
            outcome,
            detail,
        }
    }

    pub fn by(mut self, actor: SubjectId) -> Self {
        self.actor = Some(actor);
        self
    }

    pub fn on(mut self, target: ActivityTarget) -> Self {
        self.target = Some(target);
        self
    }
}

/// Displayed Activity fields; omission of detail is explicit in the list contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActivitySummary {
    pub id: ActivityId,
    pub at: String,
    pub actor: Option<SubjectId>,
    pub action: ActivityAction,
    pub target: Option<ActivityTarget>,
    pub outcome: ActivityOutcome,
    #[serde(default)]
    pub detail: ActivityDetail,
}

impl From<ActivityRecord> for ActivitySummary {
    fn from(record: ActivityRecord) -> Self {
        Self {
            id: record.id,
            at: record.at,
            actor: record.actor,
            action: record.action,
            target: record.target,
            outcome: record.outcome,
            detail: record.detail.request_summary(),
        }
    }
}

/// Bounded activity-list filter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActivityQuery {
    pub actor: Option<SubjectId>,
    pub action: Option<ActivityAction>,
    pub result: Option<ActivityResult>,
    pub channel: Option<ActivityChannel>,
    pub recorded_from: Option<String>,
    pub recorded_before: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub exclude_successful_reads: bool,
    pub limit: u32,
    pub cursor: Option<String>,
}

impl Default for ActivityQuery {
    fn default() -> Self {
        Self {
            actor: None,
            action: None,
            result: None,
            channel: None,
            recorded_from: None,
            recorded_before: None,
            exclude_successful_reads: false,
            limit: DEFAULT_ACTIVITY_LIMIT,
            cursor: None,
        }
    }
}

impl ActivityQuery {
    pub fn validate(&self) -> Result<(), crate::CoreError> {
        if !(1..=MAX_ACTIVITY_LIMIT).contains(&self.limit) {
            return Err(crate::CoreError::InvalidRequest(format!(
                "activity limit must be between 1 and {MAX_ACTIVITY_LIMIT}"
            )));
        }
        if let Some(cursor) = &self.cursor {
            let valid = cursor.len() <= 256
                && cursor
                    .split_once(':')
                    .is_some_and(|(at, id)| at.parse::<i64>().is_ok() && !id.is_empty());
            if !valid {
                return Err(crate::CoreError::InvalidRequest(
                    "activity cursor is invalid".into(),
                ));
            }
        }
        self.recorded_bounds_ms()?;
        Ok(())
    }

    /// The inclusive lower and exclusive upper bounds on persistence time.
    pub fn recorded_bounds_ms(&self) -> Result<(Option<i64>, Option<i64>), crate::CoreError> {
        use time::{format_description::well_known::Rfc3339, OffsetDateTime};

        let parse = |value: &str| {
            OffsetDateTime::parse(value, &Rfc3339)
                .map(|at| at.unix_timestamp_nanos().div_euclid(1_000_000))
                .ok()
                .and_then(|milliseconds| i64::try_from(milliseconds).ok())
                .ok_or_else(|| {
                    crate::CoreError::InvalidRequest("activity time must be RFC 3339".into())
                })
        };
        let from = self.recorded_from.as_deref().map(parse).transpose()?;
        let before = self.recorded_before.as_deref().map(parse).transpose()?;
        if from
            .zip(before)
            .is_some_and(|(from, before)| from >= before)
        {
            return Err(crate::CoreError::InvalidRequest(
                "activity start must precede its end".into(),
            ));
        }
        Ok((from, before))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_record_carries_only_explicit_safe_detail() {
        let record = ActivityRecord::new(ActivityAction::Read, ActivityOutcome::Succeeded)
            .by(SubjectId::new("subject-a"))
            .on(ActivityTarget::Resource("resource-a".into()));
        let encoded = serde_json::to_string(&record).unwrap();
        assert!(encoded.contains("resource-a"));
        assert!(!encoded.contains("credential"));
    }
}
