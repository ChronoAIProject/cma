use futures_util::TryStreamExt as _;
use mongodb::{
    bson::doc,
    options::{CollectionOptions, ReadConcern, WriteConcern},
    Client, Collection,
};

use crate::{
    model::{Change, CreateSandbox, ModelError, Sandbox},
    schema::{self, SchemaError},
};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error(transparent)]
    Invalid(#[from] ModelError),
    #[error(transparent)]
    Schema(#[from] SchemaError),
    #[error("sandbox not found")]
    NotFound,
    #[error("idempotency key was already used with different parameters")]
    IdempotencyConflict,
    #[error("sandbox revision changed")]
    Conflict,
    // Driver errors may include request fields or addresses; don't return them
    // through the public API or include them in routine logs.
    #[error("Sandbox Manager storage is unavailable")]
    Database(#[from] mongodb::error::Error),
}

#[derive(Clone)]
pub struct SandboxStore {
    sandboxes: Collection<Sandbox>,
}

impl SandboxStore {
    pub async fn open(client: &Client, database: &str) -> Result<Self, StoreError> {
        let database = client.database(database);
        schema::verify_runtime(&database).await?;
        let options = CollectionOptions::builder()
            .read_concern(ReadConcern::majority())
            .write_concern(WriteConcern::majority())
            .build();
        Ok(Self {
            sandboxes: database.collection_with_options(schema::SANDBOXES, options),
        })
    }

    pub async fn admit(
        &self,
        owner: &str,
        key: &str,
        spec: CreateSandbox,
        now: i64,
    ) -> Result<Sandbox, StoreError> {
        let sandbox = Sandbox::admit(owner, key, spec, now)?;
        match self.sandboxes.insert_one(&sandbox).await {
            Ok(_) => Ok(sandbox),
            Err(error) => {
                // Reconcile both duplicate-key and unknown commit results. A
                // retry never changes the immutable request or another owner.
                let existing = self
                    .sandboxes
                    .find_one(doc! {
                        "owner": owner, "request_key_hash": &sandbox.request_key_hash
                    })
                    .await?;
                match existing {
                    Some(existing) if existing.request_sha256 == sandbox.request_sha256 => {
                        Ok(existing)
                    }
                    Some(_) => Err(StoreError::IdempotencyConflict),
                    None => Err(StoreError::Database(error)),
                }
            }
        }
    }

    pub async fn get(&self, owner: &str, id: &str) -> Result<Sandbox, StoreError> {
        self.sandboxes
            .find_one(doc! { "_id": id, "owner": owner })
            .await?
            .ok_or(StoreError::NotFound)
    }

    /// Stable ID pagination, always confined to the authenticated owner.
    pub async fn list(
        &self,
        owner: &str,
        after: Option<&str>,
        limit: u32,
    ) -> Result<Vec<Sandbox>, StoreError> {
        if !(1..=100).contains(&limit) {
            return Err(ModelError::Invalid("limit must be between 1 and 100").into());
        }
        let mut filter = doc! { "owner": owner };
        if let Some(after) = after {
            filter.insert("_id", doc! { "$gt": after });
        }
        Ok(self
            .sandboxes
            .find(filter)
            .sort(doc! { "_id": 1 })
            .limit(i64::from(limit))
            .await?
            .try_collect()
            .await?)
    }

    pub async fn transition(
        &self,
        current: &Sandbox,
        change: Change,
        now: i64,
    ) -> Result<Sandbox, StoreError> {
        // Only the stored aggregate supplies immutable configuration and runtime
        // authority. A caller's copy provides identity/revision, not replacement
        // fields that could rewrite the environment while admitting an action.
        let stored = self.get(&current.owner, &current.id).await?;
        if stored.revision != current.revision {
            return Err(StoreError::Conflict);
        }
        let next = stored.apply(change, now)?;
        let result = self
            .sandboxes
            .replace_one(
                doc! {
                    "_id": &current.id, "owner": &current.owner,
                    "revision": current.revision, "schema_version": current.schema_version,
                },
                &next,
            )
            .await?;
        if result.matched_count != 1 {
            return Err(StoreError::Conflict);
        }
        Ok(next)
    }

    /// Workers see only this service's authority. Unknown dispatches remain
    /// visible after restart and cannot be mistaken for failed allocations.
    pub async fn pending(&self, limit: u32) -> Result<Vec<Sandbox>, StoreError> {
        if !(1..=100).contains(&limit) {
            return Err(ModelError::Invalid("invalid worker batch limit").into());
        }
        Ok(self
            .sandboxes
            .find(
                doc! { "phase": { "$in": ["accepted", "dispatching", "starting", "terminating"] } },
            )
            .sort(doc! { "updated_at_ms": 1, "_id": 1 })
            .limit(i64::from(limit))
            .await?
            .try_collect()
            .await?)
    }
}
