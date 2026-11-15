use mongodb::{
    Client, ClientSession, Collection, Database,
    bson::{self, Document, doc},
    options::{CollectionOptions, ReadConcern, ReadPreference, SelectionCriteria, WriteConcern},
};
use serde::{Serialize, de::DeserializeOwned};

use super::*;

#[derive(Clone)]
pub(crate) struct AuthorityStore {
    pub(super) client: Client,
    pub(super) heads: Collection<Document>,
    pub(super) operations: Collection<Document>,
    pub(super) effects: Collection<Document>,
}

impl AuthorityStore {
    /// Only verifies preinstalled collections; never performs startup DDL.
    pub(crate) async fn open(client: Client, database: Database) -> Result<Self> {
        let names = database
            .list_collection_names()
            .await
            .map_err(unavailable)?;
        if COLLECTIONS
            .iter()
            .any(|name| !names.iter().any(|n| n == name))
        {
            return Err(AuthorityError::Unavailable);
        }
        let options = CollectionOptions::builder()
            .read_concern(ReadConcern::majority())
            .write_concern(WriteConcern::majority())
            .selection_criteria(SelectionCriteria::ReadPreference(ReadPreference::Primary))
            .build();
        Ok(Self {
            client,
            heads: database.collection_with_options(HEADS, options.clone()),
            operations: database.collection_with_options(OPERATIONS, options.clone()),
            effects: database.collection_with_options(EFFECTS, options),
        })
    }

    pub(crate) async fn load(&self, deployment: &str) -> Result<Option<ReleaseHead>> {
        validation::identity(deployment)?;
        self.heads
            .find_one(doc! {"_id": deployment})
            .await
            .map_err(unavailable)?
            .map(read_head)
            .transpose()
    }

    pub(crate) async fn operation(
        &self,
        deployment: &str,
        id: &str,
    ) -> Result<Option<ReleaseOperation>> {
        let key = validation::key(deployment, id)?;
        self.operations
            .find_one(doc! {"_id": key})
            .await
            .map_err(unavailable)?
            .map(read_operation)
            .transpose()
    }

    pub(crate) async fn effect(&self, deployment: &str, id: &str) -> Result<Option<EffectPermit>> {
        let key = validation::key(deployment, id)?;
        self.effects
            .find_one(doc! {"_id": key})
            .await
            .map_err(unavailable)?
            .map(read_effect)
            .transpose()
    }

    pub(super) async fn transaction(&self) -> Result<ClientSession> {
        let mut session = self.client.start_session().await.map_err(unavailable)?;
        session
            .start_transaction()
            .read_concern(ReadConcern::snapshot())
            .write_concern(WriteConcern::majority())
            .selection_criteria(SelectionCriteria::ReadPreference(ReadPreference::Primary))
            .await
            .map_err(unavailable)?;
        Ok(session)
    }

    pub(super) async fn head_in(
        &self,
        deployment: &str,
        session: &mut ClientSession,
    ) -> Result<ReleaseHead> {
        self.heads
            .find_one(doc! {"_id": deployment})
            .session(session)
            .await
            .map_err(unavailable)?
            .ok_or(AuthorityError::Closed)
            .and_then(read_head)
    }

    pub(super) async fn operation_in(
        &self,
        key: &str,
        session: &mut ClientSession,
    ) -> Result<Option<ReleaseOperation>> {
        self.operations
            .find_one(doc! {"_id": key})
            .session(session)
            .await
            .map_err(unavailable)?
            .map(read_operation)
            .transpose()
    }

    pub(super) async fn effect_in(
        &self,
        key: &str,
        session: &mut ClientSession,
    ) -> Result<Option<EffectPermit>> {
        self.effects
            .find_one(doc! {"_id": key})
            .session(session)
            .await
            .map_err(unavailable)?
            .map(read_effect)
            .transpose()
    }

    pub(super) async fn write_head(
        &self,
        previous: &ReleaseHead,
        next: &ReleaseHead,
        release_commit: bool,
        session: &mut ClientSession,
    ) -> Result<()> {
        next.validate()?;
        let mut set = document(next)?;
        set.remove("_id");
        let mut update = doc! {"$set": set};
        if release_commit {
            update
                .get_document_mut("$set")
                .map_err(|_| AuthorityError::Invalid)?
                .remove("committed_at");
            update.insert("$currentDate", doc! {"committed_at": true});
        }
        let result = self.heads.update_one(
            doc! {"_id": &previous.deployment, "schema_version": 1_i64,
                "epoch": previous.epoch, "control_revision": previous.control_revision,
                "admission_sequence": previous.admission_sequence, "active_effects": previous.active_effects},
            update,
        ).session(session).await.map_err(unavailable)?;
        if result.matched_count != 1 {
            return Err(AuthorityError::Conflict);
        }
        Ok(())
    }

    /// Checks the durable set as well as the counter, including foreign epochs.
    pub(super) async fn check_effect_count(
        &self,
        head: &ReleaseHead,
        session: &mut ClientSession,
    ) -> Result<()> {
        let count = self
            .effects
            .count_documents(doc! {
                "writer.deployment": &head.deployment, "terminal_evidence_sha256": bson::Bson::Null,
            })
            .limit(10_001)
            .session(session)
            .await
            .map_err(unavailable)?;
        if count > 10_000 || i64::try_from(count).ok() != Some(head.active_effects) {
            return Err(AuthorityError::EvidenceRequired);
        }
        Ok(())
    }
}

pub(super) fn unavailable(_: mongodb::error::Error) -> AuthorityError {
    AuthorityError::Unavailable
}

pub(super) fn document(value: &impl Serialize) -> Result<Document> {
    bson::serialize_to_document(value).map_err(|_| AuthorityError::Invalid)
}

fn decode<T: DeserializeOwned>(document: Document) -> Result<T> {
    bson::deserialize_from_document(document).map_err(|_| AuthorityError::Invalid)
}

fn read_head(document: Document) -> Result<ReleaseHead> {
    let head: ReleaseHead = decode(document)?;
    head.validate()?;
    if head.committed_at.timestamp_millis() <= 0 {
        return Err(AuthorityError::Invalid);
    }
    Ok(head)
}

fn read_operation(document: Document) -> Result<ReleaseOperation> {
    let operation: ReleaseOperation = decode(document)?;
    operation.validate()?;
    Ok(operation)
}

fn read_effect(document: Document) -> Result<EffectPermit> {
    let effect: EffectPermit = decode(document)?;
    effect.validate()?;
    Ok(effect)
}

pub(super) async fn finish<T>(session: &mut ClientSession, result: Result<T>) -> Result<T> {
    match result {
        Ok(value) => {
            // No automatic retry or new identity after a possibly committed write.
            session
                .commit_transaction()
                .await
                .map_err(|_| AuthorityError::Uncertain)?;
            Ok(value)
        }
        Err(error) => {
            let _ = session.abort_transaction().await;
            Err(error)
        }
    }
}
