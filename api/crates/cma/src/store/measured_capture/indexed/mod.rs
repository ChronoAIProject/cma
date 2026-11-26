//! Generation-scoped projection of the immutable measured custody collection.
use super::{CaptureError, Document, TransactionError};
use crate::store::durability::majority_collection;
use mongodb::{bson::doc, options::ReadConcern, ClientSession, Collection, Database};
use serde::{de::DeserializeOwned, Serialize};

mod backfill;
mod model;
mod reader;
mod sharded_backfill;
mod sharded_capacity;
mod sharded_certify;
mod sharded_generation;
mod sharded_reader;
mod sharded_writer;
mod writer;

use crate::store::migrations::measured_index::{MANIFESTS, PROJECTIONS};

enum ActiveGeneration {
    Legacy(model::Generation),
    Sharded(sharded_generation::ShardedGeneration),
}

#[derive(Clone)]
pub(super) struct IndexedCustody {
    database: Database,
    projections: Collection<Document>,
    manifests: Collection<Document>,
    originals: Collection<Document>,
    writer_floor: String,
}

impl IndexedCustody {
    pub(super) fn new(database: &Database, writer_floor: String) -> Result<Self, CaptureError> {
        if !model::valid_digest(&writer_floor) {
            return Err(CaptureError::Corrupt);
        }
        Ok(Self {
            database: database.clone(),
            projections: majority_collection(database, PROJECTIONS),
            manifests: majority_collection(database, MANIFESTS),
            originals: majority_collection(database, super::COLLECTION),
            writer_floor,
        })
    }

    pub(in crate::store::measured_capture) async fn ready_generation(
        &self,
        session: &mut ClientSession,
    ) -> Result<u64, TransactionError> {
        let (generation, phase) = match self.active_generation(session, false).await? {
            ActiveGeneration::Legacy(root) => (root.generation, root.phase),
            ActiveGeneration::Sharded(root) => (root.generation, root.phase),
        };
        if phase == model::Phase::Ready {
            Ok(generation)
        } else {
            Err(CaptureError::Unavailable.into())
        }
    }

    async fn root(
        &self,
        session: &mut ClientSession,
        snapshot: bool,
    ) -> Result<model::Generation, TransactionError> {
        match self.active_generation(session, snapshot).await? {
            ActiveGeneration::Legacy(root) => Ok(root),
            ActiveGeneration::Sharded(_) => Err(CaptureError::Unavailable.into()),
        }
    }

    async fn active_generation(
        &self,
        session: &mut ClientSession,
        snapshot: bool,
    ) -> Result<ActiveGeneration, TransactionError> {
        let mut action = self.manifests.find_one(doc! {"_id": model::ROOT_ID});
        if snapshot {
            action = action.read_concern(ReadConcern::snapshot());
        }
        let row = action
            .session(session)
            .await?
            .ok_or(CaptureError::Unavailable)?;
        let generation = u64::try_from(
            row.get_i64("generation")
                .map_err(|_| CaptureError::Corrupt)?,
        )
        .map_err(|_| CaptureError::Corrupt)?;
        match row
            .get_i64("schema_version")
            .map_err(|_| CaptureError::Corrupt)?
        {
            1 => {
                let root: model::Generation = model::open(&row, model::ROOT_ID, generation, None)?;
                root.validate()?;
                self.verify_epoch(generation, root.generation, &root.writer_floor_sha256)?;
                Ok(ActiveGeneration::Legacy(root))
            }
            sharded_generation::VERSION => {
                let root: sharded_generation::ShardedGeneration = model::open_versioned(
                    &row,
                    model::ROOT_ID,
                    generation,
                    sharded_generation::VERSION,
                    None,
                )?;
                root.validate()?;
                self.verify_epoch(generation, root.generation, &root.writer_floor_sha256)?;
                Ok(ActiveGeneration::Sharded(root))
            }
            _ => Err(CaptureError::Corrupt.into()),
        }
    }

    fn verify_epoch(
        &self,
        envelope: u64,
        payload: u64,
        writer_floor: &str,
    ) -> Result<(), CaptureError> {
        if envelope != payload || writer_floor != self.writer_floor {
            return Err(CaptureError::Unavailable);
        }
        Ok(())
    }

    async fn manifest<T: Serialize + DeserializeOwned + Default>(
        &self,
        id: &str,
        generation: u64,
        expected: Option<&str>,
        snapshot: bool,
        session: &mut ClientSession,
    ) -> Result<T, TransactionError> {
        let mut action = self.manifests.find_one(doc! {"_id": id});
        if snapshot {
            action = action.read_concern(ReadConcern::snapshot());
        }
        let row = action.session(session).await?;
        match (row, expected) {
            (Some(row), Some(expected)) => Ok(model::open(&row, id, generation, Some(expected))?),
            (None, None) => Ok(T::default()),
            _ => Err(CaptureError::Corrupt.into()),
        }
    }

    async fn replace_manifest<T: Serialize>(
        &self,
        id: &str,
        generation: u64,
        previous: Option<&str>,
        value: &T,
        session: &mut ClientSession,
    ) -> Result<String, TransactionError> {
        self.write_manifest(id, generation, 1, previous, value, session)
            .await
    }

    async fn write_manifest<T: Serialize>(
        &self,
        id: &str,
        generation: u64,
        version: i64,
        previous: Option<&str>,
        value: &T,
        session: &mut ClientSession,
    ) -> Result<String, TransactionError> {
        let row = model::sealed_versioned(id, generation, version, value)?;
        let digest = model::digest(value)?;
        match previous {
            Some(previous) => {
                let result = self.manifests.replace_one(
                    doc! {"_id": id, "generation": model::signed(generation)?, "schema_version": version, "payload_sha256": previous},
                    row,
                ).session(session).await?;
                if result.matched_count != 1 {
                    return Err(CaptureError::Unavailable.into());
                }
            }
            None => {
                self.manifests.insert_one(row).session(session).await?;
            }
        }
        Ok(digest)
    }
}

pub(super) async fn started(database: &Database) -> Result<bool, CaptureError> {
    Ok(majority_collection::<Document>(database, MANIFESTS)
        .find_one(doc! {"_id": model::ROOT_ID})
        .await
        .map_err(|_| CaptureError::Unavailable)?
        .is_some())
}

pub(super) fn store_error(error: TransactionError) -> CaptureError {
    match error {
        TransactionError::Store(error) => error,
        TransactionError::Driver(_) => CaptureError::Unavailable,
    }
}

#[cfg(test)]
mod tests;
