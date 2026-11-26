//! Read a complete v1 custody snapshot before pairing physical samples.
use super::{decode, CaptureError, Document, EvidenceError, MeasuredCapture, RetainedObservation};
use futures_util::TryStreamExt;
use layer2_core::sandbox_metrics::{
    measured_evidence::interval::{
        normalize_series, CustodySample, MeasuredInterval, Quantity, Unresolved,
    },
    Range,
};
use mongodb::ClientSession;
use mongodb::{
    bson::doc,
    options::{Hint, ReadConcern},
};
use serde_json::{json, Value};
use std::{collections::BTreeMap, time::Duration};

pub(super) const MAX_ROWS: usize = 16_384;
const MAX_PAYLOAD_BYTES: usize = 16 * 1024 * 1024;

#[derive(Default)]
pub(super) struct Position {
    sample: Option<RetainedObservation>,
    conflicts: Vec<RetainedObservation>,
}

impl MeasuredCapture {
    pub(super) async fn preview(&self, anchor: &str, window: Range) -> Result<Value, CaptureError> {
        self.preview_bounded(anchor, window, MAX_ROWS).await
    }

    async fn preview_bounded(
        &self,
        anchor: &str,
        window: Range,
        row_limit: usize,
    ) -> Result<Value, CaptureError> {
        let from_ms = window
            .from
            .checked_mul(1000)
            .ok_or(CaptureError::Unavailable)?;
        let to_ms = window
            .to
            .checked_mul(1000)
            .ok_or(CaptureError::Unavailable)?;
        let mut session = self
            .client
            .start_session()
            .snapshot(true)
            .await
            .map_err(|_| CaptureError::Unavailable)?;
        let positions = match &self.indexed {
            Some(indexed) => {
                indexed
                    .read_anchor(anchor, from_ms, to_ms, &mut session)
                    .await?
            }
            None => self.read_positions(&mut session, row_limit).await?,
        };
        let snapshot = session.snapshot_time().ok_or(CaptureError::Unavailable)?;
        let intervals = normalize(&positions, anchor, from_ms..to_ms)?;
        Ok(json!({
            "snapshot": {"seconds": snapshot.time, "increment": snapshot.increment},
            "from_ms": from_ms,
            "to_ms": to_ms,
            "intervals": intervals.iter().map(encode_interval).collect::<Vec<_>>(),
            "status": if intervals.is_empty() { "no_interval_coverage" } else { "preview" },
        }))
    }
    pub(super) async fn read_positions(
        &self,
        session: &mut ClientSession,
        row_limit: usize,
    ) -> Result<BTreeMap<String, Position>, CaptureError> {
        let mut cursor = self
            .rows
            .find(doc! {})
            .sort(doc! {"_id": 1})
            .hint(Hint::Name("_id_".into()))
            .limit(row_limit as i64 + 1)
            .batch_size(64)
            .max_time(Duration::from_secs(15))
            .read_concern(ReadConcern::snapshot())
            .session(&mut *session)
            .await
            .map_err(|_| CaptureError::Unavailable)?;
        let mut stream = cursor.stream(&mut *session);
        let mut positions = BTreeMap::new();
        let mut count = 0;
        let mut payload_bytes = 0;
        while let Some(row) = stream
            .try_next()
            .await
            .map_err(|_| CaptureError::Unavailable)?
        {
            count += 1;
            let payload = row.get_str("payload").map_err(|_| CaptureError::Corrupt)?;
            payload_bytes += payload.len();
            if count > row_limit || payload_bytes > MAX_PAYLOAD_BYTES {
                return Err(CaptureError::Unavailable);
            }
            retain(&mut positions, &row)?;
        }
        drop(stream);
        Ok(positions)
    }
}

pub(super) fn retain(
    positions: &mut BTreeMap<String, Position>,
    row: &Document,
) -> Result<(), CaptureError> {
    let physical = row
        .get_str("physical_sample_id")
        .map_err(|_| CaptureError::Corrupt)?;
    let conflict = match row.get_str("kind") {
        Ok("sample") => false,
        Ok("conflict") => true,
        _ => return Err(CaptureError::Corrupt),
    };
    let evidence = decode(row, physical, conflict)?;
    let position = positions.entry(physical.to_owned()).or_default();
    if conflict {
        position.conflicts.push(evidence);
    } else if position.sample.replace(evidence).is_some() {
        return Err(CaptureError::Corrupt);
    }
    Ok(())
}

fn normalize(
    positions: &BTreeMap<String, Position>,
    anchor: &str,
    window: std::ops::Range<u64>,
) -> Result<Vec<MeasuredInterval>, CaptureError> {
    let anchor = positions
        .get(anchor)
        .and_then(|position| position.sample.as_ref())
        .ok_or(CaptureError::Unavailable)?;
    let mut selected = Vec::new();
    for position in positions.values() {
        let sample = valid_position(position)?;
        if window.contains(&sample.timestamp_ms())
            && anchor
                .shares_physical_series(sample)
                .map_err(|_| CaptureError::Corrupt)?
        {
            selected.push(if position.conflicts.is_empty() {
                CustodySample::Undisputed(sample)
            } else {
                CustodySample::Disputed(sample)
            });
        }
    }
    normalize_series(&selected).map_err(|_| CaptureError::Corrupt)
}

fn valid_position(position: &Position) -> Result<&RetainedObservation, CaptureError> {
    let sample = position.sample.as_ref().ok_or(CaptureError::Corrupt)?;
    if position.conflicts.iter().any(|other| {
        sample.verify_retained_replay(other) != Err(EvidenceError::ConflictingObservation)
    }) {
        return Err(CaptureError::Corrupt);
    }
    Ok(sample)
}

pub(super) fn custody_samples(
    positions: &BTreeMap<String, Position>,
) -> Result<Vec<CustodySample<'_>>, CaptureError> {
    positions
        .values()
        .map(|position| {
            let sample = valid_position(position)?;
            Ok(if position.conflicts.is_empty() {
                CustodySample::Undisputed(sample)
            } else {
                CustodySample::Disputed(sample)
            })
        })
        .collect()
}

fn encode_interval(interval: &MeasuredInterval) -> Value {
    let mut record = json!({
        "policy_version": interval.policy_version,
        "metric": interval.metric,
        "from_ms": interval.from_ms,
        "to_ms": interval.to_ms,
        "evidence_sha256": interval.evidence_sha256,
    });
    match interval.quantity {
        Ok(quantity) => {
            record["status"] = json!("measured");
            record["quantity"] = match quantity {
                Quantity::Cpu(value) => json!({"cpu_seconds": {
                    "significand": value.significand.to_string(), "exponent": value.exponent,
                }}),
                Quantity::ByteMilliseconds(value) => {
                    json!({"byte_milliseconds": value.to_string()})
                }
            };
        }
        Err(reason) => {
            record["status"] = json!("unresolved");
            record["reason"] = json!(match reason {
                Unresolved::Attribution => "attribution",
                Unresolved::Coverage => "coverage",
                Unresolved::CounterReset => "counter_reset",
                Unresolved::Precision => "precision",
                Unresolved::Disputed => "disputed",
            });
        }
    }
    record
}

#[cfg(test)]
mod tests;
