//! Async supervisor interface. Format selection happens once before serving.
mod ticket;
pub(crate) use crate::mutation_ledger::ProcessFilter;
pub(crate) use ticket::{finish, Ticket};

use crate::config::RuntimeConfig;
use crate::failure::Result;
use crate::mutation_journal::{self as v1, Operation, Record, Status};
use crate::mutation_ledger::Ledger;
use serde::de::DeserializeOwned;
use std::sync::Arc;

pub(crate) struct Store {
    config: RuntimeConfig,
    ledger: Option<Arc<Ledger>>,
    retirement: Option<crate::retirement_format::Reader>,
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
        Self {
            config,
            ledger,
            retirement: None,
        }
    }
    pub(crate) fn with_retirement(
        mut self,
        retirement: Option<crate::retirement_format::Reader>,
    ) -> Self {
        self.retirement = retirement;
        self
    }
    pub(crate) async fn observation(&self) -> Result<layer3_adapter::RuntimeLedgerStatus> {
        self.verify()?;
        match &self.ledger {
            Some(ledger) => ledger.observation().await,
            None => Ok(layer3_adapter::RuntimeLedgerStatus::LegacyMigrationRequired),
        }
    }
    pub(crate) fn verify(&self) -> Result<()> {
        match &self.retirement {
            Some(retirement) => retirement.verify()?,
            None => crate::retirement_format::require_absent(&self.config)?,
        }
        if let Some(ledger) = &self.ledger {
            ledger.verify()?;
        }
        Ok(())
    }

    /// Check after acquiring the operation gate and before any admitted effect.
    /// Startup's successful check cannot authorize work after a later intent.
    pub(crate) async fn require_serving(&self) -> Result<()> {
        self.verify()?;
        if let Some(retirement) = &self.retirement {
            retirement.require_ready().await?;
        }
        Ok(())
    }
    pub(crate) async fn admit(&self, fence: u64, operations: &[Operation]) -> Result<()> {
        self.authorize_retirement(operations).await?;
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
        self.authorize_retirement(std::slice::from_ref(operation))
            .await?;
        match &self.ledger {
            Some(ledger) => ledger.admitted(operation.clone()).await,
            None => v1::admitted(&self.config, operation),
        }
    }

    pub(crate) async fn completed(&self, operation: &Operation) -> Result<bool> {
        self.authorize_retirement(std::slice::from_ref(operation))
            .await?;
        match &self.ledger {
            Some(ledger) => ledger.completed(operation.clone()).await,
            None => v1::completed(&self.config, operation),
        }
    }

    pub(crate) async fn find_operation(
        &self,
        operation_id: String,
        operation: &Operation,
    ) -> Result<Option<Record>> {
        self.authorize_retirement(std::slice::from_ref(operation))
            .await?;
        match &self.ledger {
            Some(ledger) => {
                ledger
                    .find_operation(operation_id, operation.subject.clone())
                    .await
            }
            None => v1::find_operation(&self.config, &operation_id, &operation.subject),
        }
    }
    pub(crate) async fn find_process_proof_record(
        &self,
        operation_id: String,
        scope: &Operation,
    ) -> Result<Option<Record>> {
        self.authorize_retirement(std::slice::from_ref(scope))
            .await?;
        match &self.ledger {
            Some(ledger) => {
                ledger
                    .find_operation(operation_id, scope.subject.clone())
                    .await
            }
            None => v1::find_proof_record(&self.config, &operation_id, &scope.subject),
        }
    }

    pub(crate) async fn has_process_writer(
        &self,
        agent_id: &str,
        incarnation: u64,
    ) -> Result<bool> {
        use crate::mutation_authority::{self, ResourceKey};
        let key = ResourceKey::AgentProcess {
            agent_id: agent_id.to_owned(),
            incarnation: Some(incarnation),
        };
        match &self.ledger {
            Some(ledger) => ledger.has_process_writer(key).await,
            None => mutation_authority::has_process_writer(&self.config, &key),
        }
    }

    pub(crate) async fn begin<T: DeserializeOwned>(
        &self,
        operation: &Operation,
    ) -> Result<Begin<T>> {
        self.authorize_retirement(std::slice::from_ref(operation))
            .await?;
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
        self.authorize_retirement(std::slice::from_ref(operation))
            .await?;
        match &self.ledger {
            Some(ledger) => ledger.recovery_fence(operation.clone()).await,
            None => crate::mutation_authority::recovery_fence(&self.config, operation),
        }
    }
    pub(crate) async fn started(&self, cursor: &mut Cursor) -> Result<Vec<Pending>> {
        self.require_serving().await?;
        if cursor.done {
            return Ok(Vec::new());
        }
        let Some(ledger) = &self.ledger else {
            cursor.done = true;
            let began = std::time::Instant::now();
            return v1::started(&self.config).map(|rows| {
                if self.config.legacy_journal_recovery_enabled {
                    tracing::info!(
                        pending_operations = rows.len(),
                        elapsed_ms = began.elapsed().as_millis() as u64,
                        "runtime legacy ledger recovery scan completed"
                    );
                }
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
        self.verify()?;
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

    async fn authorize_retirement(&self, operations: &[Operation]) -> Result<()> {
        self.verify()?;
        if let Some(retirement) = &self.retirement {
            retirement.authorize(operations).await?;
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) async fn close_retirement(self) -> Result<()> {
        if let Some(retirement) = self.retirement {
            retirement.close().await?;
        }
        Ok(())
    }
}
