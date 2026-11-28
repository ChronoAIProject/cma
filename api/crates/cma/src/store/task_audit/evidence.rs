use super::*;
use crate::store::{
    agent_log_integrity::{IntegrityFailurePolicy, TransactionLogError},
    agent_log_segments::command_evidence,
    agent_log_telemetry::ScanCaller,
};
use layer2_core::sandbox::{
    agent_io::{AgentResponseStatus, CommandEvidence},
    store::SandboxStoreError,
    Agent, AgentLogKind,
};

pub(super) fn undispatched(command: &AdmittedAgentCommand) -> &'static str {
    // Before dispatch, a failed/cancelled response is itself the durable
    // public result. Report its distinct evidence strength explicitly.
    match command.status {
        AgentResponseStatus::Queued | AgentResponseStatus::WaitingAction => "unfinished",
        AgentResponseStatus::Failed if command.work.failure.is_some() => "receipt_terminal",
        AgentResponseStatus::Cancelled => "receipt_terminal",
        _ => "status_mismatch",
    }
}

pub(super) async fn classify_agent(
    store: &MongoSandboxStore,
    agent_id: &AgentId,
    commands: &[(usize, AdmittedAgentCommand)],
    entries: &mut [Entry],
    session: &mut ClientSession,
) -> Result<()> {
    let row = store
        .agents
        .find_one(doc! {"_id":agent_id.as_str()})
        .max_time(Duration::from_secs(1))
        .session(&mut *session)
        .await
        .map_err(|_| "database")?;
    let set_all = |entries: &mut [Entry], classification| {
        for (index, _) in commands {
            entries[*index].result = classification;
        }
    };
    let Some(row) = row else {
        set_all(entries, "evidence_missing");
        return Ok(());
    };
    let Ok(agent) = crate::store::agents::read_agent(&row) else {
        set_all(entries, "evidence_invalid");
        return Ok(());
    };
    match command_evidence::read_source(store, &agent, &row, session).await {
        Ok(Some(source)) => {
            for (index, command) in commands {
                entries[*index].result = match command_evidence::read_from_source(
                    store, &source, command, session,
                )
                .await
                {
                    Ok(evidence) => classify(&evidence, command, &agent),
                    Err(error) => failure(error)?,
                };
            }
        }
        Ok(None) => classify_legacy(store, &agent, commands, entries, session).await?,
        Err(error) => set_all(entries, failure(error)?),
    }
    Ok(())
}

struct LegacyEvidence {
    turn_sequence: u64,
    terminals: usize,
    evidence: Option<CommandEvidence>,
}

impl LegacyEvidence {
    fn new(command: &AdmittedAgentCommand) -> Self {
        Self {
            turn_sequence: command
                .work
                .dispatch
                .as_ref()
                .map_or(0, |dispatch| dispatch.turn_sequence),
            terminals: 0,
            evidence: CommandEvidence::new(command).ok(),
        }
    }
}

async fn classify_legacy(
    store: &MongoSandboxStore,
    agent: &Agent,
    commands: &[(usize, AdmittedAgentCommand)],
    entries: &mut [Entry],
    session: &mut ClientSession,
) -> Result<()> {
    let mut folds: Vec<_> = commands
        .iter()
        .map(|(_, command)| LegacyEvidence::new(command))
        .collect();
    let scan = crate::store::agent_log_integrity::scan_complete_log_with_policy(
        ScanCaller::Other,
        store,
        &agent.id,
        agent.conversation.last_seq,
        session,
        IntegrityFailurePolicy::ReportOnly,
        |entry| {
            let terminal = match &entry.kind {
                AgentLogKind::TurnCompleted { turn_sequence }
                | AgentLogKind::TurnInterrupted { turn_sequence }
                | AgentLogKind::TurnFailed { turn_sequence, .. } => Some(*turn_sequence),
                _ => None,
            };
            for fold in &mut folds {
                if let Some(evidence) = &mut fold.evidence {
                    if terminal == Some(fold.turn_sequence) {
                        fold.terminals += 1;
                    }
                    if evidence.observe(&entry).is_err() {
                        fold.evidence = None;
                    }
                }
            }
            Ok(())
        },
    )
    .await;
    let scan_failure = scan.err().map(failure).transpose()?;
    for ((index, command), fold) in commands.iter().zip(folds) {
        entries[*index].result = match (fold.evidence, scan_failure) {
            (None, _) => "evidence_invalid",
            (Some(_), Some(classification)) => classification,
            (Some(_), None) if fold.terminals > 1 => "duplicate_terminal",
            (Some(evidence), None) => classify(&evidence, command, agent),
        };
    }
    Ok(())
}

fn classify(
    evidence: &CommandEvidence,
    command: &AdmittedAgentCommand,
    agent: &Agent,
) -> &'static str {
    if evidence.has_source_deferred() {
        return "evidence_deferred";
    }
    let Ok(status) = evidence.verified_status(command, agent) else {
        return "evidence_invalid";
    };
    if status != command.status {
        "status_mismatch"
    } else if status.is_terminal() {
        "verified_terminal"
    } else {
        "unfinished"
    }
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
