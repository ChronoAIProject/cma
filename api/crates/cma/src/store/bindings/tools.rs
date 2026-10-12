//! Durable provider tool-open authority; no dependency on legacy bridge records.
use super::*;
use crate::store::sandbox_mongo::{
    commit_transaction, read_record, record_bson, start_transaction, CommitOutcome,
};
use layer3_adapter::hosted::{
    HostedProviderToolBinding, HostedProviderToolSource, HostedToolOpenReceipt,
};
use layer3_adapter::{AdapterError, AgentOpenRequest, RuntimeHandle};
use mongodb::ClientSession;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[cfg(test)]
mod tests;
mod validation;
use validation::Scope;

pub(crate) struct MongoProviderToolSource {
    store: Arc<MongoSandboxStore>,
}

impl MongoProviderToolSource {
    pub(crate) fn new(store: Arc<MongoSandboxStore>) -> Arc<Self> {
        Arc::new(Self { store })
    }

    async fn resolve_in(
        &self,
        runtime: &RuntimeHandle,
        request: &AgentOpenRequest,
        fence: u64,
        session: &mut ClientSession,
    ) -> layer3_adapter::Result<Option<(HostedProviderToolBinding, Scope)>> {
        let Some(document) = self
            .store
            .bindings
            .conversations
            .find_one(
                doc! {"record.agent_id": &request.options.slot.0, "record.borrowed": {"$ne": true}},
            )
            .session(&mut *session)
            .await
            .map_err(unavailable)?
        else {
            return Ok(None);
        };
        let conversation = read_conversation(document).map_err(unavailable)?;
        let binding = self
            .store
            .bindings
            .records
            .find_one(doc! {"_id": &conversation.binding_id})
            .session(&mut *session)
            .await
            .map_err(unavailable)?
            .map(read_binding)
            .transpose()
            .map_err(unavailable)?
            .ok_or_else(refused)?;
        if binding.tool_endpoint.is_none() {
            return Ok(None);
        }
        let snapshot = crate::store::agent_provenance::read_snapshot(
            &self.store,
            &conversation.agent_id,
            session,
        )
        .await
        .map_err(unavailable)?;
        let (scope, proxy) =
            validation::validate(&snapshot, &binding, &conversation, runtime, request, fence)?;
        let id = scope.id()?;
        let previous = self
            .store
            .bindings
            .tool_opens
            .find_one(doc! {"_id": &id})
            .session(&mut *session)
            .await
            .map_err(unavailable)?;
        let receipt = previous
            .as_ref()
            .map(|document| read(document, &scope))
            .transpose()?;
        // A binding write or conversation retirement conflicts with this fence.
        // Lineage fencing similarly excludes a stopped/replaced Agent or parent.
        let guarded = self.store.bindings.records.update_one(
            doc! {"_id": &binding.binding_id, "revision": integer(binding.revision).map_err(unavailable)?,
                "document_schema_version": DOCUMENT_SCHEMA_VERSION},
            doc! {"$inc": {"tool_open_fence": 1_i64}},
        ).session(&mut *session).await.map_err(unavailable)?;
        if guarded.matched_count != 1 {
            return Err(refused());
        }
        crate::store::resource_access::fence_lineage(&self.store, &snapshot.resource, session)
            .await
            .map_err(unavailable)?;
        let attachment = HostedProviderToolBinding::new(
            id,
            runtime.clone(),
            request.clone(),
            fence,
            proxy,
            receipt.map(|record| HostedToolOpenReceipt {
                boot_id: record.boot_id,
                conversation: record.conversation,
            }),
        )?;
        Ok(Some((attachment, scope)))
    }
}

#[async_trait::async_trait]
impl HostedProviderToolSource for MongoProviderToolSource {
    async fn resolve(
        &self,
        runtime: &RuntimeHandle,
        request: &AgentOpenRequest,
        authority_fence: u64,
    ) -> layer3_adapter::Result<Option<HostedProviderToolBinding>> {
        let mut session = self
            .store
            .client
            .start_session()
            .await
            .map_err(unavailable)?;
        start_transaction(&mut session).await.map_err(unavailable)?;
        let result = self
            .resolve_in(runtime, request, authority_fence, &mut session)
            .await;
        match result {
            Ok(value) => {
                committed(commit_transaction(&mut session).await)?;
                Ok(value.map(|(attachment, _)| attachment))
            }
            Err(error) => {
                let _ = session.abort_transaction().await;
                Err(error)
            }
        }
    }

    async fn confirm_open(
        &self,
        attachment: &HostedProviderToolBinding,
        conversation: &str,
    ) -> layer3_adapter::Result<()> {
        if conversation.is_empty()
            || conversation.len() > 512
            || conversation.trim() != conversation
            || conversation.chars().any(char::is_control)
        {
            return Err(refused());
        }
        let mut session = self
            .store
            .client
            .start_session()
            .await
            .map_err(unavailable)?;
        start_transaction(&mut session).await.map_err(unavailable)?;
        let result = self
            .confirm_in(attachment, conversation, &mut session)
            .await;
        match result {
            Ok(()) => committed(commit_transaction(&mut session).await),
            Err(error) => {
                let _ = session.abort_transaction().await;
                Err(error)
            }
        }
    }
}

impl MongoProviderToolSource {
    async fn confirm_in(
        &self,
        attachment: &HostedProviderToolBinding,
        conversation: &str,
        session: &mut ClientSession,
    ) -> layer3_adapter::Result<()> {
        let (runtime, request, fence, proxy) = attachment.scope();
        let observed = attachment.observed().ok_or_else(refused)?;
        observed.validate_for(proxy)?;
        let (current, scope) = self
            .resolve_in(runtime, request, fence, session)
            .await?
            .ok_or_else(refused)?;
        if current.scope() != attachment.scope()
            || current.provenance_sha256() != attachment.provenance_sha256()
            || scope
                .runtime
                .boot_id
                .as_ref()
                .is_some_and(|boot| boot != &observed.boot_id)
        {
            return Err(refused());
        }
        // The durable Agent conversation, if already installed, must agree too.
        let snapshot =
            crate::store::agent_provenance::read_snapshot(&self.store, &scope.agent_id, session)
                .await
                .map_err(unavailable)?;
        if snapshot
            .resource
            .lineage
            .agent
            .as_ref()
            .ok_or_else(refused)?
            .conversation
            .conversation_id
            .as_deref()
            .is_some_and(|id| id != conversation)
        {
            return Err(refused());
        }
        let id = scope.id()?;
        let previous = self
            .store
            .bindings
            .tool_opens
            .find_one(doc! {"_id": &id})
            .session(&mut *session)
            .await
            .map_err(unavailable)?;
        if previous
            .as_ref()
            .map(|document| read(document, &scope))
            .transpose()?
            .is_some_and(|receipt| receipt.conversation != conversation)
        {
            return Err(refused());
        }
        let record = OpenRecord {
            scope,
            boot_id: observed.boot_id.clone(),
            conversation: conversation.into(),
        };
        let document = doc! {"_id": &id, "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        "record": record_bson(&record).map_err(unavailable)?};
        self.store
            .bindings
            .tool_opens
            .replace_one(doc! {"_id": id}, document)
            .upsert(true)
            .session(&mut *session)
            .await
            .map_err(unavailable)?;
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OpenRecord {
    scope: Scope,
    boot_id: String,
    conversation: String,
}

fn read(document: &Document, scope: &Scope) -> layer3_adapter::Result<OpenRecord> {
    let record: OpenRecord = read_record(document).map_err(unavailable)?;
    if document.get_str("_id").ok() != Some(scope.id()?.as_str()) || &record.scope != scope {
        return Err(refused());
    }
    Ok(record)
}

fn committed(outcome: CommitOutcome) -> layer3_adapter::Result<()> {
    match outcome {
        CommitOutcome::Committed => Ok(()),
        _ => Err(unavailable(())),
    }
}

fn refused() -> AdapterError {
    AdapterError::Rejected("provider tool creation or runtime authority does not match".into())
}

fn unavailable<T>(_: T) -> AdapterError {
    AdapterError::Unavailable("provider tool authority storage is unavailable".into())
}
