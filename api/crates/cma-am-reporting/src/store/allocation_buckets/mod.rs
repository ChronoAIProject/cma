//! Disposable daily and hourly projections over the allocated ledger.
mod boundary;
mod materialize;
mod reads;
mod schema;
mod summary;

use super::usage_reports::expires;
use layer2_core::{
    sandbox_metrics::{allocation, MetricsError, Range, Result},
    sandbox_usage::{Allocation, SandboxUsage},
};
pub use materialize::build_covered;
use mongodb::bson::{doc, Document};
pub use reads::read_bucket_page;
use serde::{Deserialize, Serialize};

pub const COLLECTION: &str = "sandbox_usage_allocation_buckets_v1";
pub const DAY: u64 = 86_400;
pub const HOUR: u64 = 3600;
const MAX_BUCKET_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Slice {
    pub range: Range,
    pub projection: allocation::Projection,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DayBucket {
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
    pub fn total(&self) -> &Slice {
        &self.total
    }

    pub fn hours(&self) -> &[Slice] {
        &self.hours
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn last_observed_at(&self) -> u64 {
        self.last_observed_at
    }

    pub fn has_boundary(&self) -> bool {
        self.boundary.is_some()
    }

    pub fn key(&self) -> String {
        format!("{}:{}", self.sandbox_id, self.day)
    }

    /// Check whether publication may replace this bucket under its worker fence.
    pub fn may_replace(
        &self,
        bucket: &Self,
        existing: &Document,
        document: &Document,
    ) -> Result<bool> {
        if self.owner != bucket.owner
            || self.created_at != bucket.created_at
            || self.tracking_started_at != bucket.tracking_started_at
            || self.revision > bucket.revision
            || self.last_observed_at > bucket.last_observed_at
            || self.source_seq > bucket.source_seq
            || self.source_revision > bucket.source_revision
        {
            return Err(MetricsError::Unavailable);
        }
        let baseline_matches = self.baseline_matches(bucket.revision, bucket.complete_lifecycle)?;
        if baseline_matches && self.total.range.to > bucket.total.range.to {
            // A concurrent lane has already prepared more coverage.
            // Keep it; the reader still verifies its captured revision.
            return Ok(false);
        }
        if self.revision == bucket.revision
            && (self.source_seq != bucket.source_seq
                || self.source_revision != bucket.source_revision
                || self.allocation != bucket.allocation
                || self.last_observed_at != bucket.last_observed_at
                || (self.total.range == bucket.total.range
                    && existing.get("content_sha256") != document.get("content_sha256")))
        {
            return Err(MetricsError::Unavailable);
        }
        if self.revision == bucket.revision
            && self
                .boundary
                .as_ref()
                .zip(bucket.boundary.as_ref())
                .is_some_and(|(prior, next)| {
                    prior.queried_to > next.queried_to
                        || (prior.queried_to == next.queried_to && prior != next)
                })
        {
            return Err(MetricsError::Unavailable);
        }
        Ok(true)
    }

    fn baseline_matches(&self, revision: u64, complete_lifecycle: bool) -> Result<bool> {
        if self.complete_lifecycle == complete_lifecycle {
            return Ok(true);
        }
        // The supported rebaseline writer excludes an uncertain ledger tail.
        // That forward transition invalidates prepared coverage, not identity.
        if self.complete_lifecycle && !complete_lifecycle && self.revision < revision {
            return Ok(false);
        }
        Err(MetricsError::Unavailable)
    }

    pub fn validate_source(&self, state: &SandboxUsage) -> Result<bool> {
        if self.sandbox_id != state.sandbox_id.as_str()
            || self.owner != state.owner.as_str()
            || self.created_at != state.created_at
            || self.tracking_started_at != state.tracking_started_at
        {
            return Err(MetricsError::Unavailable);
        }
        let baseline_matches = self.baseline_matches(state.revision, state.complete_lifecycle)?;
        if self.revision == state.revision {
            if self.source_seq != state.source_seq
                || self.source_revision != state.source_revision
                || self.last_observed_at != state.last_observed_at
                || self.allocation != state.allocation
            {
                return Err(MetricsError::Unavailable);
            }
            return Ok(self.total.projection.period.projected_tail_seconds == 0
                || state.current_allocation() == state.allocation);
        }
        if self.revision < state.revision
            && (self.last_observed_at > state.last_observed_at
                || self.source_seq > state.source_seq
                || self.source_revision > state.source_revision)
        {
            return Err(MetricsError::Unavailable);
        }
        if !baseline_matches {
            return Ok(false);
        }
        // observe_inner appends at the prior last_observed_at; rebaseline only
        // moves that boundary forward. A fully queried closed day therefore
        // remains exact even when its historical unknown/gap values are nonzero.
        // The baseline checks above still refuse a change in tracking semantics.
        let closed = self.last_observed_at >= self.day + DAY
            && (self.total.range.to == self.day + DAY
                || self
                    .boundary
                    .as_ref()
                    .is_some_and(|input| input.queried_to == self.day + DAY));
        Ok(self.revision < state.revision
            && self.last_observed_at <= state.last_observed_at
            && (self.total.projection.period.complete || closed)
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
