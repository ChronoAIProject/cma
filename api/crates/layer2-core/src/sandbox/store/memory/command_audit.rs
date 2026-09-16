use crate::ids::AgentCommandId;
use crate::sandbox::agent_io::{
    AgentCommandAuditStore, CommandAuthorizationAudit, CommandLedgerError,
};
use crate::sandbox::store::{InMemorySandboxStore, Page, PageRequest};

pub(super) fn append(
    state: &mut super::State,
    audit: CommandAuthorizationAudit,
) -> Result<(), CommandLedgerError> {
    audit.validate()?;
    if state.command_authorization_audits.contains_key(&audit.id) {
        return Err(CommandLedgerError::Conflict);
    }
    state
        .command_authorization_audits
        .insert(audit.id.clone(), audit);
    Ok(())
}

#[async_trait::async_trait]
impl AgentCommandAuditStore for InMemorySandboxStore {
    async fn command_authorization_audits(
        &self,
        command_id: &AgentCommandId,
        page: &PageRequest,
    ) -> Result<Page<CommandAuthorizationAudit>, CommandLedgerError> {
        PageRequest::new(page.limit, page.cursor.clone())
            .map_err(|_| CommandLedgerError::Invalid)?;
        let state = self.lock();
        let mut items: Vec<_> = state
            .command_authorization_audits
            .values()
            .filter(|audit| {
                &audit.command_id == command_id
                    && page
                        .cursor
                        .as_deref()
                        .is_none_or(|cursor| audit.id.as_str() > cursor)
            })
            .take(usize::from(page.limit) + 1)
            .cloned()
            .collect();
        for audit in &items {
            audit.validate()?;
        }
        let next = if items.len() > usize::from(page.limit) {
            items.truncate(usize::from(page.limit));
            items.last().map(|audit| audit.id.clone())
        } else {
            None
        };
        Ok(Page { items, next })
    }
}
