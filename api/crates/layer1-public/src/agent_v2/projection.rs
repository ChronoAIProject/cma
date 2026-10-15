use super::{cursor, error::Error, shapes::EventItem, AgentV2State};
use layer2_core::sandbox::{AgentEventStreamAuthorization, AgentReplayWindow};

pub(super) const MAX_BYTES: usize = 4 * 1024 * 1024;

pub(super) async fn window(
    state: &AgentV2State,
    authorization: &AgentEventStreamAuthorization,
    after: u64,
    through: Option<u64>,
    limit: u16,
    message_bindings: bool,
) -> Result<AgentReplayWindow, Error> {
    let window = state
        .reads
        .event_stream_window(authorization, after, through, limit)
        .await?;
    let window = state
        .commands
        .bind_replay_window(&state.reads, authorization, window)
        .await?;
    Ok(if message_bindings {
        state
            .commands
            .bind_message_window(&state.reads, authorization, window)
            .await?
    } else {
        window
    })
}

/// Reserve envelope overhead and stop before a row that does not fit. That
/// row retains its own cursor and is the first row fetched by the next page.
pub(super) fn items(window: &AgentReplayWindow, identity: &str) -> Result<Vec<EventItem>, Error> {
    let mut bytes = 2048;
    let mut result = Vec::new();
    for entry in &window.entries {
        let item = EventItem {
            cursor: cursor::event(identity, entry.sequence),
            sequence: entry.sequence,
            appended_at: entry.appended_at.clone(),
            kind: entry.kind.clone(),
            response_binding: entry.response_binding.clone().map(Into::into),
            message_command_id: entry.message_command_id.as_ref().map(ToString::to_string),
        };
        let size = serde_json::to_vec(&item).map_err(|_| Error::History)?.len() + 1;
        if bytes + size > MAX_BYTES {
            if result.is_empty() {
                return Err(Error::History);
            }
            break;
        }
        bytes += size;
        result.push(item);
    }
    Ok(result)
}

/// An ignored preference preserves old servers and strict old event consumers during rollout.
pub(super) fn message_bindings(headers: &axum::http::HeaderMap) -> bool {
    headers.get_all("prefer").iter().any(|value| {
        value.to_str().is_ok_and(|value| {
            value
                .split(',')
                .any(|token| token.trim().eq_ignore_ascii_case("cma-message-bindings"))
        })
    })
}
