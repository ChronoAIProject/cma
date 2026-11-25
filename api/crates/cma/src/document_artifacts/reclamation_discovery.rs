//! Durable bounded inventory traversal. Storage effects run on a separate lane.
use super::*;
use futures_util::{stream, StreamExt};
use std::time::Duration;

pub(in crate::document_artifacts) const DISCOVERY_INTERVAL: Duration = Duration::from_secs(5);
pub(in crate::document_artifacts) const DISCOVERY_SLICE: Duration = Duration::from_secs(55);
const CLAIM_CONCURRENCY: usize = 8;

impl ReclamationStore {
    /// Advance the durable cursor only after every due source in the page has
    /// been considered. A persisted delete claim is work for the effect lane.
    pub(in crate::document_artifacts) async fn discover_page(
        &self,
        now: DateTime,
    ) -> Result<(), DocumentError> {
        let state = self
            .ledger
            .find_one(doc! {"_id":quota::STATE})
            .max_time(quota::IO)
            .await
            .map_err(quota::unavailable)?
            .ok_or(DocumentError::Unavailable)?;
        let binding = Binding::read(&state, &self.namespace)?;
        if !quota::generation_enabled(&state)? {
            return Ok(());
        }
        let after = match state.get("reclamation_scan_after") {
            None | Some(mongodb::bson::Bson::Null) => None,
            Some(mongodb::bson::Bson::String(key))
                if key.starts_with("object:") && key.len() <= 256 =>
            {
                Some(key.as_str())
            }
            _ => return Err(DocumentError::Corrupt),
        };
        let serial = if state.contains_key("reclamation_scan_serial") {
            quota::number(&state, "reclamation_scan_serial")?
        } else {
            0
        };
        let legacy_mature = binding.bound_at <= mature_before(now);
        let page = self.accounting_page(after, now, legacy_mature).await?;
        let next = page.last().map(|entry| entry.key.clone());
        let _: Vec<()> = stream::iter(page.into_iter().filter_map(|entry| entry.candidate))
            .map(|candidate| async move { self.claim(&candidate, now).await.map(|_| ()) })
            .buffer_unordered(CLAIM_CONCURRENCY)
            .try_collect::<Vec<_>>()
            .await?;
        // All claims have durable outcomes. A concurrent scanner may already
        // have advanced this page; its cursor is equally valid.
        let next_serial = serial
            .checked_add(1)
            .filter(|value| *value <= i64::MAX as u64)
            .ok_or(DocumentError::Capacity)?;
        let serial_filter = if serial == 0 {
            doc! {"$or":[{"reclamation_scan_serial":{"$exists":false}},{"reclamation_scan_serial":0_i64}]}
        } else {
            doc! {"reclamation_scan_serial":serial as i64}
        };
        let mut filter = doc! {"_id":quota::STATE, "phase":"ready", "source_generations":true,
        "activation_id":&binding.activation,"inventory_sha256":&binding.inventory,"storage_namespace_sha256":&binding.namespace,
        "storage_namespace_bound_at":binding.bound_at};
        filter.extend(serial_filter);
        self.ledger.update_one(filter, doc! {"$set":{"reclamation_scan_after":next,"reclamation_scan_serial":next_serial as i64}}).await.map_err(quota::unavailable)?;
        Ok(())
    }
}

#[cfg(test)]
#[test]
fn document_quota_reclamation_discovery_cadence_covers_supported_baseline() {
    let seconds = 24 * 3600 - 3600; // legacy writer grace from the durable binding
    let pass = DISCOVERY_INTERVAL.as_secs() + DISCOVERY_SLICE.as_secs();
    assert!(
        seconds / pass * ACCOUNTING_LIMIT as u64
            > super::super::quota_baseline::MAX_BASELINE_OBJECTS as u64
    );
}
