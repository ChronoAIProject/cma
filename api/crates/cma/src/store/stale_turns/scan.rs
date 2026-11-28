//! The scan lease fences only scheduling state. It grants no Agent authority.

use std::time::{Duration, Instant};

use futures_util::TryStreamExt;
use layer2_core::AgentId;
use mongodb::{
    bson::{self, doc, DateTime, Document},
    options::{Hint, ReturnDocument},
    Collection,
};
use serde::Deserialize;

use super::{ScanError, StaleTurnStore};
use crate::store::sandbox_mongo::{is_duplicate_key, DOCUMENT_SCHEMA_VERSION};

const PAGE_SIZE: u32 = 128;
// This outlasts the ten-second worker deadline and allows a one-second
// scheduling margin around the next 30-second tick.
const LEASE_MILLISECONDS: i64 = 29_000;
pub(super) const QUERY_DEADLINE: Duration = Duration::from_secs(1);
const SCAN_ID: &str = "stale-admitted-turns-v1";

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Lane {
    #[default]
    InProgress,
    WaitingAction,
}

impl Lane {
    fn status(self) -> &'static str {
        match self {
            Self::InProgress => "in_progress",
            Self::WaitingAction => "waiting_action",
        }
    }

    fn alternate(self) -> Self {
        match self {
            Self::InProgress => Self::WaitingAction,
            Self::WaitingAction => Self::InProgress,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Position {
    pub lane: Lane,
    pub after: Option<String>,
    pub wrapped_at: Option<DateTime>,
}

pub(crate) struct Lease {
    token: String,
    pub position: Position,
    pub now: DateTime,
}

pub(crate) struct Page {
    pub rows: Vec<Document>,
    pub eof: bool,
}

#[derive(Default)]
pub(crate) struct OwnedPosition {
    next: Lane,
    in_progress: OwnedLanePosition,
    waiting_action: OwnedLanePosition,
    traversal_started: Option<Instant>,
}

#[derive(Default)]
struct OwnedLanePosition {
    after: Option<(String, i64)>,
    wrapped: bool,
}

impl OwnedPosition {
    pub(crate) fn next_lane(&mut self) -> Lane {
        let lane = self.next;
        self.next = lane.alternate();
        self.traversal_started.get_or_insert_with(Instant::now);
        lane
    }

    pub(crate) fn advance(
        &mut self,
        lane: Lane,
        page: &Page,
        examined: usize,
    ) -> Result<(), ScanError> {
        let position = match lane {
            Lane::InProgress => &mut self.in_progress,
            Lane::WaitingAction => &mut self.waiting_action,
        };
        if page.eof && examined == page.rows.len() {
            position.after = None;
            position.wrapped = true;
        } else if let Some(row) = examined
            .checked_sub(1)
            .and_then(|index| page.rows.get(index))
        {
            let agent = row.get_str("agent_id").map_err(|_| ScanError::Invalid)?;
            let order = row
                .get_i64("agent_order")
                .or_else(|_| row.get_i32("agent_order").map(i64::from))
                .map_err(|_| ScanError::Invalid)?;
            position.after = Some((agent.to_owned(), order));
        }
        if self.in_progress.wrapped && self.waiting_action.wrapped {
            if let Some(started) = self.traversal_started.replace(Instant::now()) {
                metrics::histogram!("cma_stale_turn_full_traversal_seconds", "scope" => "owner")
                    .record(started.elapsed().as_secs_f64());
            }
            self.in_progress.wrapped = false;
            self.waiting_action.wrapped = false;
        }
        Ok(())
    }
}

impl StaleTurnStore {
    pub(crate) async fn claim(&self) -> Result<Option<Lease>, ScanError> {
        let token = uuid::Uuid::new_v4().to_string();
        let expired = doc! {"$lte": [
            {"$ifNull": ["$scan_expires_at", DateTime::from_millis(0)]}, "$$NOW",
        ]};
        let row = self
            .state
            .find_one_and_update(
                doc! {"_id": {"scan": SCAN_ID}},
                vec![doc! {"$replaceWith": {"$cond": [
                    expired,
                    {"$mergeObjects": ["$$ROOT", {
                    // Replica startup checks every row in the owner collection.
                    "document_schema_version": {"$ifNull": [
                        "$document_schema_version", DOCUMENT_SCHEMA_VERSION,
                    ]},
                    "scan_token": {"$literal": &token},
                    "scan_started_at": "$$NOW",
                    "scan_expires_at": {"$add": ["$$NOW", LEASE_MILLISECONDS]},
                    }]},
                    "$$ROOT",
                ]}}],
            )
            .upsert(true)
            .return_document(ReturnDocument::After)
            .max_time(QUERY_DEADLINE)
            .await;
        match row {
            Ok(Some(row)) => {
                if row.get_i64("document_schema_version").ok() != Some(DOCUMENT_SCHEMA_VERSION) {
                    return Err(ScanError::Invalid);
                }
                if row.get_str("scan_token").ok() != Some(token.as_str()) {
                    return Ok(None);
                }
                let now = row
                    .get_datetime("scan_started_at")
                    .map_err(|_| ScanError::Invalid)?;
                let position =
                    bson::deserialize_from_document(row.clone()).map_err(|_| ScanError::Invalid)?;
                Ok(Some(Lease {
                    token,
                    position,
                    now: *now,
                }))
            }
            Ok(None) => Err(ScanError::Invalid),
            Err(error) if is_duplicate_key(&error) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    pub(crate) async fn page(&self, lease: &Lease) -> Result<Page, ScanError> {
        page(&self.store.agent_commands, &lease.position).await
    }

    /// Index the page by the Agent and its command order. The ownership
    /// snapshot limits discovery only. Settlement still checks live authority.
    pub(crate) async fn owned_page(
        &self,
        agents: &[AgentId],
        position: &OwnedPosition,
        lane: Lane,
    ) -> Result<Page, ScanError> {
        if agents.is_empty() {
            return Ok(Page {
                eof: true,
                rows: Vec::new(),
            });
        }
        let mut filter = doc! {
            "agent_id": {"$in": agents.iter().map(AgentId::as_str).collect::<Vec<_>>()},
            "status": lane.status(),
        };
        let after = match lane {
            Lane::InProgress => &position.in_progress.after,
            Lane::WaitingAction => &position.waiting_action.after,
        };
        if let Some((agent, order)) = after {
            filter.insert(
                "$or",
                vec![
                    doc! {"agent_id": {"$gt": agent}},
                    doc! {"agent_id": agent, "agent_order": {"$gt": order}},
                ],
            );
        }
        let mut projection = candidate_projection();
        projection.insert("agent_order", 1);
        // Use the same database clock as the shared observation lease.
        projection.insert("scan_now", "$$NOW");
        let rows: Vec<_> = self
            .store
            .agent_commands
            .find(filter)
            .sort(doc! {"agent_id": 1, "agent_order": 1})
            .hint(Hint::Name("agent_command_queue".into()))
            .projection(projection)
            .limit(-i64::from(PAGE_SIZE))
            .batch_size(PAGE_SIZE)
            .max_time(QUERY_DEADLINE)
            .await?
            .try_collect()
            .await?;
        Ok(Page {
            eof: rows.len() < PAGE_SIZE as usize,
            rows,
        })
    }

    /// Returning false means another replica may have taken the scan. Progress
    /// from a stale holder must never overwrite the successor's position.
    pub(crate) async fn advance(
        &self,
        lease: &Lease,
        last_examined: Option<&str>,
        eof: bool,
    ) -> Result<bool, ScanError> {
        let position = &lease.position;
        let (lane, after, wrap) = match (eof, position.lane) {
            (true, Lane::InProgress) => (Lane::WaitingAction, None, false),
            (true, Lane::WaitingAction) => (Lane::InProgress, None, true),
            (false, lane) => (lane, last_examined.or(position.after.as_deref()), false),
        };
        let fields = doc! {
            "lane": lane.status(),
            "after": {"$literal": after},
            "wrapped_at": if wrap { "$$NOW" } else { "$wrapped_at" },
        };
        let result = self
            .state
            .update_one(
                doc! {"_id": {"scan": SCAN_ID}, "scan_token": &lease.token,
                "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                "$expr": {"$gt": ["$scan_expires_at", "$$NOW"]}},
                vec![doc! {"$set": fields}, doc! {"$unset": "scan_token"}],
            )
            .await?;
        if result.matched_count == 1 && wrap {
            if let Some(previous) = position.wrapped_at {
                let elapsed = lease.now.timestamp_millis() - previous.timestamp_millis();
                metrics::histogram!("cma_stale_turn_full_traversal_seconds")
                    .record(elapsed.max(0) as f64 / 1000.0);
            }
        }
        Ok(result.matched_count == 1)
    }
}

async fn page(collection: &Collection<Document>, position: &Position) -> Result<Page, ScanError> {
    let mut filter = doc! {"status": position.lane.status()};
    if let Some(after) = &position.after {
        filter.insert("_id", doc! {"$gt": after});
    }
    let rows: Vec<_> = collection
        .find(filter)
        .sort(doc! {"_id": 1})
        .hint(Hint::Name("agent_command_work_scan".into()))
        .projection(candidate_projection())
        .limit(-i64::from(PAGE_SIZE))
        .batch_size(PAGE_SIZE)
        .max_time(QUERY_DEADLINE)
        .await?
        .try_collect()
        .await?;
    Ok(Page {
        eof: rows.len() < PAGE_SIZE as usize,
        rows,
    })
}

fn candidate_projection() -> Document {
    doc! {
        "_id": 1, "agent_id": 1, "status": 1, "record.receipt.envelope.command.type": 1,
        "record.work.dispatch.fenced_at": 1, "record.work.dispatch.delivery": 1,
        "record.work.observation.delivery": 1,
        "record.work.dispatch.turn_sequence": 1, "record.work.dispatch.runtime_generation": 1,
    }
}
