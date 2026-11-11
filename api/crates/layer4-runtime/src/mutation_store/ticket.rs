use crate::failure::{Result, RuntimeError};
use crate::mutation_journal::{self as v1, Record, Subject};
use crate::mutation_ledger::Ledger;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
#[cfg(test)]
#[path = "ticket_tests.rs"]
mod tests;

pub(crate) struct Ticket {
    backend: Backend,
    terminal: Option<Arc<AtomicBool>>,
}

/// Local proof that this exact recovery ticket acknowledged a terminal commit.
/// Error retryability alone says nothing about the durability of its outcome.
pub(crate) struct TerminalCommit(Arc<AtomicBool>);

impl TerminalCommit {
    pub(crate) fn absorb<T>(self, result: Result<T>) -> Result<()> {
        match result {
            Ok(_) => Ok(()),
            Err(error) if !error.retryable && self.0.load(Ordering::Acquire) => Ok(()),
            Err(error) => Err(error),
        }
    }
}
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
        Self {
            backend: Backend::Legacy(ticket),
            terminal: None,
        }
    }
    pub(super) fn indexed(ledger: Arc<Ledger>, record: Record) -> Self {
        Self {
            backend: Backend::Indexed(ledger, record),
            terminal: None,
        }
    }
    pub(crate) fn track_terminal(mut self) -> (Self, TerminalCommit) {
        let terminal = Arc::new(AtomicBool::new(false));
        self.terminal = Some(Arc::clone(&terminal));
        (self, TerminalCommit(terminal))
    }
    pub(crate) async fn complete<T: Serialize + Sync>(self, receipt: &T) -> Result<()> {
        let result = match self.backend {
            Backend::Legacy(ticket) => ticket.complete(receipt),
            Backend::Indexed(ledger, record) => ledger.finish(record.completed(receipt)?).await,
        };
        confirm(self.terminal, result)
    }
    pub(crate) async fn fail(self, error: &RuntimeError) -> Result<()> {
        let result = match self.backend {
            Backend::Legacy(ticket) => ticket.fail(error),
            Backend::Indexed(ledger, record) => ledger.finish(record.failed(error, false)?).await,
        };
        confirm(self.terminal, result)
    }
    pub(crate) async fn supersede(self) -> Result<()> {
        let result = match self.backend {
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
        };
        confirm(self.terminal, result)
    }
    pub(crate) async fn indeterminate(self, detail: &str) -> Result<RuntimeError> {
        let result = match self.backend {
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
        };
        confirm(self.terminal, result)
    }
}

fn confirm<T>(terminal: Option<Arc<AtomicBool>>, result: Result<T>) -> Result<T> {
    if result.is_ok() {
        if let Some(terminal) = terminal {
            terminal.store(true, Ordering::Release);
        }
    }
    result
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
