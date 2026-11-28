//! Public projection over certified source points, never a reconstructed log.
use super::*;
use crate::responses::v2::{output::OutputProjection, V2OutputItem};
use layer2_core::sandbox::agent_output_index::{
    AgentOutputChange, AgentOutputTotals, AgentToolOutputState,
};
use layer2_core::sandbox::store::indexed::OUTPUT_PAGE_BYTES;
use layer2_core::sandbox::{AgentOutputObservation, AgentReadError};

#[path = "indexed_projection/metadata.rs"]
mod metadata;
#[path = "indexed_projection/output.rs"]
mod output;

pub(in crate::responses) fn corrupt() -> AgentReadError {
    AgentReadError::Corrupt("certified output does not match its original source".into())
}

/// Exact durable event cardinality, independent of payload bytes or lifetime.
pub(in crate::responses) fn event_count(t: &AgentOutputTotals) -> Result<u64, AgentReadError> {
    t.messages
        .checked_mul(5)
        .and_then(|n| n.checked_add(t.nonempty_messages))
        .and_then(|n| n.checked_add(t.text.deltas))
        .and_then(|n| n.checked_add(2 * u64::from(t.text.is_open())))
        .and_then(|n| n.checked_sub(t.text.finished))
        .and_then(|n| n.checked_add(t.tool_starts.checked_mul(2)?))
        .and_then(|n| n.checked_add(t.tool_finishes.checked_mul(2)?))
        .filter(|n| *n <= 9_007_199_254_740_991)
        .ok_or_else(corrupt)
}

pub(in crate::responses) use metadata::project_metadata;
pub(in crate::responses) use output::{change_events, current_item};

pub(in crate::responses) struct OutputPage {
    pub items: Vec<V2OutputItem>,
    pub next: u64,
    pub more: bool,
}

pub(in crate::responses) async fn output_page(
    source: &mut AgentOutputObservation<'_>,
    from: u64,
) -> Result<OutputPage, AgentReadError> {
    let index = source.metadata().index.as_ref().ok_or_else(corrupt)?;
    let total = index.changes;
    let incremental = index.totals.text.deltas > 0;
    // One item group shares one bounded original-point request. An index has
    // no payload byte-size summary; combining unrelated groups could multiply
    // the source-byte budget before the public item budget is checked.
    let mut changes = source.changes(from, 1).await?;
    if let Some(change) = changes
        .first()
        .filter(|row| incremental && row.after.items == row.before.items)
    {
        let from = seek_change(source, change.after.items, |row| Ok(row.after.items)).await?;
        changes = source.changes(from, 1).await?;
        if changes.is_empty() {
            return Ok(OutputPage {
                items: Vec::new(),
                next: total,
                more: false,
            });
        }
    }
    let mut items = Vec::new();
    let mut bytes = 2048;
    let mut next = from;
    for change in changes {
        if change.after.items > change.before.items {
            let item = current_item(source, &change).await?;
            let size = serde_json::to_vec(&item).map_err(|_| corrupt())?.len();
            if size + 2048 > OUTPUT_PAGE_BYTES {
                return Err(AgentReadError::Unavailable(
                    "one output item exceeds the page budget".into(),
                ));
            }
            if bytes + size > OUTPUT_PAGE_BYTES {
                break;
            }
            bytes += size;
            items.push(item);
        }
        next = change.ordinal + 1;
    }
    Ok(OutputPage {
        items,
        next,
        more: next < total,
    })
}

/// None is a result-size refusal, never missing history or an empty response.
pub(in crate::responses) async fn complete(
    source: &mut AgentOutputObservation<'_>,
    origin: &crate::agent_page::AgentPageOrigin,
    reading: AgentResponseReading,
) -> Result<Option<V2ResponseObject>, AgentReadError> {
    let mut response = project_metadata(source, origin, reading).await?;
    if source.metadata().index.is_none() {
        return Ok(Some(response));
    }
    let mut bytes = serde_json::to_vec(&response).map_err(|_| corrupt())?.len();
    let mut from = 0;
    loop {
        let page = output_page(source, from).await?;
        for item in page.items {
            bytes = bytes
                .checked_add(serde_json::to_vec(&item).map_err(|_| corrupt())?.len() + 1)
                .ok_or_else(corrupt)?;
            if bytes > OUTPUT_PAGE_BYTES {
                return Ok(None);
            }
            response.output.push(item);
        }
        if !page.more {
            break;
        }
        if page.next <= from {
            return Err(corrupt());
        }
        from = page.next;
    }
    Ok(Some(response))
}

/// Find the first immutable change that contains an output event at `offset`.
/// Each probe is an indexed point, never an OFFSET or a prefix traversal.
pub(in crate::responses) async fn seek(
    source: &mut AgentOutputObservation<'_>,
    offset: u64,
) -> Result<u64, AgentReadError> {
    let index = source.metadata().index.as_ref().ok_or_else(corrupt)?;
    if offset > event_count(&index.totals)? {
        return Err(AgentReadError::InvalidPage);
    }
    seek_change(source, offset, |row| event_count(&row.after)).await
}

pub(in crate::responses) async fn seek_message(
    source: &mut AgentOutputObservation<'_>,
    message: u64,
) -> Result<AgentOutputChange, AgentReadError> {
    let position = seek_change(source, message, |row| Ok(row.after.messages)).await?;
    source
        .changes(position, 1)
        .await?
        .into_iter()
        .next()
        .filter(|change| change.message_ordinal == Some(message) && change.after.messages > message)
        .ok_or_else(corrupt)
}

async fn seek_change(
    source: &mut AgentOutputObservation<'_>,
    offset: u64,
    count: impl Fn(&AgentOutputChange) -> Result<u64, AgentReadError>,
) -> Result<u64, AgentReadError> {
    let mut low = 0;
    let mut high = source
        .metadata()
        .index
        .as_ref()
        .ok_or_else(corrupt)?
        .changes;
    while low < high {
        let mid = low + (high - low) / 2;
        let rows = source.changes(mid, 1).await?;
        let row = rows.first().ok_or_else(corrupt)?;
        if count(row)? <= offset {
            low = mid + 1;
        } else {
            high = mid;
        }
    }
    Ok(low)
}
