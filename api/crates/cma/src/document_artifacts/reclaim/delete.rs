use super::*;
use mongodb::{bson::DateTime, options::ReturnDocument};

impl MongoDocumentReclaimStore {
    pub(super) async fn claim_deletion(
        &self,
        candidate: &ObjectCandidate,
        holder: &str,
        now: i64,
    ) -> Result<Option<DeleteClaim>, DocumentError> {
        validate_candidate(candidate)?;
        if !super::super::quota_baseline::valid_activation(holder) {
            return Err(DocumentError::Invalid);
        }
        let until = now
            .checked_add(DELETE_LEASE.as_millis() as i64)
            .ok_or(DocumentError::Invalid)?;
        let mut filter = doc! {
            "_id": id(candidate)?, "kind": "object", "state": "condemned",
            "bytes": candidate.source.bytes as i64,
            "$or": [{"delete_lease_until": {"$exists": false}},
                {"delete_lease_until": {"$lt": DateTime::from_millis(now)}}],
        };
        filter.extend(incarnation_filter(candidate));
        // A single atomic conditional write also serializes independent clients.
        // An uncertain response leaves the lease in place and authorizes no HTTP.
        let row = tokio::time::timeout(IO, self.rows.find_one_and_update(filter,
            doc! {"$set": {"delete_holder": holder, "delete_lease_until": DateTime::from_millis(until)},
                "$inc": {"delete_fence": 1_i64}})
            .return_document(ReturnDocument::After).max_time(IO))
            .await.map_err(|_| DocumentError::Unavailable)?.map_err(unavailable)?;
        let Some(row) = row else { return Ok(None) };
        validate_row(&row, "object")?;
        let fence = number(&row, "delete_fence")?;
        if fence == 0 || row.get_str("delete_holder").ok() != Some(holder) {
            return Err(DocumentError::Corrupt);
        }
        Ok(Some(DeleteClaim {
            candidate: candidate.clone(),
            holder: holder.into(),
            fence,
            lease_until: row
                .get_datetime("delete_lease_until")
                .map_err(|_| DocumentError::Corrupt)?
                .timestamp_millis(),
        }))
    }
}

pub(super) fn claim_filter(claim: &DeleteClaim) -> Result<Document, DocumentError> {
    validate_candidate(&claim.candidate)?;
    if !super::super::quota_baseline::valid_activation(&claim.holder)
        || claim.fence == 0
        || claim.fence > i64::MAX as u64
    {
        return Err(DocumentError::Corrupt);
    }
    let mut filter = doc! {"_id": id(&claim.candidate)?, "state": "condemned",
    "delete_holder": &claim.holder, "delete_fence": claim.fence as i64};
    filter.extend(incarnation_filter(&claim.candidate));
    Ok(filter)
}
