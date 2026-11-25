use super::*;
use mongodb::{bson::DateTime, options::ReturnDocument};

pub(super) async fn server_now(store: &CaptureStore) -> Result<u64, SandboxStoreError> {
    let reply = store
        .database
        .run_command(doc! {"hello":1})
        .await
        .map_err(unavailable)?;
    u64::try_from(
        reply
            .get_datetime("localTime")
            .map_err(unavailable)?
            .timestamp_millis()
            / 1000,
    )
    .map_err(unavailable)
}

impl MeasuredLease {
    pub(super) fn filter(&self) -> Document {
        doc! {"_id":keys::id(&self.stream.key()), "lease_holder":&self.holder, "lease_generation":self.generation,
        "$expr":{"$gt":["$lease_until","$$NOW"]}}
    }
}

impl CaptureStore {
    pub(crate) async fn claim_measured(
        &self,
        key: &str,
    ) -> Result<Option<MeasuredLease>, SandboxStoreError> {
        if !key.starts_with(&keys::prefix("stream", &self.config.namespace)) {
            return Err(corrupt());
        }
        let holder = uuid::Uuid::new_v4().to_string();
        let row = self.collection(INVENTORY).find_one_and_update(
            doc! {"_id":keys::id(key),"lease_generation":{"$gte":0_i64,"$lt":i64::MAX},
                "$expr":{"$lte":["$lease_until","$$NOW"]}},
            vec![doc! {"$set":{"lease_holder":{"$literal":&holder},"lease_generation":{"$add":["$lease_generation",1_i64]},
                "lease_started_at":"$$NOW","lease_until":{"$add":["$$NOW",120_000_i64]}}}])
            .return_document(ReturnDocument::After).await.map_err(unavailable)?;
        row.map(|row| {
            Ok(MeasuredLease {
                stream: MeasuredStream::read(&row)?,
                holder,
                generation: row.get_i64("lease_generation").map_err(unavailable)?,
                now: u64::try_from(
                    row.get_datetime("lease_started_at")
                        .map_err(unavailable)?
                        .timestamp_millis()
                        / 1000,
                )
                .map_err(unavailable)?,
            })
        })
        .transpose()
    }

    pub(crate) async fn release_measured(
        &self,
        lease: &MeasuredLease,
    ) -> Result<(), SandboxStoreError> {
        self.collection(INVENTORY)
            .update_one(
                lease.filter(),
                doc! {"$set":{"lease_until":DateTime::from_millis(0)}},
            )
            .await
            .map_err(unavailable)?;
        Ok(())
    }

    pub(super) async fn fence_measured(
        &self,
        session: &mut ClientSession,
        lease: &MeasuredLease,
    ) -> Result<(), TransactionEventError> {
        use TransactionEventError::{Domain, Driver};
        let mut filter = lease.filter();
        filter.insert("record", encode(&lease.stream).map_err(Domain)?);
        let changed = self
            .collection(INVENTORY)
            .update_one(filter, doc! {"$inc":{"publication_fence":1_i64}})
            .session(session)
            .await
            .map_err(Driver)?;
        if changed.matched_count != 1 {
            return Err(Domain(SandboxStoreError::Conflict));
        }
        Ok(())
    }
}
