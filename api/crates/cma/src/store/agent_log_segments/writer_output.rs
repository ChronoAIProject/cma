//! Output evidence uses the same captured transaction and Agent/head CAS.
use super::*;
use crate::store::agent_log_segments::{
    collections::collection,
    corrupt,
    projection::{self, output, EvidenceRecord},
    EVIDENCE,
};
use layer2_core::sandbox::agent_output_index::AgentOutputTool;
use mongodb::bson::doc;
use std::collections::BTreeMap;

pub(super) async fn plan(
    store: &MongoSandboxStore,
    scope: &mut TransactionScope<'_>,
    captured: &CapturedLog,
    before: &Head,
    after: &Head,
    logs: &[AgentLogEntry],
) -> Result<Vec<planning::EvidenceWrite>, TransactionLogError> {
    let mut cache = BTreeMap::<String, Option<Document>>::new();
    let mut writes = BTreeMap::<String, planning::EvidenceWrite>::new();
    let mut index = if let Some(turn) = captured.projection().latest_turn {
        let row = original(
            store,
            scope,
            before,
            "turn_output",
            &turn.to_string(),
            &mut cache,
        )
        .await?
        .ok_or_else(corrupt)?;
        match projection::read_evidence(before, "turn_output", &turn.to_string(), row)? {
            EvidenceRecord::TurnOutput { index }
                if index.through_sequence == captured.through() =>
            {
                Some(index)
            }
            _ => return Err(corrupt().into()),
        }
    } else {
        None
    };
    for entry in logs {
        let tool = if let (Some(index), Some(call)) = (&index, output::call_id(entry)) {
            let key = output::tool_key(index.turn_sequence, call);
            let id = projection::identity(before, "tool_output", &key);
            let (head, row) = if let Some(write) = writes.get(&id) {
                (after, Some(write.after.clone()))
            } else {
                (
                    before,
                    original(store, scope, before, "tool_output", &key, &mut cache).await?,
                )
            };
            match row
                .map(|row| projection::read_evidence(head, "tool_output", &key, row))
                .transpose()?
            {
                Some(EvidenceRecord::ToolOutput { tool }) => Some(tool),
                None => None,
                _ => return Err(corrupt().into()),
            }
        } else {
            None
        };
        let supplied = match (output::call_id(entry), tool.as_ref()) {
            (Some(_), Some(tool)) => AgentOutputTool::Stored(tool),
            (Some(_), None) => AgentOutputTool::Absent,
            (None, _) => AgentOutputTool::None,
        };
        for row in output::fold(after, &mut index, entry, supplied)? {
            let id = row.get_str("_id").map_err(|_| corrupt())?.to_owned();
            let kind = row.get_str("kind").map_err(|_| corrupt())?;
            if let Some(write) = writes.get_mut(&id) {
                if kind == "output_change" {
                    return Err(corrupt().into());
                }
                write.after = row;
            } else {
                let key = row.get_str("entity_key").map_err(|_| corrupt())?;
                let old = original(store, scope, before, kind, key, &mut cache).await?;
                // Immutable changes may never be overwritten, even on a retry.
                // Retry settlement uses the original publication receipt.
                if kind == "output_change" && old.is_some() {
                    return Err(corrupt().into());
                }
                if kind == "turn_output"
                    && old.is_some()
                    && index
                        .as_ref()
                        .is_some_and(|i| i.turn_sequence > captured.through())
                {
                    return Err(corrupt().into());
                }
                writes.insert(
                    id,
                    planning::EvidenceWrite {
                        before: old,
                        after: row,
                    },
                );
            }
        }
    }
    if writes.len() > 64 * 3 || cache.len() > 64 * 3 + 1 {
        return Err(corrupt().into());
    }
    Ok(writes.into_values().collect())
}

async fn original(
    store: &MongoSandboxStore,
    scope: &mut TransactionScope<'_>,
    head: &Head,
    kind: &str,
    key: &str,
    cache: &mut BTreeMap<String, Option<Document>>,
) -> Result<Option<Document>, TransactionLogError> {
    let id = projection::identity(head, kind, key);
    if let Some(row) = cache.get(&id) {
        return Ok(row.clone());
    }
    let row = scope
        .find_one(&collection(store, EVIDENCE), doc! {"_id":&id})
        .await
        .map_err(TransactionLogError::Driver)?;
    if let Some(row) = &row {
        projection::read_evidence(head, kind, key, row.clone())?;
    }
    cache.insert(id, row.clone());
    Ok(row)
}
