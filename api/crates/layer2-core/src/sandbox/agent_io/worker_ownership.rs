use super::*;
use crate::agent_ownership::{Authority, OwnershipCoordinator};
use std::future::Future;

pub(super) enum WriteScope {
    Ledger,
    Owned(Arc<Authority>),
}

impl WriteScope {
    pub(super) async fn run<T>(
        self,
        work: impl Future<Output = Result<T, CommandLedgerError>>,
    ) -> Result<T, CommandLedgerError> {
        match self {
            Self::Ledger => work.await,
            Self::Owned(authority) => authority
                .run(work)
                .await
                .map_err(|_| CommandLedgerError::StaleClaim)?,
        }
    }
}

impl AgentCommandWorker {
    pub(super) async fn reconcile_dispatched(
        &self,
        id: &AgentCommandId,
        command: &AdmittedAgentCommand,
    ) -> Result<Duration, CommandLedgerError> {
        let reconciled = match bounded(self.store.reconcile_command(id)).await {
            Err(CommandLedgerError::StaleClaim)
                if self.ownership.is_some() && command.receipt.envelope.continuation.is_some() =>
            {
                let Some(scope) = self.scope(command, true).await else {
                    return Ok(UNCLAIMED_RECHECK_DELAY);
                };
                scope.run(bounded(self.store.reconcile_command(id))).await?
            }
            result => result?,
        };
        Ok(super::reconcile_pacing::recheck_delay(
            reconciled.status,
            self.dispatched_recheck_delay,
        ))
    }

    pub(super) async fn reject_without_dispatch(
        &self,
        id: &AgentCommandId,
        command: &AdmittedAgentCommand,
        code: CommandFailureCode,
    ) -> Result<Duration, CommandLedgerError> {
        let writes_agent = command.receipt.envelope.is_first_message();
        let Some(scope) = self.scope(command, writes_agent).await else {
            return Ok(UNCLAIMED_RECHECK_DELAY);
        };
        scope
            .run(async {
                bounded(self.store.reject_unclaimed_command(
                    id,
                    command.revision,
                    code,
                    now_rfc3339(),
                ))
                .await
                .map(|()| Duration::ZERO)
            })
            .await
    }

    pub fn with_ownership(&self, ownership: Option<Arc<OwnershipCoordinator>>) -> Arc<Self> {
        Arc::new(Self {
            ownership,
            ..self.clone()
        })
    }

    pub(super) async fn scope(
        &self,
        command: &AdmittedAgentCommand,
        needs_agent: bool,
    ) -> Option<WriteScope> {
        let Some(ownership) = self.ownership.as_ref().filter(|_| needs_agent) else {
            return Some(WriteScope::Ledger);
        };
        tokio::time::timeout(
            IO_BOUND,
            ownership.acquire(&command.receipt.envelope.agent_id),
        )
        .await
        .ok()?
        .ok()?
        .map(WriteScope::Owned)
    }
}
