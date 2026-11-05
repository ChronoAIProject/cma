use layer2_core::ids::AgentCommandId;
use layer2_core::sandbox::agent_io::{AdmittedAgentCommand, CommandEvidence, CommandLedgerError};
use layer2_core::sandbox::store::SandboxStoreError;
use mongodb::bson::doc;
use mongodb::ClientSession;

use super::super::agent_log::scan_complete_log_with_session;
use super::super::agent_log_integrity::TransactionLogError;
use super::super::agents::read_agent;
use super::super::sandbox_mongo::MongoSandboxStore;
use super::{lane_records, records, worker_apply, ApplyError};

pub(super) async fn apply(
    store: &MongoSandboxStore,
    id: &AgentCommandId,
    session: &mut ClientSession,
) -> Result<AdmittedAgentCommand, ApplyError> {
    let document = store
        .agent_commands
        .find_one(doc! { "_id": id.as_str() })
        .session(&mut *session)
        .await?
        .ok_or(CommandLedgerError::NotFound)?;
    let mut command = records::read(&document)?;
    if command.work.dispatch.is_none() || command.status.is_terminal() {
        return Ok(command);
    }
    let agent_id = &command.receipt.envelope.agent_id;
    let agent = store
        .agents
        .find_one(doc! { "_id": agent_id.as_str() })
        .session(&mut *session)
        .await?
        .ok_or(CommandLedgerError::NotFound)?;
    let agent = read_agent(&agent).map_err(|_| CommandLedgerError::Corrupt)?;
    let lane = store
        .agent_command_orders
        .find_one(doc! { "_id": agent_id.as_str() })
        .session(&mut *session)
        .await?
        .ok_or(CommandLedgerError::Corrupt)?;
    let mut lane = lane_records::read(&lane, agent_id.as_str())?;
    let projected =
        super::super::agent_log_segments::command_evidence(store, &agent, &command, session)
            .await
            .map_err(|error| match error {
                TransactionLogError::Domain(_) => ApplyError::Domain(CommandLedgerError::Corrupt),
                TransactionLogError::Driver(error) => ApplyError::Driver(error),
            })?;
    let evidence = if let Some(evidence) = projected {
        evidence
    } else {
        let mut evidence = CommandEvidence::new(&command)?;
        scan_complete_log_with_session(
            store,
            agent_id,
            agent.conversation.last_seq,
            session,
            |entry| {
                evidence
                    .observe(&entry)
                    .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))
            },
        )
        .await
        .map_err(|error| match error {
            TransactionLogError::Domain(_) => ApplyError::Domain(CommandLedgerError::Corrupt),
            TransactionLogError::Driver(error) => ApplyError::Driver(error),
        })?;
        evidence
    };
    let retry_current = if evidence.has_source_deferred() {
        let current = store
            .bindings
            .continuation_retry_authority(&command.receipt.envelope, session)
            .await?;
        let fenced = store.agents.update_one(doc! {"_id":agent.id.as_str(),"revision":i64::try_from(agent.revision).map_err(|_|CommandLedgerError::Corrupt)?},doc! {"$inc":{"agent_command_admission_fence":1_i64}}).session(&mut *session).await?;
        if fenced.matched_count != 1 {
            return Err(CommandLedgerError::Conflict.into());
        }
        current
    } else {
        false
    };
    let revision = command.revision;
    if evidence.apply_with_continuation_retry(&mut command, &mut lane, &agent, retry_current)? {
        worker_apply::write(store, &command, revision, &lane, session).await?;
    }
    Ok(command)
}
