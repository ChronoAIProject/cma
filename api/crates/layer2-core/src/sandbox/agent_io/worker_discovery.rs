use super::*;
use crate::sandbox::store::{Page, PageRequest};
use std::collections::BTreeSet;
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct ScheduledAgentCommand {
    pub id: crate::ids::AgentCommandId,
    pub turn_wait_agent: Option<crate::AgentId>,
    pub dispatched: bool,
    pub reconcile_current: bool,
}

pub struct CommandDiscovery {
    pub page: Page<ScheduledAgentCommand>,
    pub heads_failed: bool,
}

impl AgentCommandWorker {
    /// Head reads are optional scheduling hints. Unavailable hints must not
    /// prevent the authoritative reconciliation from making progress.
    pub async fn discover_scheduled(
        &self,
        request: &PageRequest,
    ) -> Result<CommandDiscovery, CommandLedgerError> {
        if usize::from(request.limit) > MAX_COMMAND_RECONCILE_HEADS {
            return Err(CommandLedgerError::Invalid);
        }
        let page = self.discover(request).await?;
        let ids: Vec<_> = page
            .items
            .iter()
            .filter_map(|command| command.work.dispatch.as_ref())
            .map(|dispatch| dispatch.claim.agent_id.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let mut heads_failed = false;
        let heads = if self.dispatched_recheck_delay.is_zero() || ids.is_empty() {
            Vec::new()
        } else {
            match tokio::time::timeout(
                Duration::from_secs(1),
                self.store.command_reconcile_heads(&ids),
            )
            .await
            {
                Ok(Ok(heads)) => heads,
                result => {
                    heads_failed = true;
                    tracing::warn!(
                        timed_out = result.is_err(),
                        "Command reconciliation heads unavailable; using complete evidence"
                    );
                    Vec::new()
                }
            }
        };
        let page = Page {
            items: page
                .items
                .into_iter()
                .map(|command| ScheduledAgentCommand {
                    turn_wait_agent: (command.work.dispatch.is_none()
                        && command.receipt.envelope.is_ordinary_append())
                    .then(|| command.receipt.envelope.agent_id.clone()),
                    reconcile_current: heads
                        .iter()
                        .any(|head| command.reconciliation_current(head)),
                    dispatched: command.work.dispatch.is_some(),
                    id: command.receipt.envelope.command_id,
                })
                .collect(),
            next: page.next,
        };
        Ok(CommandDiscovery {
            page: self.include_funding_work(request, page).await,
            heads_failed,
        })
    }
}
