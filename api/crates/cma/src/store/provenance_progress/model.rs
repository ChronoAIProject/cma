use mongodb::bson::{doc, DateTime, Document};
use serde::{Deserialize, Serialize};

use super::{corrupt, Result};

pub(super) const CONTROL: &str = "agent-provenance-v1";
pub(crate) const COLLECTION: &str = "agent_provenance_progress";
pub(crate) const ACTIONS: &[&str] = &[
    "find",
    "insert",
    "update",
    "createCollection",
    "listIndexes",
];

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum Phase {
    Ready,
    Running,
    SweepFinished,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct Counts {
    pub scanned: i64,
    pub recorded: i64,
    pub retained: i64,
    pub skipped: i64,
    pub errors: i64,
}
impl Counts {
    pub fn valid(&self) -> bool {
        let values = [self.recorded, self.retained, self.skipped, self.errors];
        values.iter().all(|n| *n >= 0)
            && values.iter().try_fold(0_i64, |sum, n| sum.checked_add(*n)) == Some(self.scanned)
    }
    pub fn observe(&mut self, outcome: &str) -> Result<()> {
        self.scanned = self.scanned.checked_add(1).ok_or_else(corrupt)?;
        let value = match outcome {
            "recorded" => &mut self.recorded,
            "retained" => &mut self.retained,
            "missing" => &mut self.skipped,
            "corrupt" => &mut self.errors,
            _ => return Err(corrupt()),
        };
        *value = value.checked_add(1).ok_or_else(corrupt)?;
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Control {
    #[serde(rename = "_id")]
    pub id: String,
    pub document_schema_version: i64,
    pub protocol: i64,
    pub phase: Phase,
    pub run: String,
    pub owner: Option<String>,
    pub fence: i64,
    pub revision: i64,
    pub cursor: Option<String>,
    pub counts: Counts,
    pub started_at: Option<DateTime>,
    pub updated_at: DateTime,
    pub lease_until: DateTime,
    pub next_sweep_at: DateTime,
    pub last_receipt: Option<String>,
}
impl Control {
    pub fn initial() -> Self {
        Self {
            id: CONTROL.into(),
            document_schema_version: 1,
            protocol: 1,
            phase: Phase::Ready,
            run: uuid::Uuid::new_v4().to_string(),
            owner: None,
            fence: 0,
            revision: 0,
            cursor: None,
            counts: Counts::default(),
            started_at: None,
            updated_at: DateTime::from_millis(0),
            lease_until: DateTime::from_millis(0),
            next_sweep_at: DateTime::from_millis(0),
            last_receipt: None,
        }
    }
    pub fn decode(row: Document) -> Result<Self> {
        let value: Self = mongodb::bson::deserialize_from_document(row).map_err(|_| corrupt())?;
        if value.id != CONTROL
            || value.document_schema_version != 1
            || value.protocol != 1
            || value.fence < 0
            || value.revision < 0
            || !value.counts.valid()
            || uuid::Uuid::parse_str(&value.run).is_err()
            || value.cursor.as_ref().is_some_and(String::is_empty)
            || (value.phase != Phase::Ready && value.started_at.is_none())
            || (value.phase == Phase::SweepFinished
                && value.last_receipt.as_deref() != Some(&format!("sweep:{}", value.run)))
        {
            return Err(corrupt());
        }
        Ok(value)
    }
    pub fn document(&self) -> Result<Document> {
        mongodb::bson::serialize_to_document(self).map_err(|_| corrupt())
    }
    pub fn expected(&self) -> Document {
        doc! {"_id": CONTROL, "protocol": 1_i64, "revision": self.revision,
        "run": &self.run, "fence": self.fence}
    }
}

pub(super) fn public_control(value: &Control) -> serde_json::Value {
    serde_json::json!({"cohort":"agent_provenance", "phase":value.phase,
        "run":value.run,"revision":value.revision,"fence":value.fence,
        "counts":value.counts,"started_at":value.started_at,"updated_at":value.updated_at,
        "last_receipt":value.last_receipt,"global_completion":false})
}

#[cfg(test)]
#[path = "model_tests.rs"]
mod tests;
