use super::*;
use crate::responses::v2::projection::indexed as projection;
use layer2_core::sandbox::store::indexed::OUTPUT_PAGE_BYTES;
use layer2_core::sandbox::{
    agent_io::AgentResponseReading, AgentOutputObservation, AgentReadError,
};

pub(in crate::responses::v2) struct Page {
    pub events: Vec<Value>,
    pub through: u64,
    pub last: u64,
    pub more: bool,
    pub terminal: bool,
    pub pagination_required: bool,
}

pub(in crate::responses::v2) async fn read(
    source: &mut AgentOutputObservation<'_>,
    origin: &crate::agent_page::AgentPageOrigin,
    reading: AgentResponseReading,
    next: u64,
) -> Result<Page, AgentReadError> {
    let metadata = projection::project_metadata(source, origin, reading.clone()).await?;
    let bound = source.metadata().index.is_some();
    let opening_count = if bound { 2_u64 } else { 1 };
    let output_count = source
        .metadata()
        .index
        .as_ref()
        .map(|i| projection::event_count(&i.totals))
        .transpose()?
        .unwrap_or(0);
    let terminal_sequence = opening_count + output_count;
    let last = terminal_sequence - 1 + u64::from(metadata.terminal());
    if next > last + 1 {
        return Err(AgentReadError::InvalidPage);
    }
    let mut page = Page {
        events: vec![],
        through: source.metadata().through,
        last,
        more: false,
        terminal: false,
        pagination_required: false,
    };
    // Opening frames do not wait for source payloads or the terminal object.
    if next < opening_count {
        page.events = super::projection::events(&metadata, bound)
            .into_iter()
            .take(opening_count as usize)
            .filter(|v| v["sequence_number"].as_u64().is_some_and(|s| s >= next))
            .collect();
        page.more = metadata.terminal() || output_count > 0;
        return Ok(page);
    }
    if next < terminal_sequence {
        let from = projection::seek(source, next - opening_count).await?;
        let changes = source.changes(from, 1).await?;
        let mut bytes = 0_usize;
        for change in changes {
            for event in projection::change_events(source, &change).await? {
                let seq = event["sequence_number"]
                    .as_u64()
                    .ok_or_else(projection::corrupt)?;
                if seq < next {
                    continue;
                }
                let size = serde_json::to_vec(&event)
                    .map_err(|_| projection::corrupt())?
                    .len();
                if size > OUTPUT_PAGE_BYTES {
                    page.pagination_required = true;
                    return Ok(page);
                }
                if bytes + size > OUTPUT_PAGE_BYTES {
                    page.more = true;
                    return Ok(page);
                }
                bytes += size;
                page.events.push(event);
            }
        }
        page.more = page
            .events
            .last()
            .is_some_and(|v| v["sequence_number"].as_u64().is_some_and(|s| s < last));
        return Ok(page);
    }
    if metadata.terminal() && next == terminal_sequence {
        match projection::complete(source, origin, reading).await? {
            Some(response) => {
                let kind = match response.status {
                    crate::responses::v2::V2ResponseStatus::Completed => "response.completed",
                    crate::responses::v2::V2ResponseStatus::Failed => "response.failed",
                    _ => "response.incomplete",
                };
                let event =
                    json!({"type":kind,"sequence_number":terminal_sequence,"response":response});
                if serde_json::to_vec(&event)
                    .map_err(|_| projection::corrupt())?
                    .len()
                    > OUTPUT_PAGE_BYTES
                {
                    page.pagination_required = true;
                } else {
                    page.events.push(event);
                    page.terminal = true;
                }
            }
            None => page.pagination_required = true,
        }
    } else {
        page.terminal = metadata.terminal();
    }
    Ok(page)
}
