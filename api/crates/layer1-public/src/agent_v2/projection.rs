use super::{cursor, error::Error, shapes::EventItem, AgentV2State};
use layer2_core::sandbox::{AgentEventStreamAuthorization, AgentReplayWindow};

pub(super) const MAX_BYTES: usize = 4 * 1024 * 1024;

pub(super) struct Preferences {
    pub message_bindings: bool,
    pub tool_output: bool,
    pub retained_output: bool,
    pub external_approval: bool,
}

impl Preferences {
    pub fn from_headers(headers: &axum::http::HeaderMap) -> Self {
        Self {
            message_bindings: message_bindings(headers),
            tool_output: tool_output(headers),
            retained_output: retained_output(headers),
            external_approval: preference(headers, super::external_approval::PREFERENCE),
        }
    }
}

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
    // The core read already fenced current authority after checking the tail.
    // With no entries there is no command correlation IO to authorize again.
    // Live and durable frames retain their separate release-time checks.
    if window.entries.is_empty() {
        return Ok(window);
    }
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
pub(super) fn items(
    window: &AgentReplayWindow,
    identity: &str,
    tool_output: bool,
    retained_output: bool,
) -> Result<Vec<EventItem>, Error> {
    let mut bytes = 2048;
    let mut result = Vec::new();
    for entry in &window.entries {
        let mut kind = entry.kind.clone();
        if !tool_output {
            if matches!(
                kind,
                Some(layer2_core::sandbox::agent_io::AgentEventKind::ToolOutputReported { .. })
            ) {
                kind = None;
            }
            if let Some(layer2_core::sandbox::agent_io::AgentEventKind::ToolUpdated {
                output,
                ..
            }) = &mut kind
            {
                *output = None;
            }
        }
        let item = EventItem {
            retained_output: entry
                .retained_output
                .filter(|_| retained_output)
                .map(Into::into),
            cursor: cursor::event(identity, entry.sequence),
            sequence: entry.sequence,
            appended_at: entry.appended_at.clone(),
            kind,
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
    preference(headers, "cma-message-bindings")
}

pub(super) fn live_timestamps(headers: &axum::http::HeaderMap) -> bool {
    preference(headers, "cma-live-observed-at")
}

pub(super) fn tool_output(headers: &axum::http::HeaderMap) -> bool {
    preference(headers, "cma-tool-output")
}

pub(super) fn retained_output(headers: &axum::http::HeaderMap) -> bool {
    preference(headers, "cma-retained-output")
}

pub(super) fn preference(headers: &axum::http::HeaderMap, requested: &str) -> bool {
    headers.get_all("prefer").iter().any(|value| {
        value.to_str().is_ok_and(|value| {
            value
                .split(',')
                .any(|token| token.trim().eq_ignore_ascii_case(requested))
        })
    })
}
