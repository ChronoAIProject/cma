//! Rebuild the writer's output facts using the same bounded private generation.
use super::super::projection::{self, output as fold, EvidenceRecord};
use super::*;
use layer2_core::sandbox::{
    agent_output_index::{AgentOutputTool, AgentTurnOutputIndex},
    AgentLogEntry,
};

pub(super) async fn resume(
    database: &Database,
    progress: &Progress,
    staged: &Head,
    output: &Head,
) -> Result<Option<AgentTurnOutputIndex>, SandboxStoreError> {
    let Some(turn) = progress.completed.semantic.latest_turn else {
        return Ok(None);
    };
    let record = collect::lookup(
        database,
        staged,
        output,
        &persistence::Batch::default(),
        "turn_output",
        &turn.to_string(),
    )
    .await?
    .ok_or_else(corrupt)?;
    match record {
        EvidenceRecord::TurnOutput { index }
            if index.turn_sequence == turn
                && index.through_sequence == progress.completed.checked_entries as u64 =>
        {
            Ok(Some(index))
        }
        _ => Err(corrupt()),
    }
}

pub(super) async fn observe(
    database: &Database,
    staged: &Head,
    output: &Head,
    batch: &persistence::Batch,
    index: &mut Option<AgentTurnOutputIndex>,
    entry: &AgentLogEntry,
) -> Result<Vec<Document>, SandboxStoreError> {
    let tool = if let (Some(index), Some(call)) = (index.as_ref(), fold::call_id(entry)) {
        let key = fold::tool_key(index.turn_sequence, call);
        match collect::lookup(database, staged, output, batch, "tool_output", &key).await? {
            Some(EvidenceRecord::ToolOutput { tool }) => Some(tool),
            None => None,
            _ => return Err(corrupt()),
        }
    } else {
        None
    };
    let supplied = match (fold::call_id(entry), tool.as_ref()) {
        (Some(_), Some(tool)) => AgentOutputTool::Stored(tool),
        (Some(_), None) => AgentOutputTool::Absent,
        (None, _) => AgentOutputTool::None,
    };
    projection::output::fold(staged, index, entry, supplied)
}
