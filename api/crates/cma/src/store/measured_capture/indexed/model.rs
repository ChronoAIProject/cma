//! Canonical projection and independently anchored time-bucket manifests.
use super::super::{decode, CaptureError};
use layer2_core::sandbox_metrics::measured_evidence::CustodyProjection;
use mongodb::bson::{doc, Document};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub(super) const HOUR_MS: u64 = 3_600_000;
pub(super) const HOURS_PER_DAY: u64 = 24;
pub(super) const ROOT_ID: &str = "measured-index-generation";
pub(super) const MAX_DOCUMENT_BYTES: usize = 8 * 1024 * 1024;
pub(super) const MAX_SERIES_HOUR_ROWS: usize = 2048;
pub(super) const MAX_DAYS: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Phase {
    Building,
    Ready,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::store::measured_capture) struct Certification {
    pub original_rows: u64,
    pub ordered_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Generation {
    pub generation: u64,
    pub revision: u64,
    pub phase: Phase,
    pub writer_floor_sha256: String,
    pub rows: u64,
    pub cursor: Option<String>,
    pub certification: Option<Certification>,
    pub days: BTreeMap<String, String>,
}

impl Generation {
    pub fn new(generation: u64, writer_floor_sha256: String) -> Result<Self, CaptureError> {
        if generation == 0 || !valid_digest(&writer_floor_sha256) {
            return Err(CaptureError::Corrupt);
        }
        Ok(Self {
            generation,
            revision: 1,
            phase: Phase::Building,
            writer_floor_sha256,
            rows: 0,
            cursor: None,
            certification: None,
            days: BTreeMap::new(),
        })
    }

    pub fn validate(&self) -> Result<(), CaptureError> {
        if self.generation == 0
            || self.revision == 0
            || !valid_digest(&self.writer_floor_sha256)
            || self.days.len() > MAX_DAYS
            || self.days.values().any(|digest| !valid_digest(digest))
            || self.days.keys().any(|day| canonical_number(day).is_none())
            || (self.phase == Phase::Ready) != self.certification.is_some()
            || self.certification.as_ref().is_some_and(|proof| {
                proof.original_rows > self.rows || !valid_digest(&proof.ordered_sha256)
            })
        {
            return Err(CaptureError::Corrupt);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DayDirectory {
    pub hours: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct HourDirectory {
    pub series: BTreeMap<String, String>,
    /// Scope discovery is derived from canonical source rows, including conflicts.
    pub scopes: BTreeMap<String, BTreeSet<String>>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SeriesManifest {
    pub leaves: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Projection {
    pub generation: u64,
    pub source_row_id: String,
    pub kind: String,
    pub keys: CustodyProjection,
}

impl Projection {
    pub fn from_source(generation: u64, source: &Document) -> Result<Self, CaptureError> {
        let kind = source.get_str("kind").map_err(|_| CaptureError::Corrupt)?;
        let conflict = match kind {
            "sample" => false,
            "conflict" => true,
            _ => return Err(CaptureError::Corrupt),
        };
        let physical = source
            .get_str("physical_sample_id")
            .map_err(|_| CaptureError::Corrupt)?;
        let evidence = decode(source, physical, conflict)?;
        Ok(Self {
            generation,
            source_row_id: source
                .get_str("_id")
                .map_err(|_| CaptureError::Corrupt)?
                .into(),
            kind: kind.into(),
            keys: evidence
                .custody_projection()
                .map_err(|_| CaptureError::Corrupt)?,
        })
    }

    pub fn id(&self) -> String {
        format!("{}:{}", self.generation, self.source_row_id)
    }

    pub fn hour(&self) -> u64 {
        self.keys.timestamp_ms / HOUR_MS
    }

    pub fn day(&self) -> u64 {
        self.hour() / HOURS_PER_DAY
    }

    pub fn scope(&self) -> Result<String, CaptureError> {
        scope_id(&self.keys.owner_id, &self.keys.sandbox_id)
    }

    pub fn document(&self) -> Result<Document, CaptureError> {
        Ok(doc! {
            "_id": self.id(),
            "schema_version": 1_i64,
            "generation": signed(self.generation)?,
            "source_row_id": &self.source_row_id,
            "series": &self.keys.physical_series_id,
            "hour": signed(self.hour())?,
            "timestamp_ms": signed(self.keys.timestamp_ms)?,
            "scope": self.scope()?,
            "payload": canonical(self)?,
        })
    }

    pub fn decode(row: &Document) -> Result<Self, CaptureError> {
        let payload = row.get_str("payload").map_err(|_| CaptureError::Corrupt)?;
        let value: Self = serde_json::from_str(payload).map_err(|_| CaptureError::Corrupt)?;
        if value.generation == 0
            || !valid_digest(&value.keys.physical_series_id)
            || !valid_digest(&value.keys.physical_sample_id)
            || !valid_digest(&value.keys.evidence_sha256)
            || value.keys.owner_id.is_empty()
            || value.keys.sandbox_id.is_empty()
            || !matches!(value.kind.as_str(), "sample" | "conflict")
            || value.document()? != *row
        {
            return Err(CaptureError::Corrupt);
        }
        Ok(value)
    }

    pub fn leaf(&self) -> Result<String, CaptureError> {
        digest(self)
    }
}

pub(super) fn signed(value: u64) -> Result<i64, CaptureError> {
    value.try_into().map_err(|_| CaptureError::Corrupt)
}

pub(super) fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub(super) fn canonical_number(value: &str) -> Option<u64> {
    let number: u64 = value.parse().ok()?;
    (number.to_string() == value).then_some(number)
}

pub(super) fn scope_id(owner: &str, sandbox: &str) -> Result<String, CaptureError> {
    if owner.is_empty() || sandbox.is_empty() {
        return Err(CaptureError::Corrupt);
    }
    digest(&(owner, sandbox))
}

pub(super) fn canonical<T: Serialize>(value: &T) -> Result<String, CaptureError> {
    let payload = serde_json::to_string(value).map_err(|_| CaptureError::Corrupt)?;
    if payload.len() > MAX_DOCUMENT_BYTES {
        return Err(CaptureError::Unavailable);
    }
    Ok(payload)
}

pub(super) fn digest<T: Serialize>(value: &T) -> Result<String, CaptureError> {
    Ok(format!(
        "{:x}",
        Sha256::digest(canonical(value)?.as_bytes())
    ))
}

/// Every manifest is authenticated by its parent digest. The root document is
/// the sole mutable authority for the generation and its complete directories.
pub(super) fn sealed<T: Serialize>(
    id: &str,
    generation: u64,
    value: &T,
) -> Result<Document, CaptureError> {
    Ok(doc! {
        "_id": id,
        "schema_version": 1_i64,
        "generation": signed(generation)?,
        "payload_sha256": digest(value)?,
        "payload": canonical(value)?,
    })
}

pub(super) fn open<T: Serialize + DeserializeOwned>(
    row: &Document,
    id: &str,
    generation: u64,
    expected: Option<&str>,
) -> Result<T, CaptureError> {
    let payload = row.get_str("payload").map_err(|_| CaptureError::Corrupt)?;
    let value: T = serde_json::from_str(payload).map_err(|_| CaptureError::Corrupt)?;
    if sealed(id, generation, &value)? != *row
        || expected.is_some_and(|expected| row.get_str("payload_sha256").ok() != Some(expected))
    {
        return Err(CaptureError::Corrupt);
    }
    Ok(value)
}

pub(super) fn day_id(generation: u64, day: u64) -> String {
    format!("day:{generation}:{day}")
}

pub(super) fn hour_id(generation: u64, hour: u64) -> String {
    format!("hour:{generation}:{hour}")
}

pub(super) fn series_id(generation: u64, hour: u64, series: &str) -> String {
    format!("series:{generation}:{hour}:{series}")
}
