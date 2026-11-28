use layer2_core::ids::AgentCommandId;
use layer2_core::sandbox::agent_io::{
    AgentCommandAuditStore, CommandAuthorizationAudit, CommandLedgerError,
};
use layer2_core::sandbox::store::{Page, PageRequest};
use mongodb::ClientSession;

use super::super::sandbox_mongo::MongoSandboxStore;
use super::ApplyError;

pub(in crate::store) use cma_am_agent_commands::audit::ensure_indexes;

pub(in crate::store) async fn append(
    store: &MongoSandboxStore,
    audit: &CommandAuthorizationAudit,
    session: &mut ClientSession,
) -> Result<(), ApplyError> {
    cma_am_agent_commands::audit::append(&store.command_authorization_audits, audit, session).await
}

#[async_trait::async_trait]
impl AgentCommandAuditStore for MongoSandboxStore {
    async fn command_authorization_audits(
        &self,
        command_id: &AgentCommandId,
        page: &PageRequest,
    ) -> Result<Page<CommandAuthorizationAudit>, CommandLedgerError> {
        cma_am_agent_commands::audit::list(&self.command_authorization_audits, command_id, page)
            .await
    }
}
