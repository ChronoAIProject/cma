//! Pure folds are shared by their complete verified inputs, never by a caller.
use super::*;
use layer2_core::sandbox::{agent_io::AgentResponseReading, AgentEventStreamSnapshot};
use sha2::{Digest, Sha256};

#[derive(Clone)]
struct Fold {
    response: Option<V2ResponseObject>,
}
impl serde::Serialize for Fold {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // Internal replay payloads are omitted by the public serializer, but
        // must count toward the same cache budget as the public output.
        self.response
            .as_ref()
            .map(|response| (response, &response.output_events, &response.creation_model))
            .serialize(serializer)
    }
}

pub(super) fn project(
    state: &ResponsesV2State,
    reading: AgentResponseReading,
    history: &AgentEventStreamSnapshot,
) -> Option<V2ResponseObject> {
    if !state.reads.shared_observation_enabled() {
        return projection::project(&state.agent_page_origin, reading, history);
    }
    let input = serde_json::to_vec(&(
        1_u8,
        state.agent_page_origin.agent_url(history.id.as_str()),
        &reading,
        history,
    ))
    .ok()?;
    // Oversized legacy fallback retains its established behavior without
    // becoming an unbounded cache entry or evicting every small response.
    if input.len() > layer2_core::sandbox::agent_observation::MAX_PAGE_BYTES {
        return projection::project(&state.agent_page_origin, reading, history);
    }
    let key = Sha256::digest(input).into();
    state
        .reads
        .memoize_verified_fold(key, || {
            let response = projection::project(&state.agent_page_origin, reading, history);
            Fold { response }
        })
        .response
}
