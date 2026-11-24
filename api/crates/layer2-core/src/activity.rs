//! Generic, secret-safe activity records.

use serde::{Deserialize, Serialize};

use crate::{now_rfc3339, ActivityDetail, ServiceId, SkillId, SubjectId};

pub const DEFAULT_ACTIVITY_LIMIT: u32 = 50;
pub const MAX_ACTIVITY_LIMIT: u32 = 200;

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
        Self {
            id: ActivityId::generate(),
            at: now_rfc3339(),
            actor: None,
            action,
            target: None,
            outcome,
            detail: ActivityDetail::new(),
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
        }
    }
}

/// Bounded activity-list filter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActivityQuery {
    pub actor: Option<SubjectId>,
    pub action: Option<ActivityAction>,
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
        Ok(())
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
