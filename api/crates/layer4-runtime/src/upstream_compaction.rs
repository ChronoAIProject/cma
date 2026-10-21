//! Compatibility for Codex's local Responses compaction after hosted web search.
//!
//! Codex 0.147.0 sends historical `web_search_call` items with `tools: []` when
//! compacting through a custom provider. Some Responses backends reject that
//! combination with an HTTP 200 / `response.failed` stream. Declare only the
//! cached search schema and forbid tool execution. Keep all history bytes and
//! every other request field unchanged. This runs only on the model listener.

use axum::http::{header, HeaderMap, Method, Uri};
use serde::Deserialize;
use serde_json::value::RawValue;

const SEARCH_DECLARATION: &[u8] = br#"[{"type":"web_search","external_web_access":false}]"#;

#[derive(Deserialize)]
struct CompactionRequest<'a> {
    #[serde(borrow)]
    tools: &'a RawValue,
    #[serde(borrow)]
    tool_choice: &'a RawValue,
    #[serde(borrow)]
    input: &'a RawValue,
    client_metadata: ClientMetadata,
    reasoning: Option<Reasoning>,
}

#[derive(Deserialize)]
struct HistoryItem {
    #[serde(rename = "type")]
    kind: Option<String>,
}

#[derive(Deserialize)]
struct ClientMetadata {
    #[serde(rename = "x-codex-turn-metadata")]
    turn: String,
}

#[derive(Deserialize)]
struct TurnMetadata {
    request_kind: String,
    compaction: CompactionMetadata,
}

#[derive(Deserialize)]
struct CompactionMetadata {
    implementation: String,
}

#[derive(Deserialize)]
struct Reasoning {
    context: Option<String>,
}

pub(crate) fn normalize(
    method: &Method,
    uri: &Uri,
    headers: &HeaderMap,
    body: &[u8],
) -> Option<Vec<u8>> {
    if method != Method::POST
        || !matches!(uri.path(), "/responses" | "/v1/responses")
        || crate::upstream::encoded(headers)
        || headers.contains_key("x-openai-internal-codex-responses-lite")
        || headers
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next())
            .is_none_or(|value| !value.trim().eq_ignore_ascii_case("application/json"))
    {
        return None;
    }
    let request: CompactionRequest<'_> = serde_json::from_slice(body).ok()?;
    let metadata: TurnMetadata = serde_json::from_str(&request.client_metadata.turn).ok()?;
    if metadata.request_kind != "compaction"
        || metadata.compaction.implementation != "responses"
        || request
            .reasoning
            .as_ref()
            .and_then(|reasoning| reasoning.context.as_deref())
            == Some("all_turns")
    {
        return None;
    }
    let tools: Vec<serde::de::IgnoredAny> = serde_json::from_str(request.tools.get()).ok()?;
    let choice: &str = serde_json::from_str(request.tool_choice.get()).ok()?;
    if !tools.is_empty() || !matches!(choice, "auto" | "none") {
        return None;
    }
    // Normal turns need no history parsing or allocation. Inspect historical
    // item kinds only after recognizing an otherwise tool-free compaction.
    let history: Vec<HistoryItem> = serde_json::from_str(request.input.get()).ok()?;
    if !history
        .iter()
        .any(|item| item.kind.as_deref() == Some("web_search_call"))
        || history
            .iter()
            .any(|item| item.kind.as_deref() == Some("additional_tools"))
    {
        return None;
    }

    // Borrowed RawValue spans point into the original body. Replace only these
    // two JSON values, preserving encrypted history, unknown fields, numeric
    // spellings, whitespace, and ordering without a JSON round trip.
    let span = |raw: &RawValue| {
        let start = raw.get().as_ptr() as usize - body.as_ptr() as usize;
        (start, start + raw.get().len())
    };
    let mut edits = [
        (span(request.tools), SEARCH_DECLARATION),
        (span(request.tool_choice), br#""none""#.as_slice()),
    ];
    edits.sort_by_key(|(span, _)| span.0);
    let mut normalized = Vec::with_capacity(body.len() + SEARCH_DECLARATION.len());
    let mut copied = 0;
    for ((start, end), replacement) in edits {
        normalized.extend_from_slice(&body[copied..start]);
        normalized.extend_from_slice(replacement);
        copied = end;
    }
    normalized.extend_from_slice(&body[copied..]);
    Some(normalized)
}

#[cfg(test)]
#[path = "upstream_compaction_tests.rs"]
mod tests;
