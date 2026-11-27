//! A separate durable marker coordinates one staging suggestion claim across replicas.
use crate::config::suggestion_control::{Mode, Target};
use layer2_core::sandbox::{suggestions::*, AgentReadError, Sandbox};
use mongodb::bson::{doc, serialize_to_bson, Document};
use mongodb::Collection;
use serde::{Deserialize, Serialize};
use std::future::IntoFuture;
use std::time::Duration;

use super::sandbox_mongo::MongoSandboxStore;

const VERSION: &str = "staging-suggestion-control-v1";
const ARM_MS: i64 = 120_000;
const PAUSE_MS: i64 = 18_000;
const GRACE_MS: i64 = 7_000;
const IO_LIMIT: Duration = Duration::from_secs(1);

#[derive(Clone)]
pub(crate) struct SuggestionControl {
    rows: Collection<Document>,
    agents: Collection<Document>,
    sandboxes: Collection<Document>,
    target: Target,
    marker_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct ControlReport {
    pub mode: Mode,
    pub armed_at_ms: i64,
    pub expires_at_ms: i64,
    pub turn_sequence: Option<i64>,
    pub answer_sequence: Option<i64>,
    pub claim_started_at_ms: Option<i64>,
    pub claim_deadline_ms: Option<i64>,
    pub held_at_ms: Option<i64>,
    pub released_at_ms: Option<i64>,
    pub settled_at_ms: Option<i64>,
    pub parent_snapshot_revision: Option<i64>,
    pub pause_revision: Option<i64>,
    #[serde(default)]
    pub durable_status: Option<String>,
    #[serde(default)]
    pub durable_has_set: Option<bool>,
}

fn unavailable(_: impl std::fmt::Debug) -> AgentReadError {
    AgentReadError::Unavailable("staging suggestion control unavailable".into())
}

impl SuggestionControl {
    pub(crate) fn new(store: &MongoSandboxStore, target: Target) -> Self {
        let marker_id = format!("{VERSION}:{}:{}", target.sandbox_id, target.agent_id);
        Self {
            rows: store.suggestions.clone(),
            agents: store.agents.clone(),
            sandboxes: store.sandboxes.clone(),
            target,
            marker_id,
        }
    }

    fn marker(&self) -> Document {
        doc! {"_id": &self.marker_id, "version": VERSION}
    }

    async fn linked_owner(&self, owner: Option<&str>) -> Result<Option<String>, AgentReadError> {
        let mut filter = doc! {"_id": &self.target.agent_id, "sandbox_id": &self.target.sandbox_id};
        if let Some(owner) = owner {
            filter.insert("owner", owner);
        }
        let row = self
            .agents
            .find_one(filter)
            .projection(doc! {"owner": 1})
            .await
            .map_err(unavailable)?;
        row.map(|row| row.get_str("owner").map(str::to_owned).map_err(unavailable))
            .transpose()
    }

    pub(crate) async fn arm(&self, mode: Mode) -> Result<bool, AgentReadError> {
        let Some(owner) = self.linked_owner(None).await? else {
            return Ok(false);
        };
        let now = unix_millis() as i64;
        let mut filter = self.marker();
        filter.insert(
            "$or",
            [
                doc! {"expires_at_ms": {"$lte": now}},
                doc! {"settled_at_ms": {"$exists": true}},
            ],
        );
        let result = self
            .rows
            .replace_one(
                filter,
                doc! {
                    "_id": &self.marker_id, "version": VERSION, "owner": owner,
                    "agent_id": &self.target.agent_id, "answer_sequence": 0_i64,
                    "mode": serialize_to_bson(&mode).map_err(unavailable)?,
                    "armed_at_ms": now, "expires_at_ms": now + ARM_MS,
                },
            )
            .upsert(true)
            .await;
        match result {
            Ok(_) => Ok(true),
            Err(error) if crate::store::sandbox_mongo::is_duplicate_key(&error) => Ok(false),
            Err(error) => Err(unavailable(error)),
        }
    }

    pub(crate) async fn read(&self) -> Result<Option<ControlReport>, AgentReadError> {
        let Some(row) = self
            .rows
            .find_one(self.marker())
            .await
            .map_err(unavailable)?
        else {
            return Ok(None);
        };
        let mut report: ControlReport =
            mongodb::bson::deserialize_from_document(row.clone()).map_err(unavailable)?;
        if let Ok(claim_id) = row.get_str("claim_id") {
            let durable = self.rows.find_one(doc! {
                "_id": claim_id, "version": STRATEGY_VERSION, "agent_id": &self.target.agent_id,
                "owner": row.get_str("owner").map_err(unavailable)?,
                "answer_sequence": report.answer_sequence,
                "record.started_at_ms": report.claim_started_at_ms, "deadline_ms": report.claim_deadline_ms,
            }).projection(doc! {"status": 1, "record.set": 1}).await.map_err(unavailable)?;
            if let Some(durable) = durable {
                report.durable_status = durable.get_str("status").ok().map(str::to_owned);
                report.durable_has_set = Some(
                    durable
                        .get_document("record")
                        .ok()
                        .is_some_and(|record| record.get_document("set").is_ok()),
                );
            }
        }
        Ok(Some(report))
    }

    pub(crate) async fn release(&self, claim_started_at_ms: i64) -> Result<bool, AgentReadError> {
        let mut claim = self.marker();
        claim.insert("claim_started_at_ms", claim_started_at_ms);
        let Some(row) = self
            .rows
            .find_one(claim.clone())
            .await
            .map_err(unavailable)?
        else {
            return Ok(false);
        };
        if row.contains_key("released_at_ms") {
            return Ok(true);
        }
        let now = unix_millis() as i64;
        if row.get_i64("expires_at_ms").map_err(unavailable)? <= now {
            return Ok(false);
        }
        let mut update = doc! {};
        match row.get_str("mode").map_err(unavailable)? {
            "pause_first" => {
                let snapshot_revision = row
                    .get_i64("parent_snapshot_revision")
                    .map_err(unavailable)?;
                let Some(parent) = self.sandboxes.find_one(doc! {
                "_id": &self.target.sandbox_id, "owner":row.get_str("owner").map_err(unavailable)?,
                "status": {"$in": ["pausing", "paused"]}, "revision":{"$gt":snapshot_revision},
                }).projection(doc! {"revision":1}).await.map_err(unavailable)? else {
                    return Ok(false);
                };
                update.insert(
                    "pause_revision",
                    parent.get_i64("revision").map_err(unavailable)?,
                );
            }
            "late_ready" if now >= row.get_i64("claim_deadline_ms").map_err(unavailable)? => {}
            _ => return Ok(false),
        }
        claim.insert("claim_id", row.get_str("claim_id").map_err(unavailable)?);
        claim.insert(
            "armed_at_ms",
            row.get_i64("armed_at_ms").map_err(unavailable)?,
        );
        let now = unix_millis() as i64;
        update.insert("released_at_ms", now);
        claim.insert("released_at_ms", doc! {"$exists":false});
        claim.insert("settled_at_ms", doc! {"$exists":false});
        claim.insert("expires_at_ms", doc! {"$gt": now});
        let result = self
            .rows
            .update_one(claim.clone(), doc! {"$set":update})
            .await
            .map_err(unavailable)?;
        if result.matched_count == 1 {
            return Ok(true);
        }
        claim.remove("expires_at_ms");
        claim.remove("settled_at_ms");
        claim.insert("released_at_ms", doc! {"$exists":true});
        Ok(self
            .rows
            .find_one(claim)
            .projection(doc! {"_id":1})
            .await
            .map_err(unavailable)?
            .is_some())
    }

    pub(crate) async fn hold_before_publish(
        &self,
        claim: &SuggestionWork,
        completion: &SuggestionCompletion,
    ) {
        self.hold(claim, completion, Mode::LateReady, None).await;
    }

    pub(crate) async fn hold_before_parent_fence(
        &self,
        claim: &SuggestionWork,
        completion: &SuggestionCompletion,
        parent: &Sandbox,
    ) {
        if parent.id.as_str() == self.target.sandbox_id && parent.owner == claim.owner {
            self.hold(claim, completion, Mode::PauseFirst, Some(parent.revision))
                .await;
        }
    }

    async fn hold(
        &self,
        claim: &SuggestionWork,
        completion: &SuggestionCompletion,
        mode: Mode,
        parent_revision: Option<u64>,
    ) {
        if completion.status != SuggestionStatus::Ready
            || completion.set.is_none()
            || !self.matches(claim)
        {
            return;
        }
        let (Some(started), Some(deadline)) = (claim.started_at_ms, claim.deadline_ms) else {
            return;
        };
        if !matches!(
            tokio::time::timeout(IO_LIMIT, self.linked_owner(Some(claim.owner.as_str()))).await,
            Ok(Ok(Some(_)))
        ) {
            return;
        }
        let now = unix_millis() as i64;
        // The expiry scanner can first see this row at the original deadline.
        // Bind before then, allowing the full I/O budget for the marker write.
        if mode == Mode::LateReady && now + IO_LIMIT.as_millis() as i64 >= deadline as i64 {
            return;
        }
        let limit = match mode {
            Mode::PauseFirst => now + PAUSE_MS,
            Mode::LateReady => (deadline as i64).saturating_add(GRACE_MS),
        };
        if limit <= now {
            return;
        }
        let acquire = async {
            let mut filter = self.marker();
            filter.extend(doc! {
                "owner": claim.owner.as_str(), "mode": serialize_to_bson(&mode).map_err(unavailable)?,
                "armed_at_ms": {"$lt": claim.created_at_ms as i64},
                "expires_at_ms": {"$gt":now}, "claim_started_at_ms": {"$exists":false},
            });
            let mut held = doc! {
                "claim_id": &claim.id, "claim_started_at_ms": started as i64, "claim_deadline_ms": deadline as i64,
                "turn_sequence": claim.scope.turn_sequence as i64, "answer_sequence": claim.scope.answer_sequence as i64,
                "held_at_ms": now, "expires_at_ms": limit,
            };
            if let Some(revision) = parent_revision {
                held.insert(
                    "parent_snapshot_revision",
                    i64::try_from(revision).map_err(unavailable)?,
                );
            }
            let result = self
                .rows
                .update_one(filter, doc! {"$set":held})
                .await
                .map_err(unavailable)?;
            Ok::<_, AgentReadError>(result.matched_count == 1)
        };
        let acquisition_confirmed = match tokio::time::timeout(IO_LIMIT, acquire).await {
            Ok(Ok(false)) => return,
            Ok(Ok(true)) => true,
            // The write can still become visible after a lost acknowledgement.
            Ok(Err(_)) | Err(_) => false,
        };
        let remaining =
            Duration::from_millis(limit.saturating_sub(unix_millis() as i64).max(0) as u64);
        let mut filter = self.marker();
        filter.extend(doc! {"claim_id": &claim.id, "claim_started_at_ms": started as i64});
        let _ = tokio::time::timeout(remaining, async {
            if mode == Mode::LateReady {
                tokio::time::sleep(Duration::from_millis(
                    deadline.saturating_sub(unix_millis()),
                ))
                .await;
            }
            loop {
                let read = self
                    .rows
                    .find_one(filter.clone())
                    .projection(doc! {"released_at_ms": 1});
                match tokio::time::timeout(IO_LIMIT, read.into_future()).await {
                    Ok(Ok(Some(row))) if row.get_i64("released_at_ms").is_ok() => return,
                    Ok(Ok(None)) if acquisition_confirmed => return,
                    Ok(Ok(_)) => {}
                    Ok(Err(_)) | Err(_) => {
                        tracing::debug!("Retrying uncertain staging suggestion release read");
                    }
                }
                tokio::time::sleep(Duration::from_millis(75)).await;
            }
        })
        .await;
    }

    fn matches(&self, claim: &SuggestionWork) -> bool {
        claim.agent_id.as_str() == self.target.agent_id
            && claim.status == SuggestionStatus::Running
            && claim.strategy_version == STRATEGY_VERSION
    }

    pub(crate) async fn exclude_held_expiry(&self, filter: &mut Document, now_ms: u64) {
        let mut marker = self.marker();
        marker.extend(
            doc! {"mode":"late_ready", "expires_at_ms":{"$gt":now_ms as i64},
            "claim_deadline_ms":{"$lte":now_ms as i64}, "settled_at_ms":{"$exists":false}},
        );
        let read = self.rows.find_one(marker);
        match tokio::time::timeout(IO_LIMIT, read.into_future()).await {
            Ok(Ok(Some(row))) => exclude_claim(filter, &row),
            Ok(Ok(None)) => {}
            Ok(Err(_)) | Err(_) => {
                // Protect only claims that could still be inside the configured hold cap.
                filter.insert(
                    "$nor",
                    [doc! {
                        "agent_id": &self.target.agent_id, "status": "running",
                        "deadline_ms": {"$gt": (now_ms as i64).saturating_sub(GRACE_MS)},
                    }],
                );
                tracing::debug!(
                    "Deferring configured staging Agent suggestion expiry for one scan"
                );
            }
        }
    }

    pub(crate) async fn record_commit(&self, claim: &SuggestionWork) {
        if !self.matches(claim) {
            return;
        }
        let (Some(started), Some(deadline)) = (claim.started_at_ms, claim.deadline_ms) else {
            return;
        };
        let mut marker = self.marker();
        marker.extend(doc! {
            "claim_id": &claim.id, "claim_started_at_ms": started as i64, "claim_deadline_ms": deadline as i64,
            "settled_at_ms": {"$exists":false},
        });
        let _ = tokio::time::timeout(
            IO_LIMIT,
            self.rows
                .update_one(
                    marker,
                    doc! {
                        "$set": {"settled_at_ms": unix_millis() as i64},
                    },
                )
                .into_future(),
        )
        .await;
    }
}

fn exclude_claim(filter: &mut Document, row: &Document) {
    if let (Ok(id), Ok(started), Ok(deadline)) = (
        row.get_str("claim_id"),
        row.get_i64("claim_started_at_ms"),
        row.get_i64("claim_deadline_ms"),
    ) {
        filter.insert(
            "$nor",
            [
                doc! {"_id":id, "version":STRATEGY_VERSION, "status":"running",
                "record.started_at_ms":started, "deadline_ms":deadline},
            ],
        );
    }
}

#[cfg(test)]
mod tests;
