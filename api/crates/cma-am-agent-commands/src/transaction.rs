//! Command and lane persistence within the caller's MongoDB transaction.

use cma_am_mongo_primitives::signed;
use layer2_core::sandbox::agent_io::{
    AdmittedAgentCommand, CommandClaim, CommandLane, CommandLedgerError,
};
use mongodb::bson::{doc, Document};
use mongodb::{ClientSession, Collection};

use crate::{lane_records, records, ApplyError};

pub async fn load_claim(
    commands: &Collection<Document>,
    orders: &Collection<Document>,
    claim: &CommandClaim,
    now: &str,
    session: &mut ClientSession,
) -> Result<(AdmittedAgentCommand, CommandLane), ApplyError> {
    let lane = orders
        .find_one(doc! { "_id": claim.agent_id.as_str() })
        .session(&mut *session)
        .await?
        .ok_or(CommandLedgerError::StaleClaim)?;
    let lane = lane_records::read(&lane, claim.agent_id.as_str())?;
    lane.require_claim(claim, now)?;
    let command = commands
        .find_one(doc! { "_id": claim.command_id.as_str() })
        .session(&mut *session)
        .await?
        .ok_or(CommandLedgerError::StaleClaim)?;
    let command = records::read(&command)?;
    command.require_claim(claim, now)?;
    if lane.lease != command.work.lease {
        return Err(CommandLedgerError::Corrupt.into());
    }
    Ok((command, lane))
}

pub async fn write(
    commands: &Collection<Document>,
    orders: &Collection<Document>,
    command: &AdmittedAgentCommand,
    expected_revision: u64,
    lane: &CommandLane,
    session: &mut ClientSession,
) -> Result<(), ApplyError> {
    let mut fields = records::document(command)?;
    fields.remove("_id");
    let result = commands.update_one(doc! { "_id": command.receipt.envelope.command_id.as_str(), "channel_history_seal": {"$exists": false}, "revision": signed(expected_revision, "command revision").map_err(|_| CommandLedgerError::Corrupt)? }, doc! { "$set": fields }).session(&mut *session).await?;
    if result.matched_count != 1 {
        return Err(CommandLedgerError::Conflict.into());
    }
    let result = orders
        .update_one(
            doc! { "_id": command.receipt.envelope.agent_id.as_str() },
            doc! { "$set": lane_records::fields(lane)? },
        )
        .session(&mut *session)
        .await?;
    if result.matched_count != 1 {
        return Err(CommandLedgerError::Conflict.into());
    }
    Ok(())
}
