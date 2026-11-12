//! Durable provider tool-open authority; no dependency on legacy bridge records.
use super::*;
use crate::store::sandbox_mongo::{read_record, record_bson};
use layer3_adapter::hosted::{
    HostedProviderToolBinding, HostedProviderToolSource, HostedToolOpenReceipt,
};
use layer3_adapter::{AdapterError, AgentOpenRequest, RuntimeHandle};
use mongodb::ClientSession;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

mod relay;
mod relay_auth;
#[cfg(test)]
mod relay_tests;
mod relay_transport;
#[cfg(test)]
mod tests;
mod transaction;
#[cfg(test)]
mod transaction_tests;
mod validation;
use transaction::{Operation, Outcome};
use validation::Scope;

pub(crate) struct MongoProviderToolSource {
    standing: Arc<dyn layer2_core::account_deletion::AccountStanding>,
    store: Arc<MongoSandboxStore>,
    relay: Option<relay::Authority>,
}

impl MongoProviderToolSource {
    pub(crate) fn new(store: Arc<MongoSandboxStore>) -> Arc<Self> {
        Arc::new(Self {
            store,
            relay: None,
            standing: Arc::new(layer2_core::account_deletion::AvailableAccounts),
        })
    }
    pub(crate) fn with_account_standing(
        mut self: Arc<Self>,
        standing: Arc<dyn layer2_core::account_deletion::AccountStanding>,
    ) -> Arc<Self> {
        Arc::get_mut(&mut self)
            .expect("relay standing configured before sharing")
            .standing = standing;
        self
    }

    async fn resolve_in(
        &self,
        runtime: &RuntimeHandle,
        request: &AgentOpenRequest,
        fence: u64,
        session: &mut ClientSession,
    ) -> transaction::Result<Option<(HostedProviderToolBinding, Scope)>> {
        let Some(document) = self
            .store
            .bindings
            .conversations
            .find_one(
                doc! {"record.agent_id": &request.options.slot.0, "record.borrowed": {"$ne": true}},
            )
            .session(&mut *session)
            .await?
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
            .await?
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
        .await?;
        let (scope, proxy) =
            validation::validate(&snapshot, &binding, &conversation, runtime, request, fence)?;
        let id = scope.id()?;
        let previous = self
            .store
            .bindings
            .tool_opens
            .find_one(doc! {"_id": &id})
            .session(&mut *session)
            .await?;
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
        ).session(&mut *session).await?;
        if guarded.matched_count != 1 {
            return Err(refused().into());
        }
        crate::store::resource_access::fence_lineage(&self.store, &snapshot.resource, session)
            .await?;
        let attachment = HostedProviderToolBinding::new(
            id,
            runtime.clone(),
            request.clone(),
            fence,
            proxy,
            receipt.as_ref().map(|record| HostedToolOpenReceipt {
                boot_id: record.boot_id.clone(),
                conversation: record.conversation.clone(),
            }),
        )?;
        let attachment = self
            .relay_for_open(
                attachment,
                &scope,
                receipt.is_some()
                    || request.resume_conversation.is_some()
                    || snapshot
                        .resource
                        .lineage
                        .agent
                        .as_ref()
                        .is_some_and(|agent| agent.conversation.conversation_id.is_some()),
                session,
            )
            .await?;
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
        match self
            .run(Operation::Resolve(runtime, request, authority_fence))
            .await?
        {
            Outcome::Resolved(binding) => Ok(binding.map(|binding| *binding)),
            _ => unreachable!("resolve returns a binding"),
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
        self.run(Operation::Confirm(attachment, conversation))
            .await?;
        Ok(())
    }

    async fn prepare_transport(
        &self,
        binding: &HostedProviderToolBinding,
        origin: &str,
        conversation: &str,
        sequence: u64,
    ) -> layer3_adapter::Result<HostedProviderToolBinding> {
        match self
            .run(Operation::Transport(
                binding,
                Some(origin),
                conversation,
                sequence,
            ))
            .await?
        {
            Outcome::Resolved(Some(binding)) => Ok(*binding),
            _ => unreachable!("transport preparation returns a binding"),
        }
    }

    async fn confirm_transport(
        &self,
        binding: &HostedProviderToolBinding,
        conversation: &str,
        sequence: u64,
    ) -> layer3_adapter::Result<()> {
        self.run(Operation::Transport(binding, None, conversation, sequence))
            .await?;
        Ok(())
    }
}

impl MongoProviderToolSource {
    async fn confirm_in(
        &self,
        attachment: &HostedProviderToolBinding,
        conversation: &str,
        session: &mut ClientSession,
    ) -> transaction::Result<()> {
        let (runtime, request, fence, proxy) = attachment.scope();
        let observed = attachment.observed().ok_or_else(refused)?;
        observed.validate_for(proxy)?;
        let (current, scope) = self
            .resolve_in(runtime, request, fence, session)
            .await?
            .ok_or_else(refused)?;
        if current.scope() != attachment.scope()
            || !current.same_transport(attachment)
            || current.provenance_sha256() != attachment.provenance_sha256()
            || scope
                .runtime
                .boot_id
                .as_ref()
                .is_some_and(|boot| boot != &observed.boot_id)
        {
            return Err(refused().into());
        }
        // The durable Agent conversation, if already installed, must agree too.
        let snapshot =
            crate::store::agent_provenance::read_snapshot(&self.store, &scope.agent_id, session)
                .await?;
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
            return Err(refused().into());
        }
        let id = scope.id()?;
        let previous = self
            .store
            .bindings
            .tool_opens
            .find_one(doc! {"_id": &id})
            .session(&mut *session)
            .await?;
        if previous
            .as_ref()
            .map(|document| read(document, &scope))
            .transpose()?
            .is_some_and(|receipt| receipt.conversation != conversation)
        {
            return Err(refused().into());
        }
        self.confirm_relay_open(attachment, &scope, conversation, session)
            .await?;
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
            .await?;
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

fn refused() -> AdapterError {
    AdapterError::Rejected("provider tool creation or runtime authority does not match".into())
}

fn unavailable<T>(_: T) -> AdapterError {
    AdapterError::Unavailable("provider tool authority storage is unavailable".into())
}
