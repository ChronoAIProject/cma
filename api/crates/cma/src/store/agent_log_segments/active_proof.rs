//! A separately versioned, revision-bound proof of the original active BSON.
//! Older writers invalidate it automatically by advancing the log head.
use super::{corrupt, digest, Head};
use crate::store::{
    agent_log_integrity::TransactionLogError, durability::majority_collection,
    sandbox_mongo::MongoSandboxStore,
};
use mongodb::{
    bson::{self, doc, Binary, Document, RawDocumentBuf},
    options::ReadConcern,
    ClientSession,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub(in crate::store) const COLLECTION: &str = "agent_log_active_proofs_v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ActiveProof {
    #[serde(rename = "_id")]
    id: String,
    format_version: i64,
    head: Head,
    agent_revision: i64,
    leaves: Binary,
    proof_sha256: Binary,
}

impl ActiveProof {
    fn new(
        head: &Head,
        agent_revision: u64,
        leaves: Binary,
    ) -> Result<Self, layer2_core::sandbox::store::SandboxStoreError> {
        let agent_revision = i64::try_from(agent_revision).map_err(|_| corrupt())?;
        let proof_sha256 = checksum(head, agent_revision, &leaves)?;
        let value = Self {
            id: head.agent_id.clone(),
            format_version: 1,
            head: head.clone(),
            agent_revision,
            leaves,
            proof_sha256,
        };
        value.validate()?;
        Ok(value)
    }

    fn validate(&self) -> Result<(), layer2_core::sandbox::store::SandboxStoreError> {
        self.head.validate()?;
        if self.id != self.head.agent_id
            || self.format_version != 1
            || self.leaves.subtype != mongodb::bson::spec::BinarySubtype::Generic
            || i64::try_from(self.leaves.bytes.len()).ok() != self.head.active_count.checked_mul(32)
            || self.agent_revision < 1
            || self.proof_sha256 != checksum(&self.head, self.agent_revision, &self.leaves)?
        {
            return Err(corrupt());
        }
        Ok(())
    }

    pub(super) fn successor(
        before: &Head,
        current_revision: u64,
        next_revision: u64,
        after: &super::writer_plan::PublicationPlan,
        prior: &Binary,
        batch: &[RawDocumentBuf],
    ) -> Result<Self, layer2_core::sandbox::store::SandboxStoreError> {
        // The complete predecessor leaves came from a validated full source
        // scan or a certificate bound to that precise transaction head.
        Self::new(before, current_revision, prior.clone())?;
        let mut leaves = if after.seal.is_some() {
            Vec::new()
        } else {
            prior.bytes.clone()
        };
        for (offset, raw) in batch.iter().enumerate() {
            let seq = before
                .last_seq
                .checked_add(offset as i64 + 1)
                .ok_or_else(corrupt)?;
            leaves.extend_from_slice(
                &digest::leaf(&before.agent_id, &before.incarnation, seq, raw.as_bytes()).bytes,
            );
        }
        Self::new(&after.head, next_revision, digest::binary(leaves))
    }

    pub(super) async fn persist(
        &self,
        store: &MongoSandboxStore,
        session: &mut ClientSession,
    ) -> Result<(), TransactionLogError> {
        self.validate()?;
        let database = store.client.database(&store.agents.namespace().db);
        self.persist_database(&database, session).await
    }

    async fn persist_database(
        &self,
        database: &mongodb::Database,
        session: &mut ClientSession,
    ) -> Result<(), TransactionLogError> {
        let rows = majority_collection::<Document>(database, COLLECTION);
        let filter = doc! {"_id": &self.id, "head.incarnation": &self.head.incarnation,
        "$or": [
            {"head.revision": {"$lt": self.head.revision}},
            {"head.revision": self.head.revision, "agent_revision": {"$lt": self.agent_revision}},
        ]};
        let result = rows
            .replace_one(
                filter,
                bson::serialize_to_document(self).map_err(|_| corrupt())?,
            )
            .upsert(true)
            .session(session)
            .await
            .map_err(TransactionLogError::Driver)?;
        if result.matched_count != 1 && result.upserted_id.is_none() {
            return Err(corrupt().into());
        }
        Ok(())
    }
}

fn checksum(
    head: &Head,
    agent_revision: i64,
    leaves: &Binary,
) -> Result<Binary, layer2_core::sandbox::store::SandboxStoreError> {
    let mut hash = Sha256::new();
    hash.update(b"cma.agent-log.active-proof.v1\0");
    digest::field(
        &mut hash,
        &bson::serialize_to_vec(head).map_err(|_| corrupt())?,
    );
    hash.update(agent_revision.to_be_bytes());
    digest::field(&mut hash, &leaves.bytes);
    Ok(digest::binary(hash.finalize().to_vec()))
}

pub(super) async fn read(
    store: &MongoSandboxStore,
    head: &Head,
    agent_revision: u64,
    session: &mut ClientSession,
    snapshot: bool,
) -> Result<Option<Binary>, TransactionLogError> {
    let database = store.client.database(&store.agents.namespace().db);
    read_database(&database, head, agent_revision, session, snapshot).await
}

async fn read_database(
    database: &mongodb::Database,
    head: &Head,
    agent_revision: u64,
    session: &mut ClientSession,
    snapshot: bool,
) -> Result<Option<Binary>, TransactionLogError> {
    let agent_revision = i64::try_from(agent_revision).map_err(|_| corrupt())?;
    let rows = majority_collection::<Document>(database, COLLECTION);
    let query = rows.find_one(doc! {"_id": &head.agent_id});
    let query = if snapshot {
        query.read_concern(ReadConcern::snapshot())
    } else {
        query
    };
    let Some(row) = query
        .session(session)
        .await
        .map_err(TransactionLogError::Driver)?
    else {
        return Ok(None);
    };
    let proof: ActiveProof = bson::deserialize_from_document(row.clone()).map_err(|_| corrupt())?;
    if bson::serialize_to_document(&proof).map_err(|_| corrupt())? != row {
        return Err(corrupt().into());
    }
    proof.validate()?;
    if proof.id != head.agent_id
        || proof.head.incarnation != head.incarnation
        || proof.head.revision > head.revision
        || proof.agent_revision > agent_revision
    {
        return Err(corrupt().into());
    }
    if proof.head.revision == head.revision && proof.head != *head {
        return Err(corrupt().into());
    }
    if proof.head.revision < head.revision || proof.agent_revision < agent_revision {
        return Ok(None);
    }
    Ok(Some(proof.leaves))
}

impl MongoSandboxStore {
    pub(crate) async fn configure_active_log_proofs(
        mut self,
        enabled: bool,
    ) -> Result<Self, &'static str> {
        if enabled {
            let database = self.client.database(&self.agents.namespace().db);
            let status = database
                .run_command(doc! {"connectionStatus":1, "showPrivileges":true})
                .await
                .map_err(|_| "Agent log active proof authority unavailable")?;
            let required = doc! {"privileges":[{"resource":{"db":database.name(),"collection":COLLECTION},
            "actions":["find","insert","update","remove","listIndexes"]}]};
            cma_service_auth::mongodb_authority::verify_shared_schema(
                &status,
                database.name(),
                &required,
            )
            .map_err(|_| "Agent log active proof authority incomplete")?;
            if !crate::store::migrations::agent_log_active_proofs::inspect(&database)
                .await?
                .is_empty()
            {
                return Err("Agent log active proof schema must be migrated");
            }
        }
        self.active_log_proofs = enabled;
        Ok(self)
    }
}

mod prepare;
pub(in crate::store) use prepare::prepare;
