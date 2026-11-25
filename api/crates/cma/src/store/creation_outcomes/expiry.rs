use super::*;
use futures_util::TryStreamExt;

const BATCH: i64 = 32;

impl CreationOutcomes {
    /// Older covered writers could commit Failed without settling their proof.
    /// Only the original linked pending proof can authorize this recovery.
    pub(super) async fn settle_failed_linked(&self) -> Result<(), StoreError> {
        if !self.config.terminal_settlement {
            return Ok(());
        }
        let pipeline = vec![
            doc! {"$match": {
                "document_schema_version":VERSION,"deployment":&self.config.deployment,
                "epoch":&self.config.epoch,"state.kind":"pending","operation":"agent_bootstrap",
                "operation_id":{"$regex":"^[0-9a-f]{32}$"},"owner":{"$type":"string","$ne":""},
                "generation":{"$regex":"^[0-9a-f]{32}$"},"identity":{"$regex":"^[0-9a-f]{64}$"},
            }},
            doc! {"$lookup":{
                "from":super::super::agent_lifecycle_operations::COLLECTION,
                "localField":"operation_id","foreignField":"_id","as":"operation_row",
                "let":{"proof_id":"$_id","proof_owner":"$owner","proof_epoch":"$epoch","proof_generation":"$generation"},
                "pipeline":[{"$match":{
                    "document_schema_version":VERSION,"status":"failed","sandbox_id":null,"agent_id":null,
                    "lease_token":null,"lease_expires_at":null,
                    "$expr":{"$and":[
                        {"$eq":["$owner","$$proof_owner"]},
                        {"$eq":["$creation_fence.record_id","$$proof_id"]},
                        {"$eq":["$creation_fence.epoch","$$proof_epoch"]},
                        {"$eq":["$creation_fence.generation","$$proof_generation"]},
                    ]}
                }}]
            }},
            doc! {"$match":{"operation_row.0":{"$exists":true}}},
            doc! {"$limit":BATCH},
            doc! {"$project":{"_id":1,"owner":1,"operation_id":1,"generation":1}},
        ];
        let mut candidates = self
            .records
            .aggregate(pipeline)
            .max_time(IO_BUDGET)
            .await
            .map_err(driver)?;
        while let Some(row) = candidates.try_next().await.map_err(driver)? {
            let (Ok(id), Ok(owner), Ok(operation), Ok(generation)) = (
                row.get_str("_id"),
                row.get_str("owner"),
                row.get_str("operation_id"),
                row.get_str("generation"),
            ) else {
                continue;
            };
            let fence = CreationFence {
                record_id: id.to_owned(),
                generation: generation.to_owned(),
                epoch: self.config.epoch.clone(),
                lifecycle_lease: None,
            };
            publication::recover_failed_operation(
                &self.database,
                &SubjectId::new(owner),
                &fence,
                operation,
            )
            .await?;
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
