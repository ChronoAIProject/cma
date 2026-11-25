use super::*;
use crate::store::{
    agent_log_integrity::{IntegrityFailurePolicy, TransactionLogError},
    agent_log_telemetry::ScanCaller,
};
use layer2_core::sandbox::{
    agent_io::{AdmittedAgentCommand, AgentResponseStatus, CommandEvidence},
    store::SandboxStoreError,
    AgentLogKind,
};

pub(super) async fn classify(
    store: &MongoSandboxStore,
    command: &AdmittedAgentCommand,
    session: &mut ClientSession,
) -> Result<&'static str> {
    let Some(dispatch) = &command.work.dispatch else {
        // Before dispatch, a failed/cancelled response is itself the durable
        // public result. Report its distinct evidence strength explicitly.
        return Ok(match command.status {
            AgentResponseStatus::Queued | AgentResponseStatus::WaitingAction => "unfinished",
            AgentResponseStatus::Failed if command.work.failure.is_some() => "receipt_terminal",
            AgentResponseStatus::Cancelled => "receipt_terminal",
            _ => "status_mismatch",
        });
    };
    let Some(row) = store
        .agents
        .find_one(doc! {"_id":command.receipt.envelope.agent_id.as_str()})
        .max_time(Duration::from_secs(1))
        .session(&mut *session)
        .await
        .map_err(|_| "database")?
    else {
        return Ok("evidence_missing");
    };
    let Ok(agent) = crate::store::agents::read_agent(&row) else {
        return Ok("evidence_invalid");
    };
    let evidence = match crate::store::agent_log_segments::command_evidence::read_transaction(
        store, &agent, &row, command, session,
    )
    .await
    {
        Ok(Some(evidence)) => evidence,
        Ok(None) => {
            let Ok(mut evidence) = CommandEvidence::new(command) else {
                return Ok("evidence_invalid");
            };
            let mut terminals = 0;
            let result = crate::store::agent_log_integrity::scan_complete_log_with_policy(
                ScanCaller::Other,
                store,
                &agent.id,
                agent.conversation.last_seq,
                session,
                IntegrityFailurePolicy::ReportOnly,
                |entry| {
                    if matches!(&entry.kind,
                        AgentLogKind::TurnCompleted { turn_sequence } |
                        AgentLogKind::TurnInterrupted { turn_sequence } |
                        AgentLogKind::TurnFailed { turn_sequence, .. }
                        if *turn_sequence == dispatch.turn_sequence)
                    {
                        terminals += 1;
                    }
                    evidence
                        .observe(&entry)
                        .map_err(|_| SandboxStoreError::Corrupt("task evidence is invalid".into()))
                },
            )
            .await;
            if let Err(error) = result {
                return failure(error);
            }
            if terminals > 1 {
                return Ok("duplicate_terminal");
            }
            evidence
        }
        Err(error) => return failure(error),
    };
    if evidence.has_source_deferred() {
        return Ok("evidence_deferred");
    }
    let Ok(status) = evidence.verified_status(command, &agent) else {
        return Ok("evidence_invalid");
    };
    Ok(if status != command.status {
        "status_mismatch"
    } else if status.is_terminal() {
        "verified_terminal"
    } else {
        "unfinished"
    })
}
fn failure(error: TransactionLogError) -> Result<&'static str> {
    match error {
        TransactionLogError::Driver(_) => Err("database"),
        TransactionLogError::Domain(SandboxStoreError::Unavailable(_)) => {
            Ok("evidence_unavailable")
        }
        TransactionLogError::Domain(_) => Ok("evidence_invalid"),
    }
}
