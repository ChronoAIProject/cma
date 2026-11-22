use super::*;
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReceiptResponse {
    receipt: Receipt,
    #[serde(rename = "processing")]
    _processing: ProcessingObservation,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProcessingObservation {
    #[serde(rename = "validation_status")]
    _validation_status: String,
    #[serde(rename = "validation_reason")]
    _validation_reason: Option<String>,
    #[serde(rename = "owner_status")]
    _owner_status: String,
    #[serde(rename = "financial_status")]
    _financial_status: String,
}

pub(super) fn receipt(bytes: &[u8]) -> Result<Receipt, TransportError> {
    serde_json::from_slice::<ReceiptResponse>(bytes)
        .map(|response| response.receipt)
        .map_err(|_| TransportError::Invalid)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CursorPage {
    stream_id: String,
    contiguous_custody_sequence: String,
    custody_chain_head: Option<String>,
    contiguous_validated_sequence: String,
    validated_chain_head: Option<String>,
    highest_received_sequence: String,
    missing_ranges: Vec<MissingRange>,
    next_page: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MissingRange {
    first: String,
    last: String,
}

pub(super) fn cursor(
    bytes: &[u8],
    stream: &str,
) -> Result<(Cursor, Option<String>), TransportError> {
    let page: CursorPage = serde_json::from_slice(bytes).map_err(|_| TransportError::Invalid)?;
    let validated = contract::integer(&page.contiguous_validated_sequence)?;
    if (validated == 0) != page.validated_chain_head.is_none()
        || page
            .next_page
            .as_ref()
            .is_some_and(|value| value.is_empty() || value.len() > 4096)
    {
        return Err(TransportError::Invalid);
    }
    let cursor = Cursor {
        stream_id: page.stream_id,
        contiguous_sequence: page.contiguous_custody_sequence,
        validated_sequence: page.contiguous_validated_sequence,
        highest_sequence: page.highest_received_sequence,
        head_digest: page.custody_chain_head,
        validated_head_digest: page.validated_chain_head,
        missing_ranges: page
            .missing_ranges
            .into_iter()
            .map(|range| contract::MissingRange {
                from: range.first,
                through: range.last,
            })
            .collect(),
    };
    cursor.validate(stream)?;
    Ok((cursor, page.next_page))
}
