use serde::{Deserialize, Serialize};

// Retained v1 diagnostics. Fresh remote cursors have no v1 snapshot and are not stored.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct StoredCursorV1 {
    stream_id: String,
    contiguous_sequence: String,
    validated_sequence: String,
    highest_sequence: String,
    head_digest: Option<String>,
    missing_ranges: Vec<MissingRange>,
    snapshot: String,
    next_cursor: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MissingRange {
    from: String,
    through: String,
}
