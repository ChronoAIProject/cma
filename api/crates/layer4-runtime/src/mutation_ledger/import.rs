//! Detached import ownership has no admission, serving or recovery interface.
#[path = "import_cleanup.rs"]
mod cleanup;

use std::path::PathBuf;

use rusqlite::params;

use super::{authority, budget, fence, invalid, journal, processes, schema, storage_error};
use crate::config::RuntimeConfig;
use crate::failure::Result;
use crate::ledger_storage::Storage;
use crate::mutation_authority::Record as Authority;
use crate::mutation_journal::{process_history, Record, Status, Subject};

pub(crate) struct HeldImport {
    storage: Storage,
    config: RuntimeConfig,
    directory: PathBuf,
    anchor: Option<std::sync::Arc<std::fs::File>>,
}

impl HeldImport {
    #[cfg(test)]
    pub(crate) async fn create(path: PathBuf, config: RuntimeConfig, pages: u32) -> Result<Self> {
        let storage = Storage::open(path.clone(), true, pages).await?;
        let expected = config.clone();
        if let Err(error) = storage
            .call(move |db| schema::initialize_held(db, &expected))
            .await
        {
            return cleanup::finish(Err(error), storage.close().await, "initialize");
        }
        Ok(Self {
            storage,
            config,
            directory: path,
            anchor: None,
        })
    }

    pub(crate) async fn create_anchored(
        directory: std::sync::Arc<std::fs::File>,
        config: RuntimeConfig,
        pages: u32,
    ) -> Result<Self> {
        let storage = Storage::open_anchored(directory.clone(), pages).await?;
        let expected = config.clone();
        let storage = storage
            .validate(move |db| schema::initialize_held(db, &expected))
            .await?;
        Ok(Self {
            storage,
            config,
            directory: PathBuf::new(),
            anchor: Some(directory),
        })
    }

    pub(crate) async fn close(self) -> Result<()> {
        self.storage.close().await
    }

    pub(crate) async fn abort<T>(self, error: crate::failure::RuntimeError) -> Result<T> {
        cleanup::finish(Err(error), self.close().await, "stage")
    }

    fn scoped(&self, scope: u64) -> Result<RuntimeConfig> {
        if scope > self.config.generation {
            return Err(invalid());
        }
        let mut config = self.config.clone();
        if scope != 0 {
            config.generation = scope;
        }
        Ok(config)
    }

    pub(crate) async fn authority(
        &self,
        scope: u64,
        key: String,
        rows: [Option<Vec<u8>>; 3],
    ) -> Result<()> {
        let config = self.scoped(scope)?;
        self.storage.call(move |db| {
            let mut records = Vec::new();
            for bytes in rows.iter().flatten() {
                if bytes.len() > crate::mutation_authority::MAX_AUTHORITY_BYTES as usize {
                    return Err(invalid());
                }
                records.push(serde_json::from_slice::<Authority>(bytes).map_err(|_| invalid())?);
            }
            let high = records.iter().map(|r| r.authority_fence).max().ok_or_else(invalid)?;
            let resource = records[0].resource.clone();
            if authority::key(&resource)? != key {
                return Err(invalid());
            }
            let tx = db.transaction().map_err(|_| storage_error())?;
            tx.execute("INSERT INTO authority VALUES (?,?,?,?,?,?)",
                params![scope.to_be_bytes().as_slice(),key,fence::encode(high).as_slice(),rows[0],rows[1],rows[2]])
                .map_err(|_| storage_error())?;
            // Reuse the serving reader's exact scoped identity, disposition,
            // history and unsigned high-water checks on the imported bytes.
            authority::load(&tx, &config, &resource, &scope.to_be_bytes())?;
            for record in &records {
                journal::reserve(&tx, &config, &record.operation())?;
            }
            tx.execute("UPDATE accounting SET authority=authority+1 WHERE singleton=1", [])
                .map_err(|_| storage_error())?;
            budget::revision(&tx)?;
            tx.commit().map_err(|_| storage_error())?;
            let stored: [Option<Vec<u8>>; 3] = db.query_row(
                "SELECT present,first_absent,last_absent FROM authority WHERE scope=? AND resource=?",
                params![scope.to_be_bytes().as_slice(),key], |r| Ok([r.get(0)?,r.get(1)?,r.get(2)?]))
                .map_err(|_| storage_error())?;
            if stored != rows { return Err(invalid()); }
            Ok(())
        }).await
    }

    pub(crate) async fn journal(&self, scope: u64, bytes: Vec<u8>) -> Result<()> {
        let config = self.scoped(scope)?;
        self.storage.call(move |db| {
            let record = Record::decode(&bytes, &config)?;
            let operation = record.operation();
            if journal::scope(&config, &operation) != scope.to_be_bytes() {
                return Err(invalid());
            }
            let status = match record.status() {
                Status::Started => 1,
                Status::Completed => 2,
                Status::Failed => 3,
                Status::Indeterminate => 4,
            };
            let tx = db.transaction().map_err(|_| storage_error())?;
            journal::reserve(&tx, &config, &operation)?;
            let normal: bool = tx.query_row("SELECT normal FROM operations WHERE scope=? AND operation_id=?",
                params![scope.to_be_bytes().as_slice(),operation.operation_id], |r| r.get(0))
                .map_err(|_| storage_error())?;
            if tx.execute("UPDATE operations SET status=?,record=? WHERE scope=? AND operation_id=? AND status=0",
                params![status,bytes,scope.to_be_bytes().as_slice(),operation.operation_id])
                .map_err(|_| storage_error())? != 1 { return Err(invalid()); }
            processes::update(&tx, &config, &record)?;
            if status > 1 { budget::finish(&tx, &scope.to_be_bytes(), normal)?; }
            budget::revision(&tx)?;
            tx.commit().map_err(|_| storage_error())?;
            let stored: Vec<u8> = db.query_row("SELECT record FROM operations WHERE scope=? AND operation_id=?",
                params![scope.to_be_bytes().as_slice(),operation.operation_id], |r| r.get(0))
                .map_err(|_| storage_error())?;
            if stored != bytes { return Err(invalid()); }
            Ok(())
        }).await
    }

    /// Recheck process lineage after all Opens and Closes have been imported.
    pub(crate) async fn verify_process(&self, scope: u64, bytes: Vec<u8>) -> Result<bool> {
        let config = self.scoped(scope)?;
        self.storage.call(move |db| {
            use rusqlite::OptionalExtension;
            let record = Record::decode(&bytes, &config)?;
            let operation = record.operation();
            let scope = scope.to_be_bytes();
            if journal::scope(&config, &operation) != scope {
                return Err(invalid());
            }
            process_history::validate(&operation, |id| journal::load_id(db, &config, &scope, id))?;
            let Subject::OpenAgent { agent_id, .. } = operation.subject else {
                return Ok(false);
            };
            use layer3_adapter::AgentProcessAction::{Close, Open};
            let incarnation = Open.incarnation(&agent_id, &operation.operation_id).ok_or_else(invalid)?;
            let closed: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM operations WHERE scope=? AND operation_id=? AND status=2)",
                params![scope.as_slice(),Close.operation_id(&agent_id, incarnation)], |r| r.get(0))
                .map_err(|_| storage_error())?;
            let expected_live = record.status() != Status::Failed && !closed;
            let projected = processes::projected_open(db, &config, &scope, &agent_id)?;
            if expected_live && projected.as_ref().is_none_or(|open| open.id != operation.operation_id) {
                return Err(invalid());
            }
            let actual: Option<(String,String)> = db.query_row("SELECT cwd,home FROM logical_processes WHERE scope=? AND agent=?",
                params![scope.as_slice(),agent_id], |r| Ok((r.get(0)?,r.get(1)?)))
                .optional().map_err(|_| storage_error())?;
            if actual != projected.map(|open| (open.cwd, open.home)) { return Err(invalid()); }
            Ok(expected_live)
        }).await
    }

    pub(crate) async fn finish(self, expected_processes: u64) -> Result<serde_json::Value> {
        let config = self.config;
        let directory = self.directory;
        let anchor = self.anchor;
        let result = self.storage.call(move |db| {
            match &anchor {
                Some(directory) => crate::ledger_storage::anchored::require(directory)?,
                None => crate::ledger_storage::reserve::require(&directory)?,
            }
            let (admission,generation): (String,Vec<u8>) = db.query_row("SELECT admission,generation FROM metadata WHERE singleton=1", [], |r| Ok((r.get(0)?,r.get(1)?)))
                .map_err(|_| invalid())?;
            if admission != "import_held" || fence::decode(&generation)? != config.generation { return Err(invalid()); }
            // This read performs complete schema/identity validation and must
            // refuse before any serving generation update or recovery.
            match schema::open(db, &config, false) {
                Err(error) if error.code == "mutation_ledger_import_held" => {}
                _ => return Err(invalid()),
            }
            let integrity: String = db.query_row("PRAGMA quick_check(1)", [], |r| r.get(0)).map_err(|_| invalid())?;
            if integrity != "ok" { return Err(invalid()); }
            verify_counters(db, expected_processes)?;
            budget::check(db, config.generation)?;
            let (operations, terminal, authority, reserved): (i64,i64,i64,i64) = db.query_row(
                "SELECT operations,terminal,authority,reserved FROM accounting WHERE singleton=1", [],
                |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).map_err(|_| invalid())?;
            Ok(serde_json::json!({"admission":"import_held", "operations":operations,
                "terminal":terminal,"authority_resources":authority,"reserved_bytes":reserved,
                "logical_processes":expected_processes,"capacity":"covered"}))
        }).await;
        cleanup::finish(result, self.storage.close().await, "verify")
    }
}

#[cfg(test)]
#[path = "import_tests.rs"]
mod tests;

fn verify_counters(db: &rusqlite::Connection, expected_processes: u64) -> Result<()> {
    let expected_processes = i64::try_from(expected_processes).map_err(|_| invalid())?;
    let valid: bool = db.query_row(
        "SELECT operations=(SELECT count(*) FROM operations)
         AND authority=(SELECT count(*) FROM authority)
         AND terminal=(SELECT count(*) FROM operations WHERE status>1)
         AND reserved=((SELECT count(*) FROM operations WHERE status IN (0,1))+
             (SELECT coalesce(sum(available),0) FROM cleanup_budget))*?
         AND (SELECT count(*) FROM logical_processes)=?
         AND (SELECT coalesce(sum(processes),0) FROM scope_budget)=(SELECT count(*) FROM logical_processes)
         AND (SELECT coalesce(sum(pending),0) FROM scope_budget)=(SELECT count(*) FROM operations WHERE status IN (0,1) AND normal=1)
         AND NOT EXISTS(SELECT 1 FROM scope_budget b WHERE
             pending!=(SELECT count(*) FROM operations o WHERE o.scope=b.scope AND o.status IN (0,1) AND o.normal=1)
             OR processes!=(SELECT count(*) FROM logical_processes p WHERE p.scope=b.scope))
         FROM accounting WHERE singleton=1", params![budget::OUTCOME,expected_processes], |r| r.get(0))
        .map_err(|_| invalid())?;
    if !valid {
        return Err(invalid());
    }
    Ok(())
}
