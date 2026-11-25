//! Generation-scoped projection of the immutable measured custody collection.
use super::{CaptureError, Document, TransactionError};
use crate::store::durability::majority_collection;
use mongodb::{bson::doc, options::ReadConcern, ClientSession, Collection, Database};
use serde::{de::DeserializeOwned, Serialize};

mod backfill;
mod model;
mod reader;
mod writer;

use crate::store::migrations::measured_index::{MANIFESTS, PROJECTIONS};

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
        let root = self.root(session, false).await?;
        if root.phase != model::Phase::Ready {
            return Err(CaptureError::Unavailable.into());
        }
        Ok(root.generation)
    }

    async fn root(
        &self,
        session: &mut ClientSession,
        snapshot: bool,
    ) -> Result<model::Generation, TransactionError> {
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
        let root: model::Generation = model::open(&row, model::ROOT_ID, generation, None)?;
        root.validate()?;
        if root.generation != generation || root.writer_floor_sha256 != self.writer_floor {
            return Err(CaptureError::Unavailable.into());
        }
        Ok(root)
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
        let row = model::sealed(id, generation, value)?;
        let digest = model::digest(value)?;
        match previous {
            Some(previous) => {
                let result = self.manifests.replace_one(
                    doc! {"_id": id, "generation": model::signed(generation)?, "payload_sha256": previous},
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
