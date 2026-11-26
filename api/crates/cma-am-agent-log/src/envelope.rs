use layer2_core::sandbox::store::SandboxStoreError;
use mongodb::bson::{Bson, Document};

use super::{corrupt, digest, FORMAT};

/// Accepts only the Agent document schema versions this reader understands.
pub fn require_agent_version(document: &Document) -> Result<(), String> {
    match document.get("document_schema_version") {
        Some(Bson::Int64(1 | 2) | Bson::Int32(1 | 2)) => Ok(()),
        _ => Err("unsupported Agent document schema version".into()),
    }
}

/// Minimal discriminator validation is also used by projected aggregate reads.
pub fn read_format(document: &Document) -> Result<Option<(&str, i64)>, SandboxStoreError> {
    require_agent_version(document).map_err(SandboxStoreError::Corrupt)?;
    let version = match document.get("document_schema_version") {
        Some(Bson::Int64(v)) => *v,
        Some(Bson::Int32(v)) => i64::from(*v),
        _ => return Err(corrupt()),
    };
    // Version 1 documents predate segmented logs and must not carry log fields.
    if version == 1 {
        if [
            "agent_log_format",
            "agent_log_incarnation",
            "agent_log_head_revision",
        ]
        .iter()
        .any(|key| document.contains_key(*key))
        {
            return Err(corrupt());
        }
        return Ok(None);
    }
    let incarnation = document
        .get_str("agent_log_incarnation")
        .map_err(|_| corrupt())?;
    let revision = document
        .get_i64("agent_log_head_revision")
        .map_err(|_| corrupt())?;
    if document.get_str("agent_log_format").ok() != Some(FORMAT)
        || incarnation.len() != 64
        || !incarnation
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        || revision < 1
    {
        return Err(corrupt());
    }
    Ok(Some((incarnation, revision)))
}

/// Rejects a segmented document whose log incarnation belongs to another Agent.
pub fn validate_identity(
    document: &Document,
    agent: &layer2_core::sandbox::Agent,
) -> Result<(), SandboxStoreError> {
    if let Some((incarnation, _)) = read_format(document)? {
        if incarnation != digest::incarnation(agent) {
            return Err(corrupt());
        }
    }
    Ok(())
}
