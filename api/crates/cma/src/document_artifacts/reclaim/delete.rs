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
        crate::document_artifacts::transaction::run(
            self.rows.client(),
            Claim {
                store: self,
                candidate,
                holder,
                now,
            },
        )
        .await
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

struct Claim<'a> {
    store: &'a MongoDocumentReclaimStore,
    candidate: &'a ObjectCandidate,
    holder: &'a str,
    now: i64,
}
#[async_trait::async_trait]
impl crate::document_artifacts::transaction::Operation for Claim<'_> {
    type Output = Option<DeleteClaim>;
    async fn apply(
        &self,
        session: &mut ClientSession,
    ) -> Result<Self::Output, crate::document_artifacts::transaction::Error> {
        use crate::document_artifacts::transaction::Error;
        if self
            .store
            .fence_lifecycle(self.candidate, session)
            .await
            .map_err(|e| match e {
                TransactionError::Domain(e) => Error::Domain(e),
                TransactionError::Driver(e) => Error::Driver(e),
            })?
        {
            return Ok(None);
        }
        let until = self
            .now
            .checked_add(DELETE_LEASE.as_millis() as i64)
            .ok_or(DocumentError::Invalid)?;
        let mut filter = doc! {"_id":id(self.candidate)?,"kind":"object","state":"condemned","bytes":self.candidate.source.bytes as i64,"$or":[{"delete_lease_until":{"$exists":false}},{"delete_lease_until":{"$lt":DateTime::from_millis(self.now)}}]};
        filter.extend(incarnation_filter(self.candidate));
        let row=self.store.rows.find_one_and_update(filter,doc! {"$set":{"delete_holder":self.holder,"delete_lease_until":DateTime::from_millis(until)},"$inc":{"delete_fence":1_i64}}).return_document(ReturnDocument::After).session(session).await?;
        let Some(row) = row else { return Ok(None) };
        validate_row(&row, "object")?;
        Ok(Some(DeleteClaim {
            candidate: self.candidate.clone(),
            holder: self.holder.into(),
            fence: number(&row, "delete_fence")?,
            lease_until: row
                .get_datetime("delete_lease_until")
                .map_err(|_| DocumentError::Corrupt)?
                .timestamp_millis(),
        }))
    }
    async fn reconcile(&self, snapshot: &mut ClientSession) -> Result<Self::Output, DocumentError> {
        // A lost claim acknowledgement authorizes no remote deletion. Retain its
        // lease for the next holder rather than attributing an uncertain claim.
        self.store
            .rows
            .find_one(doc! {"_id":id(self.candidate)?})
            .read_concern(ReadConcern::snapshot())
            .max_time(IO)
            .session(snapshot)
            .await
            .map_err(unavailable)?;
        Ok(None)
    }
}
