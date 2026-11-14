//! Disposable daily/hourly projections; the allocated ledger remains authoritative.
mod boundary;
mod materialize;
mod publication;
mod reads;
mod schema;
mod summary;
mod worker;
pub(crate) use worker::Lane;

use super::usage_reports::{expires, ReportingStore, CONTROL, DB_DEADLINE};
use layer2_core::{
    sandbox_metrics::{allocation, MetricsError, Range, Result},
    sandbox_usage::{Allocation, SandboxUsage},
};
use mongodb::bson::{doc, Document};
use serde::{Deserialize, Serialize};

pub(crate) const COLLECTION: &str = "sandbox_usage_allocation_buckets_v1";
const DAY: u64 = 86_400;
const HOUR: u64 = 3600;
const MAX_BUCKET_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Slice {
    range: Range,
    projection: allocation::Projection,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DayBucket {
    version: u32,
    sandbox_id: String,
    owner: String,
    revision: u64,
    source_seq: u64,
    source_revision: u64,
    allocation: Allocation,
    created_at: u64,
    tracking_started_at: u64,
    complete_lifecycle: bool,
    last_observed_at: u64,
    day: u64,
    total: Slice,
    hours: Vec<Slice>,
    // Boundary inputs have their own digest and are excluded from summary IO.
    #[serde(skip)]
    boundary: Option<boundary::Boundary>,
}

impl DayBucket {
    fn key(&self) -> String {
        format!("{}:{}", self.sandbox_id, self.day)
    }

    fn validate_source(&self, state: &SandboxUsage) -> Result<bool> {
        if self.sandbox_id != state.sandbox_id.as_str()
            || self.owner != state.owner.as_str()
            || self.created_at != state.created_at
            || self.tracking_started_at != state.tracking_started_at
            || self.complete_lifecycle != state.complete_lifecycle
        {
            return Err(MetricsError::Unavailable);
        }
        if self.revision == state.revision {
            if self.source_seq != state.source_seq
                || self.source_revision != state.source_revision
                || self.last_observed_at != state.last_observed_at
                || self.allocation != state.allocation
            {
                return Err(MetricsError::Unavailable);
            }
            return Ok(true);
        }
        if self.revision < state.revision
            && (self.last_observed_at > state.last_observed_at
                || self.source_seq > state.source_seq
                || self.source_revision > state.source_revision)
        {
            return Err(MetricsError::Unavailable);
        }
        // Only finalized evidence can outlive its captured source revision.
        // A projected tail may be superseded by a newly committed interval.
        Ok(self.revision < state.revision
            && self.last_observed_at <= state.last_observed_at
            && self.total.projection.period.complete
            && self.total.projection.period.projected_tail_seconds == 0)
    }
}

// Fixed outcomes only: driver errors can include collection filters or identities.
fn unavailable(_: impl std::fmt::Display) -> MetricsError {
    MetricsError::Unavailable
}

fn encode<T: Serialize>(value: &T) -> Result<mongodb::bson::Bson> {
    mongodb::bson::serialize_to_bson(value).map_err(unavailable)
}

fn decode<T: serde::de::DeserializeOwned>(document: &Document) -> Result<T> {
    if document.get_i64("document_schema_version").ok() != Some(1) {
        return Err(MetricsError::Unavailable);
    }
    mongodb::bson::deserialize_from_bson(
        document
            .get("record")
            .cloned()
            .ok_or(MetricsError::Unavailable)?,
    )
    .map_err(unavailable)
}

#[cfg(test)]
mod read_fixture;
#[cfg(test)]
mod tests;
