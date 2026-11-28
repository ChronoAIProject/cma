//! Agent command MongoDB records and caller-scoped persistence.

use layer2_core::sandbox::agent_io::CommandLedgerError;

pub mod audit;
pub mod control;
pub mod lane_records;
pub mod records;
pub mod transaction;

const DOCUMENT_SCHEMA_VERSION: i64 = 1;

#[derive(Debug)]
pub enum ApplyError {
    Domain(CommandLedgerError),
    Driver(mongodb::error::Error),
}

impl From<CommandLedgerError> for ApplyError {
    fn from(value: CommandLedgerError) -> Self {
        Self::Domain(value)
    }
}

impl From<mongodb::error::Error> for ApplyError {
    fn from(value: mongodb::error::Error) -> Self {
        Self::Driver(value)
    }
}
