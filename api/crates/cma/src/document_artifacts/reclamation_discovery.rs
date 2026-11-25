//! Durable bounded inventory traversal. Storage effects run on a separate lane.
use super::*;
use futures_util::{stream, StreamExt};
use layer2_core::document_artifacts::digest;
use std::time::Duration;

pub(in crate::document_artifacts) const DISCOVERY_INTERVAL: Duration = Duration::from_secs(5);
pub(in crate::document_artifacts) const DISCOVERY_SLICE: Duration = Duration::from_secs(55);
const CLAIM_CONCURRENCY: usize = 8;

#[derive(Clone, Copy)]
pub(in crate::document_artifacts) enum ScanLane {
    Discovery,
    Effect,
}
impl ScanLane {
    fn fields(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Self::Discovery => (
                "reclamation_scan_after",
                "reclamation_scan_serial",
                "object:",
            ),
            Self::Effect => (
                "reclamation_effect_after",
                "reclamation_effect_serial",
                "source_commit:",
            ),
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::Discovery => "discovery",
            Self::Effect => "effect",
        }
    }
}

pub(in crate::document_artifacts) struct Cursor {
    pub after: Option<String>,
    serial: u64,
    binding: Binding,
}

impl ReclamationStore {
    pub(in crate::document_artifacts) async fn cursor(
        &self,
        lane: ScanLane,
    ) -> Result<Cursor, DocumentError> {
        let state = self
            .ledger
            .find_one(doc! {"_id":quota::STATE})
            .max_time(quota::IO)
            .await
            .map_err(quota::unavailable)?
            .ok_or(DocumentError::Unavailable)?;
        let binding = Binding::read(&state, &self.namespace)?;
        if !quota::generation_enabled(&state)? {
            return Err(DocumentError::Unavailable);
        }
        let (after_field, serial_field, prefix) = lane.fields();
        let after = match state.get(after_field) {
            None | Some(mongodb::bson::Bson::Null) => None,
            Some(mongodb::bson::Bson::String(key)) if key.starts_with(prefix) => Some(key.clone()),
            _ => return Err(DocumentError::Corrupt),
        };
        let serial = if state.contains_key(serial_field) {
            quota::number(&state, serial_field)?
        } else {
            0
        };
        Ok(Cursor {
            after,
            serial,
            binding,
        })
    }

    pub(in crate::document_artifacts) async fn advance_cursor(
        &self,
        lane: ScanLane,
        cursor: Cursor,
        next: Option<String>,
    ) -> Result<(), DocumentError> {
        let (after_field, serial_field, _) = lane.fields();
        let next_serial = cursor
            .serial
            .checked_add(1)
            .filter(|value| *value <= i64::MAX as u64)
            .ok_or(DocumentError::Capacity)?;
        let serial_filter = if cursor.serial == 0 {
            doc! {"$or":[{serial_field:{"$exists":false}},{serial_field:0_i64}]}
        } else {
            doc! {serial_field:cursor.serial as i64}
        };
        let binding = cursor.binding;
        let mut filter = doc! {"_id":quota::STATE, "phase":"ready", "source_generations":true,
        "activation_id":&binding.activation,"inventory_sha256":&binding.inventory,"storage_namespace_sha256":&binding.namespace,
        "storage_namespace_bound_at":binding.bound_at};
        filter.extend(serial_filter);
        self.ledger
            .update_one(
                filter,
                doc! {"$set":{after_field:next,serial_field:next_serial as i64}},
            )
            .await
            .map_err(quota::unavailable)?;
        Ok(())
    }

    fn repair_id(lane: ScanLane, key: &str) -> String {
        format!(
            "reclamation_repair:{}:{}",
            lane.label(),
            digest(key.as_bytes())
        )
    }

    /// Keep corrupt or ambiguous charges and provide durable repair evidence.
    /// The ordinary cursor wraps and reconsiders them during the next sweep.
    pub(in crate::document_artifacts) async fn repair(
        &self,
        lane: ScanLane,
        key: &str,
        error: DocumentError,
        now: DateTime,
    ) -> Result<(), DocumentError> {
        let id = Self::repair_id(lane, key);
        self.ledger.update_one(doc! {"_id":id}, doc! {
            "$setOnInsert":{"document_schema_version":1_i64,"kind":"reclamation_repair","row_key":key,"lane":lane.label(),"first_seen_at":now,"storage_namespace_sha256":&self.namespace},
            "$set":{"last_error":error.to_string()},"$max":{"last_seen_at":now},"$inc":{"attempts":1_i64},
        }).upsert(true).await.map_err(quota::unavailable)?;
        tracing::warn!(lane=lane.label(), error=%error, "document reclamation retained an unresolved ledger row");
        Ok(())
    }

    pub(in crate::document_artifacts) async fn resolved(
        &self,
        lane: ScanLane,
        keys: &[String],
        now: DateTime,
    ) -> Result<(), DocumentError> {
        if keys.is_empty() {
            return Ok(());
        }
        let ids: Vec<_> = keys.iter().map(|key| Self::repair_id(lane, key)).collect();
        self.ledger
            .delete_many(doc! {"_id":{"$in":ids},"last_seen_at":{"$lt":now}})
            .await
            .map_err(quota::unavailable)?;
        Ok(())
    }

    pub(in crate::document_artifacts) async fn discover_page(
        &self,
        now: DateTime,
    ) -> Result<(), DocumentError> {
        let lane = ScanLane::Discovery;
        let cursor = self.cursor(lane).await?;
        let legacy_mature = cursor.binding.bound_at <= mature_before(now);
        let page = self
            .accounting_page(cursor.after.as_deref(), now, legacy_mature)
            .await?;
        let next = page.last().map(|entry| entry.key.clone());
        let resolved = stream::iter(page)
            .map(|entry| async move {
                let outcome = match (entry.error, entry.candidate) {
                    (Some(error), _) => Err(error),
                    (None, Some(candidate)) => self.claim(&candidate, now).await.map(|_| ()),
                    (None, None) => Ok(()),
                };
                match outcome {
                    Ok(()) => Ok::<_, DocumentError>(Some(entry.key)),
                    Err(error) => {
                        self.repair(lane, &entry.key, error, now).await?;
                        Ok(None)
                    }
                }
            })
            .buffer_unordered(CLAIM_CONCURRENCY)
            .try_collect::<Vec<_>>()
            .await?;
        self.resolved(
            lane,
            &resolved.into_iter().flatten().collect::<Vec<_>>(),
            now,
        )
        .await?;
        self.advance_cursor(lane, cursor, next).await
    }
}

#[cfg(test)]
#[test]
fn document_quota_reclamation_discovery_cadence_covers_supported_baseline() {
    let seconds = 24 * 3600 - 3600;
    let pass = DISCOVERY_INTERVAL.as_secs() + DISCOVERY_SLICE.as_secs();
    assert!(
        seconds / pass * ACCOUNTING_LIMIT as u64
            > super::super::quota_baseline::MAX_BASELINE_OBJECTS as u64
    );
}
