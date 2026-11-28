use super::*;
use crate::sandbox::store::{Page, PageRequest};
use std::{collections::BTreeMap, time::Duration};

impl AgentCommandWorker {
    pub(super) async fn reconcile_funding(&self, command: &AdmittedAgentCommand) -> Duration {
        let Some(funding) = &self.model_funding else {
            return Duration::ZERO;
        };
        if !command.status.is_terminal() && command.work.dispatch.is_none() {
            return Duration::ZERO;
        }
        let result =
            tokio::time::timeout(worker::IO_BOUND, funding.reconcile_message(command)).await;
        if matches!(result, Ok(Ok(()))) {
            return Duration::ZERO;
        }
        tracing::warn!(command_id = %command.receipt.envelope.command_id,
            error_kind = ?result, "Model funding settlement is unresolved");
        worker::UNCLAIMED_RECHECK_DELAY
    }

    /// Merge two equally bounded, ordered indexes using the shared command-ID
    /// cursor. Funding obligations remain discoverable after ledger termination.
    pub(super) async fn include_funding_work(
        &self,
        request: &PageRequest,
        ordinary: Page<ScheduledAgentCommand>,
    ) -> Page<ScheduledAgentCommand> {
        let Some(funding) = &self.model_funding else {
            return ordinary;
        };
        let result =
            tokio::time::timeout(Duration::from_secs(1), funding.pending_commands(request)).await;
        let Ok(Ok(pending)) = result else {
            tracing::warn!(error_kind = ?result, "Pending model funding discovery unavailable");
            return ordinary;
        };
        let limit = usize::from(request.limit);
        let mut previous = request.cursor.as_deref();
        if pending.items.len() > limit
            || pending.next.as_ref().is_some_and(|next| {
                pending.items.len() != limit
                    || pending
                        .items
                        .last()
                        .is_none_or(|last| next != last.as_str())
            })
            || pending.items.iter().any(|id| {
                let unordered = previous.is_some_and(|prev| prev >= id.as_str());
                previous = Some(id.as_str());
                unordered
            })
        {
            tracing::warn!("Pending model funding discovery returned an invalid page");
            return ordinary;
        }
        let mut items: BTreeMap<_, _> = ordinary
            .items
            .into_iter()
            .map(|item| (item.id.to_string(), item))
            .collect();
        for id in pending.items {
            let item = items
                .entry(id.to_string())
                .or_insert_with(|| ScheduledAgentCommand {
                    id,
                    turn_wait_agent: None,
                    dispatched: false,
                    reconcile_current: false,
                });
            item.reconcile_current = false;
        }
        let has_more = ordinary.next.is_some() || pending.next.is_some() || items.len() > limit;
        let items: Vec<_> = items.into_values().take(limit).collect();
        let next = if has_more {
            items.last().map(|item| item.id.to_string())
        } else {
            None
        };
        Page { items, next }
    }
}
