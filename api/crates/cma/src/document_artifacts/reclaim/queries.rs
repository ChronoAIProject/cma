use super::*;
use futures_util::TryStreamExt;
use mongodb::{
    bson::{Bson, DateTime},
    options::Hint,
};

pub(super) fn age_filter(before: i64, activation: Option<DateTime>) -> Document {
    let old = doc! {"reserved_at":{"$lt":DateTime::from_millis(before)}};
    if activation.is_some_and(|time| time.timestamp_millis() < before) {
        doc! {"$or":[old,{"reserved_at":{"$exists":false}}]}
    } else {
        old
    }
}

pub(super) fn reserved_filter(before: i64, activation: Option<DateTime>) -> Document {
    doc! {"kind":"object","$and":[
        {"$or":[{"state":"reserved"},{"state":{"$exists":false}}]},
        age_filter(before,activation)
    ]}
}

impl MongoDocumentReclaimStore {
    pub(super) async fn find_candidates(
        &self,
        before: i64,
        limit: usize,
    ) -> Result<Vec<ObjectCandidate>, DocumentError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let mut result = self
            .candidate_rows(
                doc! {"kind":"object","state":"condemned"},
                before,
                limit,
                true,
            )
            .await?;
        if result.len() < limit {
            let filter = reserved_filter(before, self.activation_time().await?);
            result.extend(
                self.candidate_rows(filter, before, limit - result.len(), false)
                    .await?,
            );
        }
        Ok(result)
    }
    async fn candidate_rows(
        &self,
        filter: Document,
        before: i64,
        limit: usize,
        condemned: bool,
    ) -> Result<Vec<ObjectCandidate>, DocumentError> {
        let rows: Vec<Document> = self
            .rows
            .find(filter)
            .hint(Hint::Keys(doc! {"kind":1,"state":1,"reserved_at":1}))
            .limit(limit as i64)
            .batch_size(limit as u32)
            .max_time(IO)
            .await
            .map_err(unavailable)?
            .try_collect()
            .await
            .map_err(unavailable)?;
        rows.into_iter()
            .map(|row| decode_candidate(&row, before, condemned))
            .collect()
    }
    pub(super) async fn find_backlog(&self, now: i64) -> Result<ReclaimBacklog, DocumentError> {
        let activation = self.activation_time().await?;
        let before = now.saturating_sub(RECLAIM_GRACE.as_millis() as i64);
        let filter =
            doc! {"$or":[{"kind":"object","state":"condemned"},reserved_filter(before,activation)]};
        let fallback = activation.map(Bson::DateTime).unwrap_or(Bson::Null);
        let aggregate:Vec<Document> = self.rows.aggregate([
            doc!{"$match":filter},
            doc!{"$group":{"_id":Bson::Null,"pending":{"$sum":1_i64},
                "oldest":{"$min":{"$ifNull":["$reserved_at",{"$ifNull":["$condemned_at",fallback]}]}}}}
        ]).max_time(IO).await.map_err(unavailable)?.try_collect().await.map_err(unavailable)?;
        let Some(row) = aggregate.first() else {
            return Ok(ReclaimBacklog::default());
        };
        let pending_objects = number(row, "pending")?;
        let oldest = row
            .get_datetime("oldest")
            .map_err(|_| DocumentError::Corrupt)?
            .timestamp_millis();
        Ok(ReclaimBacklog {
            pending_objects,
            oldest_age_seconds: now.saturating_sub(oldest).max(0) as u64 / 1000,
        })
    }
}

fn decode_candidate(
    row: &Document,
    before: i64,
    condemned: bool,
) -> Result<ObjectCandidate, DocumentError> {
    validate_row(row, "object")?;
    let parts: Vec<_> = row
        .get_str("_id")
        .map_err(|_| DocumentError::Corrupt)?
        .split(':')
        .collect();
    let ["object", owner, document, sha] = parts.as_slice() else {
        return Err(DocumentError::Corrupt);
    };
    let source = DocumentSourceReservation {
        owner_hash: (*owner).into(),
        document_id: (*document).into(),
        sha256: (*sha).into(),
        bytes: number(row, "bytes")?,
    };
    source.validate().map_err(|_| DocumentError::Corrupt)?;
    let reservation_id = match row.get("reservation_id") {
        Some(Bson::String(value)) if super::super::quota_baseline::valid_activation(value) => {
            Some(value.clone())
        }
        None => None,
        _ => return Err(DocumentError::Corrupt),
    };
    Ok(ObjectCandidate {
        source,
        reservation_id,
        reserved_before: before,
        condemned,
    })
}
