use super::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Receipt {
    pub receipt_id: String,
    pub event_id: String,
    pub stream_id: String,
    pub sequence: String,
    pub payload_digest: String,
    pub accepted_at_ms: String,
    pub custody: Custody,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Custody {
    Persisted,
}

impl Receipt {
    pub(crate) fn validate(&self, event: &FrozenUsageEvent) -> Result<(), TransportError> {
        let latest = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| TransportError::Invalid)?
            .as_millis()
            .saturating_add(30_000);
        let accepted = integer(&self.accepted_at_ms)?;
        if !id(&self.receipt_id)
            || self.event_id != event.event_id
            || self.stream_id != event.stream_id
            || integer(&self.sequence)? != event.sequence
            || self.payload_digest != event.digest
            || accepted < event.produced_at_ms
            || u128::from(accepted) > latest
        {
            return Err(TransportError::Conflict);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Cursor {
    pub stream_id: String,
    pub contiguous_sequence: String,
    pub validated_sequence: String,
    pub highest_sequence: String,
    pub head_digest: Option<String>,
    pub validated_head_digest: Option<String>,
    pub missing_ranges: Vec<MissingRange>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MissingRange {
    pub from: String,
    pub through: String,
}

impl Cursor {
    pub(crate) fn validate(&self, stream: &str) -> Result<u64, TransportError> {
        let contiguous = integer(&self.contiguous_sequence)?;
        let validated = integer(&self.validated_sequence)?;
        let high = integer(&self.highest_sequence)?;
        if self.stream_id != stream
            || !id(stream)
            || validated > contiguous
            || contiguous > high
            || (contiguous == 0) != self.head_digest.is_none()
            || self
                .head_digest
                .as_ref()
                .is_some_and(|value| !digest(value))
            || (validated == 0) != self.validated_head_digest.is_none()
            || self.validated_head_digest.as_ref().is_some_and(|value| {
                validated == 0
                    || !digest(value)
                    || (validated == contiguous && Some(value) != self.head_digest.as_ref())
            })
            || self.missing_ranges.len() > 512
        {
            return Err(TransportError::Invalid);
        }
        let mut previous = contiguous;
        for range in &self.missing_ranges {
            let from = integer(&range.from)?;
            let through = integer(&range.through)?;
            if from <= previous || from > through || through > high {
                return Err(TransportError::Invalid);
            }
            previous = through;
        }
        Ok(contiguous)
    }
}

pub(super) fn integer(value: &str) -> Result<u64, TransportError> {
    let parsed = value.parse::<u64>().map_err(|_| TransportError::Invalid)?;
    if parsed > i64::MAX as u64 || parsed.to_string() != value {
        return Err(TransportError::Invalid);
    }
    Ok(parsed)
}
fn id(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok_and(|v| v.get_version_num() == 4 && v.to_string() == value)
}
fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}
