use super::*;

impl MongoDocumentReclaimStore {
    pub(super) async fn judge_transaction(
        &self,
        candidate: &ObjectCandidate,
        session: &mut ClientSession,
    ) -> Result<Judgement, TransactionError> {
        if self.fence_lifecycle(candidate, session).await? {
            return Ok(Judgement::OwnedByPurge);
        }
        // Both the publishing transaction and this transaction write the same
        // object row. Their write conflict closes the snapshot write-skew gap.
        let document = self
            .documents
            .find_one(doc! {"_id":&candidate.source.document_id})
            .session(&mut *session)
            .await?;
        let referenced = match document.as_ref() {
            Some(row) => super::super::mongo::decode(row)?
                .revisions
                .iter()
                .any(|revision| revision.object_key == candidate.source.object_key()),
            None => false,
        };
        let object_id = id(candidate)?;
        let Some(row) = self
            .rows
            .find_one(doc! {"_id":&object_id})
            .session(&mut *session)
            .await?
        else {
            return Ok(Judgement::Skipped);
        };
        if !object_identity_matches(&row, &object_row(&candidate.source)?) {
            return Err(DocumentError::Corrupt.into());
        }
        if !same_incarnation(&row, candidate) {
            return Ok(Judgement::Skipped);
        }
        if row.contains_key("state") && row.get_str("state").ok() != Some("reserved") {
            return Ok(Judgement::Skipped);
        }
        let activation = self
            .rows
            .find_one(doc! {"_id":STATE})
            .session(&mut *session)
            .await?
            .and_then(|row| row.get_datetime("activated_at").ok().copied());
        let reserved_at = match row.get("reserved_at") {
            Some(mongodb::bson::Bson::DateTime(time)) => Some(*time),
            None => activation,
            _ => return Err(DocumentError::Corrupt.into()),
        };
        if reserved_at.is_none_or(|time| time.timestamp_millis() >= candidate.reserved_before) {
            return Ok(Judgement::Skipped);
        }
        let filter = doc! {"_id":&object_id,"kind":"object","bytes":candidate.source.bytes as i64,
        "$and":[{"$or":[{"state":"reserved"},{"state":{"$exists":false}}]},queries::age_filter(candidate.reserved_before,activation),incarnation_filter(candidate)]};
        let state = if referenced { "committed" } else { "condemned" };
        let update = if referenced {
            doc! {"$set":{"state":state}}
        } else {
            doc! {"$set":{"state":state},"$currentDate":{"condemned_at":true}}
        };
        let result = self
            .rows
            .update_one(filter, update)
            .session(&mut *session)
            .await?;
        Ok(if result.matched_count == 0 {
            Judgement::Skipped
        } else if referenced {
            Judgement::Committed
        } else {
            Judgement::Condemned
        })
    }

    pub(super) async fn finalize_transaction(
        &self,
        claim: &DeleteClaim,
        session: &mut ClientSession,
    ) -> Result<bool, TransactionError> {
        let candidate = &claim.candidate;
        if self.fence_lifecycle(candidate, session).await? {
            return Ok(false);
        }
        let owner_id = owner_id(&candidate.source.owner_hash);
        // The same owner fence used by reserve serializes release and admission.
        let fence = self
            .rows
            .update_one(doc! {"_id":&owner_id}, doc! {"$inc":{"fence":1_i64}})
            .session(&mut *session)
            .await?;
        if fence.matched_count != 1 {
            return Err(DocumentError::Corrupt.into());
        }
        let condemned = delete::claim_filter(claim)?;
        let Some(object) = self
            .rows
            .find_one(condemned.clone())
            .session(&mut *session)
            .await?
        else {
            return Ok(false);
        };
        if !object_identity_matches(&object, &object_row(&candidate.source)?) {
            return Err(DocumentError::Corrupt.into());
        }
        let owner = self
            .rows
            .find_one(doc! {"_id":&owner_id})
            .session(&mut *session)
            .await?
            .ok_or(DocumentError::Corrupt)?;
        validate_row(&owner, "owner")?;
        let bytes = number(&owner, "bytes")?;
        let objects = number(&owner, "objects")?;
        let documents = number(&owner, "documents")?;
        if bytes < candidate.source.bytes || objects == 0 {
            return Err(DocumentError::Corrupt.into());
        }
        let deleted = self
            .rows
            .delete_one(condemned)
            .session(&mut *session)
            .await?;
        if deleted.deleted_count != 1 {
            return Ok(false);
        }
        let changed=self.rows.update_one(doc!{"_id":&owner_id,"bytes":{"$gte":candidate.source.bytes as i64},"objects":{"$gte":1_i64}},
            doc!{"$inc":{"bytes":-(candidate.source.bytes as i64),"objects":-1_i64}}).session(&mut *session).await?;
        if changed.matched_count != 1 {
            return Err(DocumentError::Corrupt.into());
        }
        let document_exists = self
            .documents
            .find_one(doc! {"_id":&candidate.source.document_id})
            .projection(doc! {"_id":1})
            .session(&mut *session)
            .await?
            .is_some();
        let prefix = format!(
            "object:{}:{}:",
            candidate.source.owner_hash, candidate.source.document_id
        );
        let end = format!(
            "object:{}:{};",
            candidate.source.owner_hash, candidate.source.document_id
        );
        let any_object = self
            .rows
            .find_one(doc! {"_id":{"$gte":prefix,"$lt":end}})
            .projection(doc! {"_id":1})
            .session(&mut *session)
            .await?
            .is_some();
        let lifecycle_exists = self
            .lifecycle
            .find_one(doc! {"_id":&candidate.source.document_id})
            .projection(doc! {"_id":1})
            .session(&mut *session)
            .await?
            .is_some();
        if !document_exists && !lifecycle_exists && !any_object {
            let document_id = format!(
                "document:{}:{}",
                candidate.source.owner_hash, candidate.source.document_id
            );
            if let Some(row) = self
                .rows
                .find_one(doc! {"_id":&document_id})
                .session(&mut *session)
                .await?
            {
                validate_row(&row, "document")?;
                if documents == 0 {
                    return Err(DocumentError::Corrupt.into());
                }
                let deleted = self
                    .rows
                    .delete_one(doc! {"_id":document_id,"kind":"document"})
                    .session(&mut *session)
                    .await?;
                if deleted.deleted_count == 1 {
                    let changed = self
                        .rows
                        .update_one(
                            doc! {"_id":owner_id,"documents":{"$gte":1_i64}},
                            doc! {"$inc":{"documents":-1_i64}},
                        )
                        .session(&mut *session)
                        .await?;
                    if changed.matched_count != 1 {
                        return Err(DocumentError::Corrupt.into());
                    }
                }
            }
        }
        Ok(true)
    }
}
