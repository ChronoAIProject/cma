//! The release admission is guarded by the original exclusion and database time.
use super::*;

impl ArchiveStore {
    pub(crate) async fn complete(
        &self,
        request: &Request,
        bytes: &[u8],
        receipt: RetirementArchiveReceipt,
    ) -> Result<()> {
        receipt.validate().map_err(|_| REFUSED)?;
        let mut session = self.client.start_session().await.map_err(|_| REFUSED)?;
        start_transaction(&mut session).await.map_err(|_| REFUSED)?;
        let result = self
            .complete_in(&mut session, request, bytes, &receipt)
            .await
            .and_then(|fresh| {
                // A delayed final write acknowledgement can outlive its proof.
                // Abort before sending commit if admission is no longer current.
                if fresh {
                    crate::retirement_archive::export::exclusion(request)?;
                }
                Ok(())
            });
        if result.is_err() {
            let _ = session.abort_guarded().await;
            return result;
        }
        self.commit_or_reconcile(session, request, bytes, Some(&receipt))
            .await
    }
    async fn complete_in(
        &self,
        session: &mut ClientSession,
        request: &Request,
        bytes: &[u8],
        receipt: &RetirementArchiveReceipt,
    ) -> Result<bool> {
        let current = self
            .parents()
            .find_one(doc! {"_id":&request.sandbox_id})
            .session(&mut *session)
            .await
            .map_err(|_| REFUSED)?
            .ok_or(REFUSED)?;
        let mut parent = accepted(&current, request, bytes)?;
        let gate = parent
            .lifecycle
            .retirement_archive
            .as_mut()
            .ok_or(REFUSED)?;
        if let Some(existing) = &gate.verified {
            return if existing == receipt {
                Ok(false)
            } else {
                Err(REFUSED)
            };
        }
        let manifest: ObjectReceipt =
            mongodb::bson::deserialize_from_bson(current.get(MANIFEST).ok_or(REFUSED)?.clone())
                .map_err(|_| REFUSED)?;
        let restore: ObjectReceipt =
            mongodb::bson::deserialize_from_bson(current.get(RESTORE).ok_or(REFUSED)?.clone())
                .map_err(|_| REFUSED)?;
        if manifest.sha256 != receipt.manifest_sha256
            || restore.sha256 != receipt.verification_sha256
        {
            return Err(REFUSED);
        }
        gate.verified = Some(receipt.clone());
        // Validate the exact original private authority after all census I/O.
        // Committed identical receipts above reconcile without reopening admission.
        let authority = crate::retirement_archive::export::exclusion(request)?;
        let authority: crate::retirement_archive::ExclusionReceipt =
            serde_json::from_slice(&authority).map_err(|_| REFUSED)?;
        self.advance(session, &current, parent, Document::new())
            .await?;
        // Last transactional write: server time, not the CLI clock, admits the
        // release. A delayed command or expired lease aborts the whole transition,
        // including its usage event. No source/custody I/O follows this write.
        require_exclusion_at_release(self, session, request, &authority).await?;
        Ok(true)
    }
}

async fn require_exclusion_at_release(
    store: &ArchiveStore,
    session: &mut ClientSession,
    request: &Request,
    authority: &crate::retirement_archive::ExclusionReceipt,
) -> Result<()> {
    // Re-read immediately before the guard too: the original proof must remain
    // readable and byte-identical even if the usage write took a long time.
    crate::retirement_archive::export::exclusion(request)?;
    let changed = store
        .parents()
        .update_one(
            release_filter(request, authority)?,
            doc! {"$inc": {"runtime_writer_claim_fence": 1_i64}},
        )
        .session(session)
        .await
        .map_err(|_| REFUSED)?;
    if changed.matched_count != 1 {
        return Err(REFUSED);
    }
    Ok(())
}

fn release_filter(
    request: &Request,
    authority: &crate::retirement_archive::ExclusionReceipt,
) -> Result<Document> {
    let timestamp = |value: &str| -> Result<mongodb::bson::DateTime> {
        let seconds = layer2_core::seconds_from_rfc3339(value).ok_or(REFUSED)?;
        let millis = seconds.checked_mul(1000).ok_or(REFUSED)?;
        Ok(mongodb::bson::DateTime::from_millis(
            i64::try_from(millis).map_err(|_| REFUSED)?,
        ))
    };
    Ok(doc! {
        "_id": &request.sandbox_id,
        "record.lifecycle.retirement_archive.operation_id": &request.operation_id,
        "record.lifecycle.retirement_archive.exclusion_sha256": &request.exclusion_sha256,
        "$expr": {"$and": [
            {"$lte": [timestamp(&authority.qualified_at)?, "$$NOW"]},
            {"$lt": ["$$NOW", timestamp(&authority.expires_at)?]}
        ]}
    })
}

#[cfg(test)]
#[path = "completion_guard_tests.rs"]
mod tests;
