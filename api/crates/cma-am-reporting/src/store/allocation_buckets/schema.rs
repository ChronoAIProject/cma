use super::*;
use sha2::{Digest, Sha256};

impl DayBucket {
    pub fn document(&self) -> Result<Document> {
        self.validate()?;
        let record = encode(self)?;
        let canonical = mongodb::bson::serialize_to_vec(self).map_err(unavailable)?;
        if canonical.len() > MAX_BUCKET_BYTES {
            return Err(MetricsError::Busy);
        }
        let mut document = doc! {
            "_id": self.key(), "document_schema_version": 1_i64,
            "sandbox_id": &self.sandbox_id, "owner": &self.owner,
            "day": self.day as i64, "revision": self.revision as i64,
            "cutoff": self.total.range.to as i64,
            "content_sha256": format!("{:x}", Sha256::digest(canonical)),
            "expires_at": expires(self.day + 32 * DAY), "record": record,
        };
        document.extend(self.summary_document()?);
        if let Some(boundary) = &self.boundary {
            document.extend(boundary.document(self, &document)?);
        }
        Ok(document)
    }

    pub fn read(document: &Document) -> Result<Self> {
        let mut bucket: Self = decode(document)?;
        let expected = bucket.document()?;
        for field in [
            "_id",
            "sandbox_id",
            "owner",
            "day",
            "revision",
            "cutoff",
            "content_sha256",
            "summary_record",
            "summary_sha256",
        ] {
            if document.get(field) != expected.get(field) {
                return Err(MetricsError::Unavailable);
            }
        }
        bucket.boundary = boundary::Boundary::read(document, &bucket)?;
        Ok(bucket)
    }

    pub(super) fn validate(&self) -> Result<()> {
        self.validate_summary_fields()?;
        if self.hours.len() != ((self.total.range.to - self.day) / HOUR) as usize {
            return Err(MetricsError::Unavailable);
        }
        for (index, hour) in self.hours.iter().enumerate() {
            if hour.range
                != (Range {
                    from: self.day + index as u64 * HOUR,
                    to: self.day + (index as u64 + 1) * HOUR,
                })
            {
                return Err(MetricsError::Unavailable);
            }
            self.validate_slice(hour)?;
        }
        Ok(())
    }

    pub(super) fn validate_summary_fields(&self) -> Result<()> {
        if self.version != 1
            || !self.day.is_multiple_of(DAY)
            || self.day > i64::MAX as u64 / 1000 - 32 * DAY
            || self.revision > i64::MAX as u64
            || self.total.range.from != self.day
            || self.total.range.to <= self.day
            || self.total.range.to > self.day + DAY
            || !self.total.range.to.is_multiple_of(HOUR)
            || self.sandbox_id.is_empty()
            || self.owner.is_empty()
            || self.created_at > self.tracking_started_at
            || self.tracking_started_at > self.last_observed_at
        {
            return Err(MetricsError::Unavailable);
        }
        self.validate_slice(&self.total)
    }

    fn validate_slice(&self, slice: &Slice) -> Result<()> {
        let p = &slice.projection.period;
        if [
            p.uptime_seconds,
            p.disk_retained_seconds,
            p.cpu_core_hours,
            p.memory_gib_hours,
            p.disk_gib_hours,
        ]
        .iter()
        .any(|v| !v.is_finite() || *v < 0.0)
            || p.unknown_seconds > slice.range.seconds()
            || p.projected_tail_seconds > slice.range.seconds()
            || (p.complete && p.unknown_seconds != 0)
        {
            return Err(MetricsError::Unavailable);
        }
        let mut end = slice.range.from.max(self.created_at).min(slice.range.to);
        for expectation in &slice.projection.expectations {
            if expectation.from != end || expectation.to <= end || expectation.to > slice.range.to {
                return Err(MetricsError::Unavailable);
            }
            end = expectation.to;
        }
        if end != slice.range.to {
            return Err(MetricsError::Unavailable);
        }
        Ok(())
    }
}
