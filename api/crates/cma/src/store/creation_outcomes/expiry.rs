use super::*;
use futures_util::TryStreamExt;

const BATCH: i64 = 32;

impl CreationOutcomes {
    /// Older covered writers could commit Failed without settling their proof.
    /// Only the original linked pending proof can authorize this recovery.
    pub(super) async fn settle_failed_linked(
        &self,
        after: &mut Option<String>,
    ) -> Result<(), StoreError> {
        if !self.config.terminal_settlement {
            return Ok(());
        }
        let mut id_filter = doc! {"$regex":"^[0-9a-f]{32}$"};
        if let Some(id) = after.as_ref() {
            id_filter.insert("$gt", id);
        }
        let mut candidates = majority_collection::<Document>(
            &self.database,
            super::super::agent_lifecycle_operations::COLLECTION,
        )
        .find(
            doc! {"status":"failed", "creation_fence.epoch":&self.config.epoch,
            "_id":id_filter, "document_schema_version":VERSION,
            "sandbox_id":null,"agent_id":null,"lease_token":null,"lease_expires_at":null},
        )
        .projection(doc! {"_id":1,"owner":1,"creation_fence":1})
        .sort(doc! {"_id":1})
        .hint(mongodb::options::Hint::Name(
            "creation_terminal_recovery".into(),
        ))
        .limit(BATCH)
        .max_time(IO_BUDGET)
        .await
        .map_err(driver)?;
        let mut count = 0;
        while let Some(row) = candidates.try_next().await.map_err(driver)? {
            count += 1;
            let Ok(operation) = row.get_str("_id") else {
                continue;
            };
            *after = Some(operation.to_owned());
            let (Ok(owner), Ok(fence)) = (row.get_str("owner"), row.get_document("creation_fence"))
            else {
                continue;
            };
            let Ok(fence) =
                mongodb::bson::deserialize_from_document::<CreationFence>(fence.clone())
            else {
                continue;
            };
            if owner.is_empty()
                || !contract::lower_hex(&fence.record_id, 64)
                || !contract::lower_hex(&fence.generation, 32)
                || fence.epoch != self.config.epoch
            {
                continue;
            }
            publication::recover_failed_operation(
                &self.database,
                &SubjectId::new(owner),
                &fence,
                operation,
            )
            .await?;
        }
        if count < BATCH {
            *after = None;
        }
        Ok(())
    }

    /// A covered direct submission cannot link an operation or publish a Sandbox
    /// after its database-clock fence expires. Commit that terminal proof using
    /// the same row that both publication paths must update.
    pub(super) async fn settle_expired_unlinked(&self) -> Result<(), StoreError> {
        if !self.config.terminal_settlement {
            return Ok(());
        }
        let filter = doc! {
            "_id":{"$regex":"^[0-9a-f]{64}$"},
            "document_schema_version": VERSION,
            "deployment": &self.config.deployment,
            "epoch": &self.config.epoch,
            "state.kind": "pending",
            "operation_id": {"$exists": false},
            "owner": {"$type":"string", "$ne":""},
            "identity": {"$regex":"^[0-9a-f]{64}$"},
            "generation": {"$regex":"^[0-9a-f]{32}$"},
            "operation": {"$in":["sandbox","agent_bootstrap"]},
            "$expr": {"$and": [
                {"$eq": [{"$type": "$lease_until"}, "date"]},
                {"$eq": [{"$type": "$owner"}, "string"]},
                {"$eq": [{"$type": "$identity"}, "string"]},
                {"$eq": [{"$type": "$generation"}, "string"]},
                {"$lte": ["$lease_until", "$$NOW"]},
            ]},
        };
        let mut candidates = self
            .records
            .find(filter.clone())
            .projection(doc! {"_id":1,"owner":1,"identity":1,"generation":1})
            .sort(doc! {"lease_until":1,"_id":1})
            .limit(BATCH)
            .max_time(IO_BUDGET)
            .await
            .map_err(driver)?;
        while let Some(row) = candidates.try_next().await.map_err(driver)? {
            let (Ok(id), Ok(owner), Ok(identity), Ok(generation)) = (
                row.get_str("_id"),
                row.get_str("owner"),
                row.get_str("identity"),
                row.get_str("generation"),
            ) else {
                continue;
            };
            if owner.is_empty()
                || !contract::lower_hex(identity, 64)
                || !contract::lower_hex(generation, 32)
            {
                continue;
            }
            let mut exact = filter.clone();
            exact.insert("_id", id);
            exact.insert("owner", owner);
            exact.insert("identity", identity);
            exact.insert("generation", generation);
            self.records.update_one(exact, doc! {"$set":{"state":{"kind":"refused","code":RefusalCode::CreationExpired.code()}}})
                .await.map_err(driver)?;
        }
        Ok(())
    }
}
