//! Read-only point/range queries: terminal history is never recovery work.
use std::sync::Arc;

use rusqlite::{params, Connection};
use serde::Serialize;

use super::{authority, invalid, journal, storage_error, Ledger};
use crate::failure::Result;
use crate::mutation_authority as v1;
use crate::mutation_journal::{process_history, Operation, Record, Status, Subject};

pub(super) const STARTED_PAGE: &str = "SELECT identity,record FROM operations INDEXED BY outstanding WHERE scope=? AND status IN (0,1) AND status=1 AND operation_id>? ORDER BY operation_id LIMIT 64";

pub(crate) enum ProcessFilter {
    Any,
    Agent(String),
    Worktree(String),
    Workspace(String),
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub(crate) struct Snapshot {
    pub(crate) revision: i64,
    pub(crate) operations: i64,
    pub(crate) terminal: i64,
    pub(crate) authority: i64,
    pub(crate) reserved_bytes: i64,
    pub(crate) pending: i64,
    pub(crate) processes: i64,
    pub(crate) allocated_bytes: i64,
    pub(crate) maximum_bytes: i64,
    pub(crate) emergency_remaining_bytes: u64,
}

impl Ledger {
    pub(crate) async fn admitted(&self, operation: Operation) -> Result<bool> {
        Ok(self.read_status(operation).await?.is_some())
    }

    pub(crate) async fn completed(&self, operation: Operation) -> Result<bool> {
        Ok(self.read_status(operation).await? == Some(Status::Completed))
    }

    pub(crate) async fn find_operation(
        &self,
        operation_id: String,
        scope_subject: Subject,
    ) -> Result<Option<Record>> {
        self.verify()?;
        let config = Arc::clone(&self.config);
        self.storage
            .call(move |db| {
                let scope = crate::mutation_scope::generation(&config, &scope_subject)
                    .unwrap_or(0)
                    .to_be_bytes();
                journal::load_id(db, &config, &scope, &operation_id)
            })
            .await
    }

    async fn read_status(&self, operation: Operation) -> Result<Option<Status>> {
        self.verify()?;
        crate::mutation_journal::validate_operation(&self.config, &operation)?;
        let config = Arc::clone(&self.config);
        self.storage
            .call(move |db| {
                let record =
                    journal::load(db, &config, &operation)?.and_then(|stored| stored.record);
                if record.is_some() {
                    let scope = journal::scope(&config, &operation);
                    process_history::validate(&operation, |id| {
                        journal::load_id(db, &config, &scope, id)
                    })?;
                }
                Ok(record.map(|record| record.status()))
            })
            .await
    }

    pub(crate) async fn recovery_fence(&self, operation: Operation) -> Result<Option<u64>> {
        self.verify()?;
        crate::mutation_journal::validate_operation(&self.config, &operation)?;
        let config = Arc::clone(&self.config);
        self.storage
            .call(move |db| {
                if let Subject::InstallAgentSkill { request } = &operation.subject {
                    let parent = v1::ResourceKey::AgentHome {
                        agent_id: request.agent_id.as_str().into(),
                    };
                    if v1::require_skill_parent(
                        &authority::load(db, &config, &parent, &[0; 8])?,
                        &operation,
                    )
                    .is_err()
                    {
                        return Ok(None);
                    }
                }
                let records = authority::load(
                    db,
                    &config,
                    &v1::resource_operation(&operation).0,
                    &journal::scope(&config, &operation),
                )?;
                Ok(v1::current(records.iter())
                    .filter(|r| r.operation() == operation)
                    .map(|r| r.authority_fence))
            })
            .await
    }

    pub(crate) async fn has_process_writer(&self, key: v1::ResourceKey) -> Result<bool> {
        self.verify()?;
        let config = Arc::clone(&self.config);
        self.storage
            .call(move |db| {
                let scope = config.generation.to_be_bytes();
                Ok(!authority::load(db, &config, &key, &scope)?.is_empty())
            })
            .await
    }

    pub(crate) async fn started_page(&self, generation: u64, after: String) -> Result<Vec<Record>> {
        self.verify()?;
        if (generation != 0
            && (self.config.home_identity.is_none() || generation != self.config.generation))
            || after.len() > 256
        {
            return Err(invalid());
        }
        let config = Arc::clone(&self.config);
        self.storage
            .call(move |db| {
                let mut statement = db.prepare(STARTED_PAGE).map_err(|_| storage_error())?;
                let mut rows = statement
                    .query(params![generation.to_be_bytes().as_slice(), after])
                    .map_err(|_| storage_error())?;
                let mut records = Vec::new();
                let mut bytes = 0;
                while let Some(row) = rows.next().map_err(|_| storage_error())? {
                    let identity: Vec<u8> = row.get(0).map_err(|_| invalid())?;
                    let payload: Vec<u8> = row.get(1).map_err(|_| invalid())?;
                    if identity.len() > 65536 || payload.len() > 1024 * 1024 {
                        return Err(invalid());
                    }
                    bytes += identity.len() + payload.len();
                    if bytes > 1024 * 1024 {
                        if records.is_empty() {
                            return Err(invalid());
                        }
                        break;
                    }
                    let operation: Operation =
                        serde_json::from_slice(&identity).map_err(|_| invalid())?;
                    if journal::scope(&config, &operation) != generation.to_be_bytes() {
                        return Err(invalid());
                    }
                    let record = Record::decode(&payload, &config)?;
                    record.exact(&operation)?;
                    if record.status() != crate::mutation_journal::Status::Started {
                        return Err(invalid());
                    }
                    let scope = generation.to_be_bytes();
                    process_history::validate(&operation, |id| {
                        journal::load_id(db, &config, &scope, id)
                    })?;
                    records.push(record);
                }
                Ok(records)
            })
            .await
    }

    pub(crate) async fn active(&self, filter: ProcessFilter) -> Result<bool> {
        self.verify()?;
        let scope = self
            .config
            .home_identity
            .as_ref()
            .map_or(0, |_| self.config.generation)
            .to_be_bytes();
        self.storage.call(move |db| {
            let result=match filter {
                ProcessFilter::Any=>db.query_row("SELECT EXISTS(SELECT 1 FROM logical_processes WHERE scope=?)",[scope.as_slice()],|r|r.get(0)),
                ProcessFilter::Agent(agent)=>db.query_row("SELECT EXISTS(SELECT 1 FROM logical_processes WHERE scope=? AND agent=?)",params![scope.as_slice(),bounded(agent)?],|r|r.get(0)),
                ProcessFilter::Worktree(cwd)=>db.query_row("SELECT EXISTS(SELECT 1 FROM logical_processes WHERE scope=? AND cwd=?)",params![scope.as_slice(),bounded(cwd)?],|r|r.get(0)),
                ProcessFilter::Workspace(root)=>{
                    let root=bounded(root)?;
                    // '/' is the path separator; '0' is its exclusive lexical successor.
                    let start=format!("{root}/");
                    let end=format!("{root}0");
                    db.query_row("SELECT EXISTS(SELECT 1 FROM logical_processes WHERE scope=? AND (cwd=? OR (cwd>=? AND cwd<?)))",params![scope.as_slice(),root,start,end],|r|r.get(0))
                }
            };
            result.map_err(|_|storage_error())
        }).await
    }

    pub(crate) async fn snapshot(&self) -> Result<Snapshot> {
        self.verify()?;
        let generation = self.config.generation.to_be_bytes();
        let directory = self.directory.clone();
        self.storage.call(move |db| {
            let tx=db.unchecked_transaction().map_err(|_|storage_error())?;
            let mut snapshot=tx.query_row("SELECT revision,operations,terminal,authority,reserved FROM accounting WHERE singleton=1",[],|r|Ok(Snapshot {
                revision:r.get(0)?,operations:r.get(1)?,terminal:r.get(2)?,authority:r.get(3)?,reserved_bytes:r.get(4)?,pending:0,processes:0,allocated_bytes:0,maximum_bytes:0,emergency_remaining_bytes:0,
            })).map_err(|_|storage_error())?;
            (snapshot.pending,snapshot.processes)=tx.query_row("SELECT coalesce(sum(pending),0),coalesce(sum(processes),0) FROM scope_budget WHERE scope IN (zeroblob(8),?)",[generation.as_slice()],|r|Ok((r.get(0)?,r.get(1)?))).map_err(|_|storage_error())?;
            snapshot.emergency_remaining_bytes=crate::ledger_storage::reserve::remaining(&directory)?;
            snapshot.allocated_bytes=pragma(&tx,"page_count")?*4096;
            snapshot.maximum_bytes=pragma(&tx,"max_page_count")?*4096;
            tx.commit().map_err(|_|storage_error())?;
            Ok(snapshot)
        }).await
    }
}

fn bounded(text: String) -> Result<String> {
    if text.len() > 65536 {
        Err(invalid())
    } else {
        Ok(text)
    }
}
fn pragma(db: &Connection, name: &str) -> Result<i64> {
    db.pragma_query_value(None, name, |r| r.get(0))
        .map_err(|_| storage_error())
}
