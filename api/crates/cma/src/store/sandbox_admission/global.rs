//! All owners contend on one transaction fence before consuming global capacity.
use super::*;

impl MongoSandboxStore {
    /// Zero preserves the legacy admission path. Enable only after all writers
    /// have this implementation and capacity outside this store is reserved.
    pub(crate) fn with_global_sandbox_limit(mut self, limit: u16) -> Self {
        self.global_sandbox_limit = limit;
        self
    }
}

pub(super) async fn admit(
    store: &MongoSandboxStore,
    session: &mut mongodb::ClientSession,
) -> Result<(), AdmissionError> {
    let limit = store.global_sandbox_limit;
    if limit == 0 {
        return Ok(());
    }
    // A structured _id cannot collide with any subject's string _id. The write
    // forces different-owner transactions to retry with a fresh snapshot.
    store
        .sandbox_admissions
        .update_one(
            doc! { "_id": { "scope": "global_sandbox_capacity" } },
            doc! {
                "$inc": { "fence": 1_i64 },
                "$setOnInsert": { "document_schema_version": DOCUMENT_SCHEMA_VERSION },
            },
        )
        .upsert(true)
        .session(&mut *session)
        .await
        .map_err(|error| {
            if is_duplicate_key(&error) {
                AdmissionError::Retry(error)
            } else {
                AdmissionError::Driver(error)
            }
        })?;
    // Paused, provisioning and retiring rows keep their reservation. A resume
    // therefore cannot grow admitted capacity; only completed retirement frees it.
    // Existing rows are counted directly, so activation needs no counter backfill.
    let live = store
        .sandboxes
        .count_documents(doc! { "retired_at": Bson::Null })
        .hint(mongodb::options::Hint::Name("sandboxes_global_live".into()))
        .limit(u64::from(limit))
        .max_time(Duration::from_secs(3))
        .session(&mut *session)
        .await
        .map_err(AdmissionError::Driver)?;
    if live >= u64::from(limit) {
        tracing::info!(
            outcome = "global_capacity_refused",
            "sandbox admission refused"
        );
        return Err(AdmissionError::Domain(
            SandboxStoreError::GlobalCapacityReached,
        ));
    }
    Ok(())
}
