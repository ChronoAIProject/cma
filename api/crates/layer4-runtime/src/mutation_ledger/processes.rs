use rusqlite::{params, Connection};

use super::{journal, storage_error};
use crate::config::RuntimeConfig;
use crate::failure::Result;
use crate::mutation_journal::{Record, Status, Subject};

pub(super) fn update(db: &Connection, config: &RuntimeConfig, record: &Record) -> Result<()> {
    let operation = record.operation();
    let scope = journal::scope(config, &operation);
    match &operation.subject {
        Subject::OpenAgent {
            agent_id,
            cwd,
            home,
        } if record.status() != Status::Failed => {
            // Completed cleanup alone removes the durable process projection.
            let closed: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM operations WHERE scope=? AND operation_id=? AND status=2)",
                params![scope.as_slice(),format!("agent:{agent_id}:close:v1")], |r|r.get(0)).map_err(|_|storage_error())?;
            if !closed {
                db.execute("INSERT INTO logical_processes VALUES (?,?,?,?) ON CONFLICT(scope,agent) DO NOTHING",
                    params![scope.as_slice(),agent_id,cwd,home]).map_err(|_| storage_error())?;
            }
        }
        Subject::OpenAgent { agent_id, .. } => {
            db.execute(
                "DELETE FROM logical_processes WHERE scope=? AND agent=?",
                params![scope.as_slice(), agent_id],
            )
            .map_err(|_| storage_error())?;
        }
        Subject::CloseAgent { agent_id } if record.status() == Status::Completed => {
            db.execute(
                "DELETE FROM logical_processes WHERE scope=? AND agent=?",
                params![scope.as_slice(), agent_id],
            )
            .map_err(|_| storage_error())?;
        }
        _ => {}
    }
    Ok(())
}
