//! Independently checksummed daily projections avoid transferring 24 unused hours.
use super::*;
use sha2::{Digest, Sha256};

impl DayBucket {
    pub(super) fn summary_document(&self) -> Result<Document> {
        // Copy only the daily projection: hourly and boundary payloads can be
        // much larger and are deliberately absent from the common read path.
        let summary = Self {
            version: self.version,
            sandbox_id: self.sandbox_id.clone(),
            owner: self.owner.clone(),
            revision: self.revision,
            source_seq: self.source_seq,
            source_revision: self.source_revision,
            allocation: self.allocation.clone(),
            created_at: self.created_at,
            tracking_started_at: self.tracking_started_at,
            complete_lifecycle: self.complete_lifecycle,
            last_observed_at: self.last_observed_at,
            day: self.day,
            total: self.total.clone(),
            hours: Vec::new(),
            boundary: None,
        };
        summary.validate_summary_fields()?;
        let canonical = mongodb::bson::serialize_to_vec(&summary).map_err(unavailable)?;
        if canonical.len() > MAX_BUCKET_BYTES {
            return Err(MetricsError::Busy);
        }
        let digest = format!("{:x}", Sha256::digest(&canonical));
        Ok(doc! {
            "summary_record": mongodb::bson::Binary {
                subtype: mongodb::bson::spec::BinarySubtype::Generic,
                bytes: canonical,
            },
            "summary_sha256": digest,
        })
    }

    pub fn read_summary(document: &Document) -> Result<Self> {
        if document.get_i64("document_schema_version").ok() != Some(1) {
            return Err(MetricsError::Unavailable);
        }
        let canonical = document
            .get_binary_generic("summary_record")
            .map_err(unavailable)?;
        if canonical.len() > MAX_BUCKET_BYTES
            || document.get_str("summary_sha256").ok()
                != Some(format!("{:x}", Sha256::digest(canonical)).as_str())
        {
            return Err(MetricsError::Unavailable);
        }
        // Decode the authenticated bytes once. Reconstructing a BSON tree and
        // reserializing every expectation made long-range reads CPU bound.
        let summary: Self =
            mongodb::bson::deserialize_from_slice(canonical).map_err(unavailable)?;
        summary.validate_summary_fields()?;
        if !summary.hours.is_empty()
            || document.get_str("_id").ok() != Some(summary.key().as_str())
            || document.get_str("sandbox_id").ok() != Some(summary.sandbox_id.as_str())
            || document.get_str("owner").ok() != Some(summary.owner.as_str())
            || document.get_i64("day").ok() != Some(summary.day as i64)
            || document.get_i64("revision").ok() != Some(summary.revision as i64)
            || document.get_i64("cutoff").ok() != Some(summary.total.range.to as i64)
        {
            return Err(MetricsError::Unavailable);
        }
        Ok(summary)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bucket() -> DayBucket {
        DayBucket {
            version: 1,
            sandbox_id: "summary-fixture".into(),
            owner: "owner-fixture".into(),
            revision: 1,
            source_seq: 1,
            source_revision: 1,
            allocation: Allocation {
                running: false,
                cpu_millis: 137,
                memory_mib: 733,
                disk_gib: 19,
            },
            created_at: 0,
            tracking_started_at: 0,
            complete_lifecycle: true,
            last_observed_at: DAY,
            day: 0,
            total: Slice {
                range: Range { from: 0, to: DAY },
                projection: allocation::Projection {
                    period: layer2_core::sandbox_metrics::AllocatedPeriod {
                        complete: true,
                        ..Default::default()
                    },
                    expectations: vec![allocation::Expectation {
                        from: 0,
                        to: DAY,
                        running: Some(false),
                        home: Some(true),
                    }],
                },
            },
            hours: Vec::new(),
            boundary: None,
        }
    }

    fn document(bucket: &DayBucket) -> Document {
        let mut doc = bucket.summary_document().unwrap();
        doc.extend(doc! {
            "document_schema_version": 1_i64, "_id":bucket.key(),
            "sandbox_id":&bucket.sandbox_id,"owner":&bucket.owner,
            "day":bucket.day as i64,"revision":bucket.revision as i64,
            "cutoff":bucket.total.range.to as i64,
        });
        doc
    }

    #[test]
    fn binary_summary_preserves_exact_values_and_refuses_mutated_bytes_or_identity() {
        let b = bucket();
        let valid = document(&b);
        let read = DayBucket::read_summary(&valid).unwrap();
        assert_eq!(read.total.projection.period, b.total.projection.period);
        assert_eq!(
            read.total.projection.expectations,
            b.total.projection.expectations
        );
        assert!(read.hours.is_empty());
        for field in [
            "_id",
            "sandbox_id",
            "owner",
            "day",
            "revision",
            "cutoff",
            "summary_sha256",
        ] {
            let mut bad = valid.clone();
            bad.insert(field, "altered");
            assert!(DayBucket::read_summary(&bad).is_err(), "{field}");
        }
        let mut bad = valid.clone();
        let mut bytes = bad.get_binary_generic("summary_record").unwrap().to_vec();
        bytes[8] ^= 1;
        bad.insert(
            "summary_record",
            mongodb::bson::Binary {
                subtype: mongodb::bson::spec::BinarySubtype::Generic,
                bytes,
            },
        );
        assert!(DayBucket::read_summary(&bad).is_err());
    }

    #[test]
    fn authenticated_invalid_summary_is_still_refused() {
        let b = bucket();
        for mode in 0..4 {
            let mut invalid = b.clone();
            match mode {
                0 => invalid.total.projection.period.cpu_core_hours = f64::NAN,
                1 => invalid.total.range.to -= 1,
                2 => invalid.total.projection.expectations[0].from = 1,
                _ => invalid.hours.push(invalid.total.clone()),
            }
            let bytes = mongodb::bson::serialize_to_vec(&invalid).unwrap();
            let mut bad = document(&b);
            bad.insert("summary_sha256", format!("{:x}", Sha256::digest(&bytes)));
            bad.insert(
                "summary_record",
                mongodb::bson::Binary {
                    subtype: mongodb::bson::spec::BinarySubtype::Generic,
                    bytes,
                },
            );
            assert!(DayBucket::read_summary(&bad).is_err(), "mode {mode}");
        }
    }
}
