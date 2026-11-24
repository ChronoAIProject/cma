use super::*;
use crate::store::sandbox_mongo::{commit_transaction, start_transaction, CommitOutcome};
use crate::store::transaction_terminal::GuardedTerminal as _;
use mongodb::ClientSession;

impl MongoSandboxStore {
    pub(super) async fn commit_binding_conversation(
        &self,
        binding_revision: u64,
        expected: Option<u64>,
        next: BindingConversation,
    ) -> Result<(), SandboxStoreError> {
        let mut session = self.client.start_session().await.map_err(driver)?;
        start_transaction(&mut session).await.map_err(driver)?;
        if let Err(error) = apply(self, binding_revision, expected, &next, &mut session).await {
            let _ = session.abort_guarded().await;
            return Err(error);
        }
        match commit_transaction(&mut session).await {
            CommitOutcome::Committed => Ok(()),
            CommitOutcome::Retry(error) | CommitOutcome::Failed(error) => Err(driver(error)),
            CommitOutcome::Unknown(_) => {
                // Exact durable equality proves an acknowledged effect without
                // treating a later write as this request's successful commit.
                if self
                    .binding_conversation(&next.binding_id, next.generation, &next.conversation_id)
                    .await?
                    .as_ref()
                    == Some(&next)
                {
                    Ok(())
                } else {
                    Err(SandboxStoreError::Unavailable(
                        "binding conversation commit is unknown".into(),
                    ))
                }
            }
        }
    }
}

async fn apply(
    store: &MongoSandboxStore,
    binding_revision: u64,
    expected: Option<u64>,
    next: &BindingConversation,
    session: &mut ClientSession,
) -> Result<(), SandboxStoreError> {
    store.bindings.safety.require_associations()?;
    let guarded = store.bindings.records.update_one(doc! {
        "_id": &next.binding_id, "revision": integer(binding_revision)?, "document_schema_version": DOCUMENT_SCHEMA_VERSION,
    }, doc! {"$inc": {"admission_fence": 1_i64}}).session(&mut *session).await.map_err(driver)?;
    if guarded.matched_count != 1 {
        return Err(SandboxStoreError::Conflict);
    }
    let binding = store
        .bindings
        .records
        .find_one(doc! {"_id": &next.binding_id})
        .session(&mut *session)
        .await
        .map_err(driver)?
        .map(read_binding)
        .transpose()?
        .ok_or(SandboxStoreError::NotFound)?;
    let key = conversation_storage_key(next);
    let current = store
        .bindings
        .conversations
        .find_one(doc! {"_id": &key})
        .session(&mut *session)
        .await
        .map_err(driver)?
        .map(read_conversation)
        .transpose()?;
    validate_conversation_write(&binding, binding_revision, current.as_ref(), expected, next)?;
    if expected.is_none()
        && current.is_none()
        && matches!(
            next.state,
            layer2_core::sandbox::binding::BindingConversationState::Provisioning
                | layer2_core::sandbox::binding::BindingConversationState::Ready
        )
    {
        crate::store::admission_checks::require_fresh(&store.budget_settings, session).await?;
    }
    let document = conversation_document(next)?;
    if let Some(expected) = expected {
        let result = store
            .bindings
            .conversations
            .replace_one(doc! {"_id": key, "revision": integer(expected)?}, document)
            .session(&mut *session)
            .await
            .map_err(driver)?;
        if result.matched_count != 1 {
            return Err(SandboxStoreError::Conflict);
        }
    } else {
        store
            .bindings
            .conversations
            .insert_one(document)
            .session(&mut *session)
            .await
            .map_err(driver)?;
    }
    Ok(())
}
