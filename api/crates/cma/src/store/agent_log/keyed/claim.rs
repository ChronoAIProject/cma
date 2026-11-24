//! A majority-durable attempt fence survives process loss before commit settles.
//! Receipt absence never releases it. Only a locally known abort may do so;
//! successful publication consumes it atomically with the immutable receipt.
use super::*;
use mongodb::bson::Document;

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
        let stored = update_marker(store, current, doc! {
            "_id":current.id.as_str(),"revision":current.revision as i64,
            "last_seq":current.conversation.last_seq as i64,
            "creation_key":&current.creation_key,"creation_fingerprint":&current.creation_fingerprint,
            "record.created_at":&current.created_at,FIELD:{"$exists":false},
        }, doc! {"$set":{FIELD:&claim.record}}).await?;
        if !stored {
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
        self,
        store: &MongoSandboxStore,
        current: &Agent,
    ) -> Result<(), SandboxStoreError> {
        // The exact marker and unheld Sandbox authorize cleanup independently
        // of a superseded work claim. The cleanup can finish if the caller drops.
        let store = store.clone();
        let current = current.clone();
        tokio::spawn(async move {
            let removed = update_marker(
                &store,
                &current,
                doc! {"_id":self.agent_id.as_str(),FIELD:&self.record},
                doc! {"$unset":{FIELD:""}},
            )
            .await?;
            if !removed {
                return Err(SandboxStoreError::Conflict);
            }
            Ok(())
        })
        .await
        .map_err(|_| SandboxStoreError::Unavailable("keyed abort cleanup interrupted".into()))?
    }
}

async fn update_marker(
    store: &MongoSandboxStore,
    current: &Agent,
    filter: Document,
    update: Document,
) -> Result<bool, SandboxStoreError> {
    let mut session = store.client.start_session().await.map_err(driver)?;
    start_transaction(&mut session).await.map_err(driver)?;
    let applied = async {
        // Legacy independently retained Agents can have no Sandbox document.
        crate::store::work_publication::agent::fence_parent(store, current, &mut session).await?;
        store
            .agents
            .update_one(filter, update)
            .session(&mut session)
            .await
            .map_err(TransactionLogError::Driver)
    }
    .await;
    match applied {
        Ok(result) if result.matched_count == 1 => {}
        other => {
            session.abort_transaction().await.map_err(driver)?;
            return other.map(|_| false).map_err(domain);
        }
    }
    match commit_transaction(&mut session).await {
        CommitOutcome::Committed => Ok(true),
        CommitOutcome::Retry(_) => Err(SandboxStoreError::Conflict),
        CommitOutcome::Unknown(error) | CommitOutcome::Failed(error) => Err(driver(error)),
    }
}
