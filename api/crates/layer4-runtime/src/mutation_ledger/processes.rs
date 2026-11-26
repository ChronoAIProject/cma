use layer3_adapter::AgentProcessAction::{Close, Open};
use rusqlite::{params, Connection, OptionalExtension};

use super::{invalid, journal, storage_error};
use crate::config::RuntimeConfig;
use crate::failure::Result;
use crate::mutation_journal::{Record, Status, Subject};

pub(super) fn update(db: &Connection, config: &RuntimeConfig, record: &Record) -> Result<()> {
    let operation = record.operation();
    let scope = journal::scope(config, &operation);
    let agent = match &operation.subject {
        Subject::OpenAgent { agent_id, .. } => agent_id,
        Subject::CloseAgent { agent_id } if record.status() == Status::Completed => agent_id,
        _ => return Ok(()),
    };
    let change = if let Some(process) = projected_open(db, config, &scope, agent)? {
        let inserted = db
            .execute(
                "INSERT OR IGNORE INTO logical_processes VALUES (?,?,?,?)",
                params![scope.as_slice(), agent, process.cwd, process.home],
            )
            .map_err(|_| storage_error())?;
        if inserted == 0 {
            db.execute(
                "UPDATE logical_processes SET cwd=?,home=? WHERE scope=? AND agent=? AND (cwd<>? OR home<>?)",
                params![process.cwd, process.home, scope.as_slice(), agent, process.cwd, process.home],
            ).map_err(|_| storage_error())?;
        }
        inserted as i64
    } else {
        -(db.execute(
            "DELETE FROM logical_processes WHERE scope=? AND agent=?",
            params![scope.as_slice(), agent],
        )
        .map_err(|_| storage_error())? as i64)
    };
    if change < 0 {
        db.execute(
            "UPDATE scope_budget SET processes=processes-1 WHERE scope=?",
            [scope.as_slice()],
        )
        .map_err(|_| storage_error())?;
    } else if change > 0 {
        db.execute("INSERT INTO scope_budget VALUES (?,0,?) ON CONFLICT(scope) DO UPDATE SET processes=processes+excluded.processes",
            params![scope.as_slice(),change]).map_err(|_|storage_error())?;
    }
    Ok(())
}

pub(super) struct ProcessOpen {
    pub(super) id: String,
    pub(super) cwd: String,
    pub(super) home: String,
}

/// Import can encounter a successor before the predecessor's Close. Project
/// the newest unclosed Open, then verify the complete receipt pairs at the end.
pub(super) fn projected_open(
    db: &Connection,
    config: &RuntimeConfig,
    scope: &[u8; 8],
    agent: &str,
) -> Result<Option<ProcessOpen>> {
    let open_prefix = Open.prefix(agent);
    let close_prefix = Close.prefix(agent);
    let end = format!("{};", open_prefix.trim_end_matches(':'));
    let opening: Option<Vec<u8>> = db
        .query_row(
            "SELECT opening.record FROM operations AS opening
         WHERE opening.scope=?1 AND opening.operation_id>=?2 AND opening.operation_id<?3
           AND opening.status IN (1,2,4) AND NOT EXISTS(
             SELECT 1 FROM operations AS closing WHERE closing.scope=?1 AND closing.status=2
             AND closing.operation_id=?4||substr(opening.operation_id,length(?2)+1)
           )
         ORDER BY length(opening.operation_id) DESC, opening.operation_id DESC LIMIT 1",
            params![scope.as_slice(), open_prefix, end, close_prefix],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| storage_error())?;
    let Some(opening) = opening else {
        return Ok(None);
    };
    let operation = Record::decode(&opening, config)?.operation();
    match operation.subject {
        Subject::OpenAgent {
            agent_id,
            cwd,
            home,
        } if agent_id == agent => Ok(Some(ProcessOpen {
            id: operation.operation_id,
            cwd,
            home,
        })),
        _ => Err(invalid()),
    }
}
