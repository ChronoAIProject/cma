use super::*;
use crate::document_artifacts::{
    quota::{owner_id, validate_row},
    transaction::{self, Error, Operation},
};
use mongodb::ClientSession;

pub(super) async fn release(
    store: &MongoDocumentPurgeStore,
    lease: &PurgeLease,
) -> Result<bool, DocumentError> {
    claim_filter(lease)?;
    transaction::run(store.rows.client(), Release { store, lease }).await
}
struct Release<'a> {
    store: &'a MongoDocumentPurgeStore,
    lease: &'a PurgeLease,
}
#[async_trait::async_trait]
impl Operation for Release<'_> {
    type Output = bool;
    async fn apply(&self, session: &mut ClientSession) -> Result<bool, Error> {
        let document = &self.lease.document;
        // The lifecycle write locks this identity before touching quota rows. A
        // competing claim or completion cannot pass this lease fence twice.
        let changed=self.store.rows.update_one(claim_filter(self.lease)?,vec![doc! {"$set":{"state":"purged","purge.completed_at":"$$NOW","purge.lease.expires_at":"$$NOW"}},doc! {"$unset":["purge.blocked","purge.position"]}]).session(&mut *session).await?;
        if changed.matched_count == 0 {
            return Ok(false);
        }
        let rows = &self.store.quotas;
        let owner_id = owner_id(&document.owner_hash);
        let fence = rows
            .update_one(doc! {"_id":&owner_id}, doc! {"$inc":{"fence":1_i64}})
            .session(&mut *session)
            .await?;
        // Match the ledger decoder's exact integer types. BSON comparisons alone
        // also accept strings/arrays, which $sum would silently ignore.
        let valid = doc! {"$and":[
            {"$eq":["$kind","object"]},
            {"$eq":[{"$type":"$document_schema_version"},"long"]},
            {"$eq":["$document_schema_version",1_i64]},
            {"$eq":[{"$type":"$bytes"},"long"]},
            {"$gte":["$bytes",1_i64]},
            {"$lte":["$bytes",MAX_DOCUMENT_BYTES as i64]}
        ]};
        let mut cursor = rows
            .aggregate([
                doc! {"$match":object_range(&document.owner_hash,&document.id)},
                doc! {"$group":{"_id":null,"objects":{"$sum":1_i64},"bytes":{"$sum":"$bytes"},
                "invalid":{"$sum":{"$cond":[valid,0_i64,1_i64]}}}},
            ])
            .session(&mut *session)
            .await?;
        let totals: Vec<Document> = cursor.stream(&mut *session).try_collect().await?;
        let (objects, bytes) = match totals.first() {
            Some(total) if number(total, "invalid")? == 0 => {
                (number(total, "objects")?, number(total, "bytes")?)
            }
            Some(_) => return Err(DocumentError::Corrupt.into()),
            None => (0, 0),
        };
        let document_id = format!("document:{}:{}", document.owner_hash, document.id);
        let document_row = rows
            .find_one(doc! {"_id":&document_id})
            .session(&mut *session)
            .await?;
        if let Some(row) = &document_row {
            validate_row(row, "document")?;
        }
        if fence.matched_count == 0 && (objects > 0 || document_row.is_some()) {
            return Err(DocumentError::Corrupt.into());
        }
        let deleted = rows
            .delete_many(object_range(&document.owner_hash, &document.id))
            .session(&mut *session)
            .await?;
        if deleted.deleted_count != objects {
            return Err(DocumentError::Corrupt.into());
        }
        let documents = rows
            .delete_one(doc! {"_id":document_id,"kind":"document"})
            .session(&mut *session)
            .await?
            .deleted_count;
        if fence.matched_count == 1 {
            let owner = rows
                .find_one(doc! {"_id":&owner_id})
                .session(&mut *session)
                .await?
                .ok_or(DocumentError::Corrupt)?;
            validate_row(&owner, "owner")?;
            for (field, released) in [
                ("documents", documents),
                ("objects", objects),
                ("bytes", bytes),
            ] {
                if number(&owner, field)? < released {
                    return Err(DocumentError::Corrupt.into());
                }
            }
            let documents = i64::try_from(documents).map_err(|_| DocumentError::Corrupt)?;
            let objects = i64::try_from(objects).map_err(|_| DocumentError::Corrupt)?;
            let bytes = i64::try_from(bytes).map_err(|_| DocumentError::Corrupt)?;
            let result=rows.update_one(doc! {"_id":owner_id,"documents":{"$gte":documents},"objects":{"$gte":objects},"bytes":{"$gte":bytes}},doc! {"$inc":{"documents":-documents,"objects":-objects,"bytes":-bytes}}).session(&mut *session).await?;
            if result.matched_count != 1 {
                return Err(DocumentError::Corrupt.into());
            }
        }
        Ok(true)
    }
    async fn reconcile(&self, snapshot: &mut ClientSession) -> Result<bool, DocumentError> {
        let row = self
            .store
            .rows
            .find_one(doc! {"_id":&self.lease.document.id})
            .read_concern(ReadConcern::snapshot())
            .max_time(IO)
            .session(&mut *snapshot)
            .await
            .map_err(unavailable)?;
        let Some(row) = row else {
            return Err(DocumentError::Unavailable);
        };
        let owned = row
            .get_document("purge")
            .and_then(|p| p.get_document("lease"))
            .ok()
            .is_some_and(|lease| {
                lease.get_str("holder").ok() == Some(self.lease.holder.as_str())
                    && number(lease, "fence") == Ok(self.lease.fence)
            });
        if row.get_str("state").ok() != Some("purged") || !owned {
            return Err(DocumentError::Unavailable);
        }
        let objects = self
            .store
            .quotas
            .find_one(object_range(
                &self.lease.document.owner_hash,
                &self.lease.document.id,
            ))
            .read_concern(ReadConcern::snapshot())
            .max_time(IO)
            .session(snapshot)
            .await
            .map_err(unavailable)?;
        if objects.is_some() {
            return Err(DocumentError::Unavailable);
        }
        Ok(true)
    }
}
