//! A majority-durable attempt fence survives process loss before commit settles.
//! Receipt absence never releases it. Only a locally known abort may do so;
//! successful publication consumes it atomically with the immutable receipt.
use super::*;
use mongodb::{bson::Document, options::WriteConcern};

const FIELD: &str = "keyed_publication_attempt_v1";

pub(super) async fn ensure_clear(
    store: &MongoSandboxStore,
    id: &AgentId,
) -> Result<(), SandboxStoreError> {
    if store
        .agents
        .find_one(doc! {"_id":id.as_str(),FIELD:{"$exists":true}})
        .projection(doc! {"_id":1})
        .read_concern(ReadConcern::majority())
        .await
        .map_err(driver)?
        .is_some()
    {
        return Err(SandboxStoreError::Unavailable(
            "keyed Agent log attempt remains unresolved; reconcile its original receipt before retry".into(),
        ));
    }
    Ok(())
}

pub(super) struct Claim {
    agent_id: AgentId,
    record: Document,
}

impl Claim {
    pub(super) async fn acquire(
        store: &MongoSandboxStore,
        current: &Agent,
        operation: &PublicationIdentity,
        entry: &AgentLogEntry,
    ) -> Result<Self, SandboxStoreError> {
        let claim = Self {
            agent_id: current.id.clone(),
            record: doc! {
                "format_version":1_i64,
                "creation_key":&current.creation_key,"created_at":&current.created_at,
                "publication_key":&operation.key,"fingerprint":&operation.fingerprint,
                "appended_at":&operation.appended_at,
                "expected_revision":signed(current.revision,"publication revision").map_err(SandboxStoreError::Corrupt)?,
                "expected_seq":signed(current.conversation.last_seq,"publication sequence").map_err(SandboxStoreError::Corrupt)?,
                "result_digest":publication::result_digest(entry)?,
                "token":uuid::Uuid::new_v4().to_string(),
            },
        };
        let result = store.agents.update_one(doc! {
            "_id":current.id.as_str(),"revision":current.revision as i64,
            "last_seq":current.conversation.last_seq as i64,
            "creation_key":&current.creation_key,"creation_fingerprint":&current.creation_fingerprint,
            "record.created_at":&current.created_at,FIELD:{"$exists":false},
        }, doc! {"$set":{FIELD:&claim.record}})
            .write_concern(WriteConcern::majority()).await.map_err(driver)?;
        if result.matched_count != 1 {
            return Err(SandboxStoreError::Unavailable(
                "keyed Agent log attempt is pending or its source changed; receipt absence is not KnownAbort".into(),
            ));
        }
        Ok(claim)
    }

    /// This write shares the publication transaction. Abort restores the claim;
    /// a lost commit reply leaves either the claim or the completion receipt.
    pub(super) async fn consume(
        &self,
        store: &MongoSandboxStore,
        scope: &mut TransactionScope<'_>,
    ) -> Result<(), TransactionLogError> {
        let result = scope
            .update_one(
                &store.agents,
                doc! {"_id":self.agent_id.as_str(),FIELD:&self.record},
                doc! {"$unset":{FIELD:""}},
            )
            .await
            .map_err(TransactionLogError::Driver)?;
        if result.matched_count != 1 {
            return Err(SandboxStoreError::Conflict.into());
        }
        Ok(())
    }

    /// Called only before any commit was sent or after a positive KnownAbort.
    pub(super) async fn release_known_abort(
        &self,
        store: &MongoSandboxStore,
    ) -> Result<(), SandboxStoreError> {
        let result = store
            .agents
            .update_one(
                doc! {"_id":self.agent_id.as_str(),FIELD:&self.record},
                doc! {"$unset":{FIELD:""}},
            )
            .write_concern(WriteConcern::majority())
            .await
            .map_err(driver)?;
        if result.matched_count != 1 {
            return Err(SandboxStoreError::Conflict);
        }
        Ok(())
    }
}
