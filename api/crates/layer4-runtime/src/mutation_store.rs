//! Async supervisor interface. Format selection happens once before serving.
mod ticket;
pub(crate) use crate::mutation_ledger::ProcessFilter;
pub(crate) use ticket::{finish, Ticket};

use crate::config::RuntimeConfig;
use crate::failure::Result;
use crate::mutation_journal::{self as v1, Operation, Status};
use crate::mutation_ledger::Ledger;
use serde::de::DeserializeOwned;
use std::sync::Arc;

pub(crate) struct Store {
    config: RuntimeConfig,
    ledger: Option<Arc<Ledger>>,
}
#[derive(Default)]
pub(crate) struct Cursor {
    scope: u64,
    after: String,
    done: bool,
}
pub(crate) struct Pending {
    pub(crate) operation: Operation,
    pub(crate) ticket: Ticket,
}
#[derive(Debug)]
pub(crate) enum Begin<T> {
    New(Ticket),
    Started(Ticket),
    Completed(T),
    Failed(crate::failure::RuntimeError),
    Indeterminate(crate::failure::RuntimeError),
}

impl Store {
    pub(crate) fn new(config: RuntimeConfig, ledger: Option<Arc<Ledger>>) -> Self {
        Self { config, ledger }
    }
    pub(crate) async fn admit(&self, fence: u64, operations: &[Operation]) -> Result<()> {
        match &self.ledger {
            Some(ledger) => {
                crate::persistent_home::verify(&self.config)?;
                ledger.admit(fence, operations.to_vec()).await
            }
            None => match operations {
                [operation] => crate::mutation_authority::admit(&self.config, fence, operation),
                _ => crate::mutation_authority::admit_many(&self.config, fence, operations),
            },
        }
    }
    pub(crate) async fn admitted(&self, operation: &Operation) -> Result<bool> {
        match &self.ledger {
            Some(ledger) => ledger.admitted(operation.clone()).await,
            None => v1::admitted(&self.config, operation),
        }
    }
    pub(crate) async fn begin<T: DeserializeOwned>(
        &self,
        operation: &Operation,
    ) -> Result<Begin<T>> {
        let Some(ledger) = &self.ledger else {
            return v1::begin(&self.config, operation).map(|begin| match begin {
                v1::Begin::New(t) => Begin::New(Ticket::legacy(t)),
                v1::Begin::Started(t) => Begin::Started(Ticket::legacy(t)),
                v1::Begin::Completed(r) => Begin::Completed(r),
                v1::Begin::Failed(e) => Begin::Failed(e),
                v1::Begin::Indeterminate(e) => Begin::Indeterminate(e),
            });
        };
        let entry = ledger.begin(operation.clone()).await?;
        Ok(match entry.record.status() {
            Status::Started => {
                let ticket = Ticket::indexed(Arc::clone(ledger), entry.record);
                if entry.new {
                    Begin::New(ticket)
                } else {
                    Begin::Started(ticket)
                }
            }
            Status::Completed => Begin::Completed(entry.record.receipt()?),
            Status::Failed => Begin::Failed(entry.record.error()?),
            Status::Indeterminate => Begin::Indeterminate(entry.record.error()?),
        })
    }
    pub(crate) async fn recovery_fence(&self, operation: &Operation) -> Result<Option<u64>> {
        match &self.ledger {
            Some(ledger) => ledger.recovery_fence(operation.clone()).await,
            None => crate::mutation_authority::recovery_fence(&self.config, operation),
        }
    }
    pub(crate) async fn started(&self, cursor: &mut Cursor) -> Result<Vec<Pending>> {
        if cursor.done {
            return Ok(Vec::new());
        }
        let Some(ledger) = &self.ledger else {
            cursor.done = true;
            return v1::started(&self.config).map(|rows| {
                rows.into_iter()
                    .map(|p| Pending {
                        operation: p.operation,
                        ticket: Ticket::legacy(p.ticket),
                    })
                    .collect()
            });
        };
        loop {
            let rows = ledger
                .started_page(cursor.scope, cursor.after.clone())
                .await?;
            if let Some(last) = rows.last() {
                cursor.after = last.operation().operation_id;
                return Ok(rows
                    .into_iter()
                    .map(|r| Pending {
                        operation: r.operation(),
                        ticket: Ticket::indexed(Arc::clone(ledger), r),
                    })
                    .collect());
            }
            if cursor.scope == 0 && self.config.home_identity.is_some() {
                cursor.scope = self.config.generation;
                cursor.after.clear();
            } else {
                cursor.done = true;
                return Ok(Vec::new());
            }
        }
    }
    pub(crate) async fn active(&self, filter: ProcessFilter) -> Result<bool> {
        if let Some(ledger) = &self.ledger {
            return ledger.active(filter).await;
        }
        Ok(v1::active_agents(&self.config)?
            .iter()
            .any(|agent| match &filter {
                ProcessFilter::Any => true,
                ProcessFilter::Agent(id) => agent.agent_id == *id,
                ProcessFilter::Worktree(path) => {
                    std::path::Path::new(&agent.cwd) == std::path::Path::new(path)
                }
                ProcessFilter::Workspace(path) => {
                    std::path::Path::new(&agent.cwd).starts_with(path)
                }
            }))
    }
}
