//! Compare a bounded archived selection with the exact current source columns.
//! These observations are not a complete-group proof or a mutation permit.
use rusqlite::{params, Connection};

use super::{invalid, validate, Auxiliary, Context, Ledger, Result, SourceRow, BYTES, ROWS};

impl Ledger {
    /// Every page must use the original full inventory context. A changed
    /// store, generation, revision or page geometry refuses the comparison.
    /// Deactivation must repeat these checks inside its own source transaction
    /// under an owned writer hold; a successful read does not reserve the rows.
    pub(crate) async fn verify_retirement_sources(
        &self,
        expected: Context,
        rows: Vec<SourceRow>,
    ) -> Result<()> {
        self.verify()?;
        if rows.is_empty() || rows.len() > ROWS {
            return Err(invalid());
        }
        let bytes = rows.iter().try_fold(expected.bytes(), |total, row| {
            total.checked_add(row.bytes()).ok_or_else(invalid)
        })?;
        if bytes > BYTES {
            return Err(invalid());
        }
        let config = self.config.clone();
        self.storage
            .call(move |db| {
                // Inspection failures must not enter Storage::call's mutating
                // emergency-reserve release path.
                let tx = db.transaction().map_err(|_| invalid())?;
                if Context::read(&tx, &config)? != expected {
                    return Err(invalid());
                }
                let mut previous = None;
                for row in rows {
                    validate(&config, &row)?;
                    let ordering = row.ordering();
                    if previous.as_ref().is_some_and(|prior| prior >= &ordering)
                        || !matches(&tx, &row)?
                    {
                        return Err(invalid());
                    }
                    previous = Some(ordering);
                }
                tx.commit().map_err(|_| invalid())
            })
            .await
    }
}

// IS preserves NULL distinctions; BLOB comparison preserves exact raw bytes.
// No row contents are loaded or logged while comparing the active store.
pub(super) fn matches(db: &Connection, row: &SourceRow) -> Result<bool> {
    let result = match row {
        SourceRow::Operation { scope, id, identity, status, normal, record } => db.query_row(
            "SELECT EXISTS(SELECT 1 FROM operations WHERE scope=? AND operation_id=?
             AND identity=? AND status=? AND normal=? AND record IS ?)",
            params![scope, id, identity, status, normal, record], |r| r.get(0)),
        SourceRow::Authority { scope, resource, high, records } => db.query_row(
            "SELECT EXISTS(SELECT 1 FROM authority WHERE scope=? AND resource=?
             AND high_water=? AND present IS ? AND first_absent IS ? AND last_absent IS ?)",
            params![scope, resource, high, records[0], records[1], records[2]], |r| r.get(0)),
        SourceRow::Auxiliary(Auxiliary::Cleanup { scope, resource, available }) => db.query_row(
            "SELECT EXISTS(SELECT 1 FROM cleanup_budget WHERE scope=? AND resource=? AND available=?)",
            params![scope, resource, available], |r| r.get(0)),
        SourceRow::Auxiliary(Auxiliary::Scope { scope, pending, processes }) => db.query_row(
            "SELECT EXISTS(SELECT 1 FROM scope_budget WHERE scope=? AND pending=? AND processes=?)",
            params![scope, pending, processes], |r| r.get(0)),
        SourceRow::Auxiliary(Auxiliary::Process { scope, agent, cwd, home }) => db.query_row(
            "SELECT EXISTS(SELECT 1 FROM logical_processes WHERE scope=? AND agent=? AND cwd=? AND home=?)",
            params![scope, agent, cwd, home], |r| r.get(0)),
    };
    result.map_err(|_| invalid())
}

#[cfg(test)]
#[path = "retirement_source_tests.rs"]
mod tests;
