use layer2_core::sandbox::agent_io::{CommandLane, CommandLedgerError};
use mongodb::bson::{doc, Bson, Document};

use cma_am_mongo_primitives::{record_bson, require_version, signed};

use crate::DOCUMENT_SCHEMA_VERSION;

pub fn read(document: &Document, agent_id: &str) -> Result<CommandLane, CommandLedgerError> {
    require_version(document, DOCUMENT_SCHEMA_VERSION).map_err(|_| CommandLedgerError::Corrupt)?;
    if document.get_str("_id").ok() != Some(agent_id) {
        return Err(CommandLedgerError::Corrupt);
    }
    let last_order = document
        .get_i64("last_order")
        .map_err(|_| CommandLedgerError::Corrupt)?;
    let next_fence = match document.get("next_fence") {
        None => 1,
        Some(Bson::Int64(value)) => *value,
        _ => return Err(CommandLedgerError::Corrupt),
    };
    if last_order < 1 || next_fence < 1 {
        return Err(CommandLedgerError::Corrupt);
    }
    let lease = match document.get("lease") {
        None | Some(Bson::Null) => None,
        Some(value) => Some(
            mongodb::bson::deserialize_from_bson::<layer2_core::sandbox::agent_io::CommandLease>(
                value.clone(),
            )
            .map_err(|_| CommandLedgerError::Corrupt)?,
        ),
    };
    if lease.as_ref().is_some_and(|lease| {
        lease.claim.agent_id.as_str() != agent_id
            || lease.claim.fence == 0
            || lease.claim.fence >= next_fence as u64
            || layer2_core::seconds_from_rfc3339(&lease.expires_at).is_none()
    }) {
        return Err(CommandLedgerError::Corrupt);
    }
    Ok(CommandLane {
        last_order: last_order as u64,
        next_fence: next_fence as u64,
        lease,
    })
}

pub(crate) fn fields(lane: &CommandLane) -> Result<Document, CommandLedgerError> {
    Ok(doc! {
        "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        "last_order": signed(lane.last_order, "command order").map_err(|_| CommandLedgerError::Corrupt)?,
        "next_fence": signed(lane.next_fence, "command fence").map_err(|_| CommandLedgerError::Corrupt)?,
        "lease": record_bson(&lane.lease).map_err(|_| CommandLedgerError::Corrupt)?,
    })
}
