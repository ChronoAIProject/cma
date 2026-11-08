use crate::failure::{Result, RuntimeError};
use crate::mutation_journal::{self as v1, Record, Subject};
use crate::mutation_ledger::Ledger;
use serde::Serialize;
use std::sync::Arc;

pub(crate) struct Ticket(Backend);
enum Backend {
    Legacy(v1::Ticket),
    Indexed(Arc<Ledger>, Record),
}
impl std::fmt::Debug for Ticket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("mutation ticket")
    }
}
impl Ticket {
    pub(super) fn legacy(ticket: v1::Ticket) -> Self {
        Self(Backend::Legacy(ticket))
    }
    pub(super) fn indexed(ledger: Arc<Ledger>, record: Record) -> Self {
        Self(Backend::Indexed(ledger, record))
    }
    pub(crate) async fn complete<T: Serialize + Sync>(self, receipt: &T) -> Result<()> {
        match self.0 {
            Backend::Legacy(ticket) => ticket.complete(receipt),
            Backend::Indexed(ledger, record) => ledger.finish(record.completed(receipt)?).await,
        }
    }
    pub(crate) async fn fail(self, error: &RuntimeError) -> Result<()> {
        match self.0 {
            Backend::Legacy(ticket) => ticket.fail(error),
            Backend::Indexed(ledger, record) => ledger.finish(record.failed(error, false)?).await,
        }
    }
    pub(crate) async fn supersede(self) -> Result<()> {
        match self.0 {
            Backend::Legacy(ticket) => ticket.supersede(),
            Backend::Indexed(ledger, record) => {
                ledger
                    .finish(record.failed(
                        &RuntimeError::rejected(
                            "mutation_authority_superseded",
                            "runtime mutation was superseded by newer resource authority",
                        ),
                        false,
                    )?)
                    .await
            }
        }
    }
    pub(crate) async fn indeterminate(self, detail: &str) -> Result<RuntimeError> {
        match self.0 {
            Backend::Legacy(ticket) => ticket.indeterminate(detail),
            Backend::Indexed(ledger, record) => {
                let code = match record.operation().subject {
                    Subject::OpenAgent { .. } => "agent_open_outcome_unknown",
                    Subject::CloseAgent { .. } => "agent_close_outcome_unknown",
                    _ => "runtime_mutation_outcome_unknown",
                };
                let error = RuntimeError::rejected(code, detail);
                ledger.finish(record.failed(&error, true)?).await?;
                Ok(error)
            }
        }
    }
}

pub(crate) async fn finish<T: Serialize + Sync>(
    ticket: Ticket,
    result: Result<T>,
    response: impl FnOnce(T) -> layer3_adapter::wire::ResponseFrame,
) -> Result<layer3_adapter::wire::ResponseFrame> {
    match result {
        Ok(receipt) => {
            ticket.complete(&receipt).await?;
            Ok(response(receipt))
        }
        Err(error) => {
            if !error.retryable {
                ticket.fail(&error).await?;
            }
            Err(error)
        }
    }
}
