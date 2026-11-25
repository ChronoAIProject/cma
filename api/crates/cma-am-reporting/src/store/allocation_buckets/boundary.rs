//! Compact exact fold inputs for partial UTC days. Daily numeric totals are never split.
use super::*;
use layer2_core::sandbox_usage::{UsageEvent, UsageEventKind, UsageTotals};
use mongodb::bson::{spec::BinarySubtype, Binary, Bson};
use sha2::{Digest, Sha256};
mod delta;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    revision: u64,
    kind: UsageEventKind,
    started_at: u64,
    ended_at: u64,
    allocation: Allocation,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Boundary {
    pub(super) queried_to: u64,
    scope_valid: bool,
    events: Vec<Input>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Bound<T> {
    source_sha256: String,
    input: T,
}

impl Boundary {
    fn events(&self, state: &SandboxUsage) -> Vec<UsageEvent> {
        self.events
            .iter()
            .map(|e| UsageEvent {
                sandbox_id: state.sandbox_id.clone(),
                owner: state.owner.clone(),
                revision: e.revision,
                kind: e.kind,
                started_at: e.started_at,
                ended_at: e.ended_at,
                allocation: e.allocation.clone(),
                // The original projector does not consume these ledger fields.
                source_seq: 0,
                next_allocation: e.allocation.clone(),
                status: String::new(),
                quantity: UsageTotals::default(),
            })
            .collect()
    }

    pub(super) fn new(state: &SandboxUsage, events: &[UsageEvent], queried_to: u64) -> Self {
        let scope_valid = events
            .iter()
            .all(|e| e.sandbox_id == state.sandbox_id && e.owner == state.owner);
        Self {
            queried_to,
            scope_valid,
            // Foreign scope must retain the raw path's incomplete semantics.
            // Never erase a foreign identity and reconstruct it as this owner.
            events: if scope_valid {
                events
                    .iter()
                    .map(|e| Input {
                        revision: e.revision,
                        kind: e.kind,
                        started_at: e.started_at,
                        ended_at: e.ended_at,
                        allocation: e.allocation.clone(),
                    })
                    .collect()
            } else {
                Vec::new()
            },
        }
    }

    fn validate(&self, bucket: &DayBucket) -> Result<()> {
        if self.queried_to < bucket.total.range.to
            || self.queried_to > bucket.day + DAY
            || self.events.len() > 100_000
            || (!self.scope_valid && !self.events.is_empty())
            || self
                .events
                .windows(2)
                .any(|p| p[0].revision >= p[1].revision)
            || self.events.iter().any(|e| {
                e.revision > bucket.revision
                    || e.started_at > e.ended_at
                    || e.ended_at > bucket.last_observed_at
                    || e.ended_at <= bucket.day
                    || e.started_at >= self.queried_to
            })
        {
            return Err(MetricsError::Unavailable);
        }
        Ok(())
    }

    pub(super) fn document(&self, bucket: &DayBucket, source: &Document) -> Result<Document> {
        self.validate(bucket)?;
        let bound = Bound {
            source_sha256: source
                .get_str("summary_sha256")
                .map_err(unavailable)?
                .into(),
            input: self,
        };
        let bytes = mongodb::bson::serialize_to_vec(&bound).map_err(unavailable)?;
        if bytes.len() > MAX_BUCKET_BYTES {
            return Err(MetricsError::Busy);
        }
        Ok(doc! {
            "boundary_sha256": format!("{:x}", Sha256::digest(&bytes)),
            "boundary_record": Bson::Binary(Binary { subtype: BinarySubtype::Generic, bytes }),
        })
    }

    pub(super) fn read(document: &Document, bucket: &DayBucket) -> Result<Option<Self>> {
        if !document.contains_key("boundary_record") && !document.contains_key("boundary_sha256") {
            return Ok(None);
        }
        let bytes = document
            .get_binary_generic("boundary_record")
            .map_err(unavailable)?;
        if bytes.len() > MAX_BUCKET_BYTES {
            return Err(MetricsError::Busy);
        }
        if document.get_str("boundary_sha256").map_err(unavailable)?
            != format!("{:x}", Sha256::digest(bytes))
        {
            return Err(MetricsError::Unavailable);
        }
        let bound: Bound<Self> =
            mongodb::bson::deserialize_from_slice(bytes).map_err(unavailable)?;
        if bound.source_sha256 != document.get_str("summary_sha256").map_err(unavailable)? {
            return Err(MetricsError::Unavailable);
        }
        bound.input.validate(bucket)?;
        Ok(Some(bound.input))
    }
}

impl DayBucket {
    pub fn read_boundary(document: &Document) -> Result<Self> {
        let mut bucket = Self::read_summary(document)?;
        bucket.boundary = Boundary::read(document, &bucket)?;
        Ok(bucket)
    }

    pub fn boundary_prepared(&self, state: &SandboxUsage) -> bool {
        self.boundary.as_ref().is_some_and(|input| {
            input.queried_to == self.day + DAY
                && (self.revision == state.revision || self.last_observed_at >= self.day + DAY)
        })
    }

    pub fn project_boundary(
        &self,
        state: &SandboxUsage,
        range: Range,
    ) -> Result<Option<allocation::Projection>> {
        if range.from < self.day || range.to > self.day + DAY || range.from >= range.to {
            return Err(MetricsError::Unavailable);
        }
        if !self.validate_source(state)? {
            return Ok(None);
        }
        let Some(input) = &self.boundary else {
            return Ok(None);
        };
        if !input.scope_valid
            || (input.queried_to < range.to && input.queried_to < self.last_observed_at)
            || (self.revision != state.revision && self.last_observed_at < range.to)
        {
            return Ok(None);
        }
        let events = input.events(state);
        Ok(Some(allocation::project(state, &events, range)))
    }
}
