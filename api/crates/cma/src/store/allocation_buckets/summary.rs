//! Independently checksummed daily projections avoid transferring 24 unused hours.
use super::*;
use sha2::{Digest, Sha256};

impl DayBucket {
    pub(super) fn summary_document(&self) -> Result<Document> {
        let mut summary = self.clone();
        summary.hours.clear();
        summary.validate_summary_fields()?;
        let canonical = mongodb::bson::serialize_to_vec(&summary).map_err(unavailable)?;
        if canonical.len() > MAX_BUCKET_BYTES {
            return Err(MetricsError::Busy);
        }
        Ok(doc! {
            "summary_record": encode(&summary)?,
            "summary_sha256": format!("{:x}", Sha256::digest(canonical)),
        })
    }

    pub(super) fn read_summary(document: &Document) -> Result<Self> {
        let summary: Self = decode(&doc! {
            "document_schema_version": document.get("document_schema_version")
                .cloned().ok_or(MetricsError::Unavailable)?,
            "record": document.get("summary_record").cloned().ok_or(MetricsError::Unavailable)?,
        })?;
        if !summary.hours.is_empty() {
            return Err(MetricsError::Unavailable);
        }
        let mut expected = summary.summary_document()?;
        expected.extend(doc! {
            "_id":summary.key(),"sandbox_id":&summary.sandbox_id,"owner":&summary.owner,
            "day":summary.day as i64,"revision":summary.revision as i64,
            "cutoff":summary.total.range.to as i64,
        });
        for (field, value) in expected {
            if document.get(&field) != Some(&value) {
                return Err(MetricsError::Unavailable);
            }
        }
        Ok(summary)
    }
}
