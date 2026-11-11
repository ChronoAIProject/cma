//! Reconcile the complete inventory without retaining its rows in memory.
//! This proves internal accounting only, never retirement eligibility.
use rusqlite::{Connection, OptionalExtension};

use super::{Auxiliary, Context, Kind, SourceRow};
use crate::failure::Result;
use crate::mutation_ledger::{budget, invalid, storage_error};

#[derive(Clone, Default)]
pub(super) struct Audit {
    operations: i64,
    authority: i64,
    terminal: i64,
    reservations: i64,
    normal_pending: i64,
    processes: i64,
    reported_pending: i64,
    reported_processes: i64,
    scope: Option<ScopeCount>,
}

#[derive(Clone)]
struct ScopeCount {
    key: Vec<u8>,
    value: i64,
}

impl Audit {
    pub(super) fn observe(&mut self, db: &Connection, row: &SourceRow) -> Result<()> {
        match row {
            SourceRow::Operation {
                scope,
                status,
                normal,
                ..
            } => {
                add(&mut self.operations, 1)?;
                let pending = i64::from(*status < 2);
                add(&mut self.terminal, 1 - pending)?;
                add(&mut self.reservations, pending)?;
                let normal_pending = pending * i64::from(*normal);
                add(&mut self.normal_pending, normal_pending)?;
                self.scope(db, scope, normal_pending, Kind::Operations)?;
            }
            SourceRow::Authority { .. } => add(&mut self.authority, 1)?,
            SourceRow::Auxiliary(Auxiliary::Cleanup { available, .. }) => {
                add(&mut self.reservations, i64::from(*available))?;
            }
            SourceRow::Auxiliary(Auxiliary::Scope {
                pending, processes, ..
            }) => {
                add(&mut self.reported_pending, *pending)?;
                add(&mut self.reported_processes, *processes)?;
            }
            SourceRow::Auxiliary(Auxiliary::Process { scope, .. }) => {
                add(&mut self.processes, 1)?;
                self.scope(db, scope, 1, Kind::Processes)?;
            }
        }
        Ok(())
    }

    fn scope(&mut self, db: &Connection, key: &[u8], value: i64, kind: Kind) -> Result<()> {
        if self.scope.as_ref().is_some_and(|scope| scope.key != key) {
            self.flush(db, kind)?;
        }
        let scope = self.scope.get_or_insert_with(|| ScopeCount {
            key: key.into(),
            value: 0,
        });
        add(&mut scope.value, value)
    }

    // Each source table is in primary-key scope order. One point read when a
    // scope ends proves its exact total; pages carry only the current subtotal.
    // Global totals also detect nonzero scope rows with no matching sources.
    fn flush(&mut self, db: &Connection, kind: Kind) -> Result<()> {
        let Some(scope) = self.scope.take() else {
            return Ok(());
        };
        let query = match kind {
            Kind::Operations => "SELECT pending FROM scope_budget WHERE scope=?",
            Kind::Processes => "SELECT processes FROM scope_budget WHERE scope=?",
            _ => return Err(invalid()),
        };
        let reported: Option<i64> = db
            .query_row(query, [scope.key], |r| r.get(0))
            .optional()
            .map_err(|_| storage_error())?;
        if reported.unwrap_or(0) != scope.value {
            return Err(invalid());
        }
        Ok(())
    }

    pub(super) fn end_table(&mut self, db: &Connection, kind: Kind) -> Result<()> {
        if matches!(kind, Kind::Operations | Kind::Processes) {
            self.flush(db, kind)?;
        }
        Ok(())
    }

    pub(super) fn finish(&self, context: &Context) -> Result<()> {
        let reserved = self
            .reservations
            .checked_mul(budget::OUTCOME)
            .ok_or_else(invalid)?;
        if self.scope.is_some()
            || context.accounting[1..] != [reserved, self.operations, self.authority, self.terminal]
            || self.reported_pending != self.normal_pending
            || self.reported_processes != self.processes
        {
            return Err(invalid());
        }
        Ok(())
    }
}

fn add(total: &mut i64, value: i64) -> Result<()> {
    if value < 0 {
        return Err(invalid());
    }
    *total = total.checked_add(value).ok_or_else(invalid)?;
    Ok(())
}
