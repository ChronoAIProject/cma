use super::*;
use crate::document_artifacts::mongo;
use layer2_core::document_artifacts::digest;

impl ReclamationStore {
    pub(super) async fn binding(
        &self,
        session: &mut ClientSession,
    ) -> Result<Binding, quota::QuotaError> {
        let state = self
            .ledger
            .find_one(doc! {"_id":quota::STATE})
            .session(session)
            .await?
            .ok_or(DocumentError::Unavailable)?;
        if !quota::generation_enabled(&state)? {
            return Err(DocumentError::Unavailable.into());
        }
        Ok(Binding::read(&state, &self.namespace)?)
    }

    pub(super) async fn document(
        &self,
        source: &DocumentSourceReservation,
        session: &mut ClientSession,
    ) -> Result<Option<layer2_core::document_artifacts::StoredDocument>, quota::QuotaError> {
        let Some(row) = self
            .documents
            .rows
            .find_one(doc! { "_id": &source.document_id })
            .session(&mut *session)
            .await?
        else {
            return Ok(None);
        };
        if let Some((tombstone, _)) = crate::document_artifacts::deletion::decode_audit(&row)? {
            if tombstone.owner_hash != source.owner_hash
                || !reclamation_slots::terminal(&self.ledger, source, session).await?
            {
                return Err(DocumentError::Corrupt.into());
            }
            return Ok(None);
        }
        let record = mongo::decode(&row)?;
        if digest(record.metadata.owner_subject.as_bytes()) != source.owner_hash {
            return Err(DocumentError::Corrupt.into());
        }
        Ok(Some(record))
    }
    pub(super) async fn settle_transaction(
        &self,
        claim: &DeleteClaim,
        session: &mut ClientSession,
    ) -> Result<(), quota::QuotaError> {
        if self.binding(session).await? != claim.binding {
            return Err(DocumentError::Conflict.into());
        }
        let source = &claim.source;
        let row = self
            .ledger
            .find_one(doc! { "_id": object_authority::key(source) })
            .session(&mut *session)
            .await?
            .ok_or(DocumentError::Corrupt)?;
        let mut authority = Authority::decode(&row)?;
        if authority.source != *source {
            return Err(DocumentError::Corrupt.into());
        }
        let expected = authority.condition();
        authority.settle(claim.generation, &claim.holder)?;
        self.replace(expected, &authority, session).await?;

        // Owner admissions increment this same row before testing slot usage.
        // It serializes the last-object decision with admission for another digest.
        let owner = quota::owner_id(&source.owner_hash);
        if self
            .ledger
            .update_one(
                doc! { "_id": &owner, "kind": "owner" },
                doc! { "$inc": { "fence": 1_i64 } },
            )
            .session(&mut *session)
            .await?
            .matched_count
            != 1
        {
            return Err(DocumentError::Corrupt.into());
        }
        let charge = quota::object_row(source)?;
        if self
            .ledger
            .delete_one(charge)
            .session(&mut *session)
            .await?
            .deleted_count
            != 1
        {
            return Err(DocumentError::Corrupt.into());
        }
        let release_document = self.document(source, session).await?.is_none()
            && self
                .ledger
                .find_one(quota::object_range(source))
                .projection(doc! { "_id": 1 })
                .session(&mut *session)
                .await?
                .is_none()
            && reclamation_slots::can_release(&self.ledger, source, session).await?;
        if release_document
            && self
                .ledger
                .delete_one(quota::document_row(source))
                .session(&mut *session)
                .await?
                .deleted_count
                != 1
        {
            return Err(DocumentError::Corrupt.into());
        }
        let docs = i64::from(release_document);
        let bytes = source.bytes as i64;
        let updated = self
            .ledger
            .update_one(
                doc! { "_id": owner, "documents": { "$gte": docs }, "bytes": { "$gte": bytes },
                "objects": { "$gte": 1_i64 } },
                doc! { "$inc": { "documents": -docs, "bytes": -bytes, "objects": -1_i64 } },
            )
            .session(session)
            .await?;
        if updated.matched_count != 1 {
            return Err(DocumentError::Corrupt.into());
        }
        Ok(())
    }
}
