//! Versioned additive envelope and exact raw BSON tail identity.
use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::store) struct LogIntegrity {
    pub(in crate::store) head_seq: u64,
    pub(in crate::store) entry_count: u64,
    pub(in crate::store) encoded_bytes: usize,
    pub(in crate::store) head_hash: [u8; HASH_BYTES],
    pub(in crate::store) bound_agent_revision: u64,
    pub(in crate::store) history_identity: [u8; HASH_BYTES],
    pub(in crate::store) semantic: SemanticCheckpoint,
}

impl LogIntegrity {
    pub(in crate::store) fn from_summary(
        agent: &Agent,
        summary: &LogSummary,
        bound_agent_revision: u64,
        semantic: SemanticCheckpoint,
    ) -> Self {
        Self {
            head_seq: summary.entry_count,
            entry_count: summary.entry_count,
            encoded_bytes: summary.encoded_bytes,
            head_hash: summary.head_hash,
            bound_agent_revision,
            history_identity: history_identity(agent),
            semantic,
        }
    }

    pub(in crate::store) fn to_document(&self) -> Result<Document, SandboxStoreError> {
        Ok(doc! {
            "format": INTEGRITY_FORMAT,
            "head_seq": signed(self.head_seq, "agent log integrity head sequence")
                .map_err(SandboxStoreError::Corrupt)?,
            "head_hash": Bson::Binary(Binary {
                subtype: mongodb::bson::spec::BinarySubtype::Generic,
                bytes: self.head_hash.to_vec(),
            }),
            "entry_count": signed(self.entry_count, "agent log integrity entry count")
                .map_err(SandboxStoreError::Corrupt)?,
            "encoded_bytes": signed(
                u64::try_from(self.encoded_bytes)
                    .map_err(|_| SandboxStoreError::Corrupt("encoded bytes exceed BSON range".into()))?,
                "agent log integrity encoded bytes",
            )
            .map_err(SandboxStoreError::Corrupt)?,
            "bound_agent_revision": signed(
                self.bound_agent_revision,
                "agent log integrity bound revision",
            )
            .map_err(SandboxStoreError::Corrupt)?,
            "history_identity": Bson::Binary(Binary {
                subtype: mongodb::bson::spec::BinarySubtype::Generic,
                bytes: self.history_identity.to_vec(),
            }),
            SEMANTIC_FIELD: mongodb::bson::serialize_to_document(&self.semantic)
                .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?,
        })
    }
}

pub(in crate::store) fn parse_integrity(
    document: &Document,
) -> Result<Option<LogIntegrity>, SandboxStoreError> {
    let Some(value) = document.get(INTEGRITY_FIELD) else {
        return Ok(None);
    };
    let block = value.as_document().ok_or_else(|| {
        SandboxStoreError::Corrupt("agent log integrity is not a document".into())
    })?;
    let format = i64_value(block, "format")?;
    if format != INTEGRITY_FORMAT {
        return Ok(None);
    }
    let head_seq = unsigned(
        i64_value(block, "head_seq")?,
        "agent log integrity head sequence",
    )
    .map_err(SandboxStoreError::Corrupt)?;
    let entry_count = unsigned(
        i64_value(block, "entry_count")?,
        "agent log integrity entry count",
    )
    .map_err(SandboxStoreError::Corrupt)?;
    let encoded_bytes = usize::try_from(
        unsigned(
            i64_value(block, "encoded_bytes")?,
            "agent log integrity encoded bytes",
        )
        .map_err(SandboxStoreError::Corrupt)?,
    )
    .map_err(|_| SandboxStoreError::Corrupt("agent log integrity bytes exceed usize".into()))?;
    let bound_agent_revision = unsigned(
        i64_value(block, "bound_agent_revision")?,
        "agent log integrity bound revision",
    )
    .map_err(SandboxStoreError::Corrupt)?;
    let head_hash = hash_value(block, "head_hash")?;
    let history_identity = hash_value(block, "history_identity")?;
    let semantic = block
        .get_document(SEMANTIC_FIELD)
        .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))
        .and_then(parse_semantic)?;
    Ok(Some(LogIntegrity {
        head_seq,
        entry_count,
        encoded_bytes,
        head_hash,
        bound_agent_revision,
        history_identity,
        semantic,
    }))
}

fn i64_value(document: &Document, field: &str) -> Result<i64, SandboxStoreError> {
    document
        .get_i64(field)
        .map_err(|error| SandboxStoreError::Corrupt(format!("{field}: {error}")))
}

fn hash_value(document: &Document, field: &str) -> Result<[u8; HASH_BYTES], SandboxStoreError> {
    let value = document
        .get(field)
        .ok_or_else(|| SandboxStoreError::Corrupt(format!("missing integrity field {field}")))?;
    let Bson::Binary(binary) = value else {
        return Err(SandboxStoreError::Corrupt(format!(
            "integrity field {field} is not binary"
        )));
    };
    if binary.bytes.len() != HASH_BYTES
        || binary.subtype != mongodb::bson::spec::BinarySubtype::Generic
    {
        return Err(SandboxStoreError::Corrupt(format!(
            "integrity field {field} has invalid length"
        )));
    }
    let mut output = [0_u8; HASH_BYTES];
    output.copy_from_slice(&binary.bytes);
    Ok(output)
}

pub(super) fn hash_raw(raw: &[u8]) -> [u8; HASH_BYTES] {
    let mut digest = Sha256::new();
    digest.update(b"cma.agent-log.head.v1\0");
    digest.update(raw);
    digest.finalize().into()
}

fn update_hash_material(digest: &mut Sha256, value: &str) {
    digest.update((value.len() as u64).to_le_bytes());
    digest.update(value.as_bytes());
}

pub(super) fn history_identity(agent: &Agent) -> [u8; HASH_BYTES] {
    let mut digest = Sha256::new();
    digest.update(b"cma.agent-log.history.v1\0");
    for value in [
        agent.id.as_str(),
        agent.creation_key.as_str(),
        agent.creation_fingerprint.as_str(),
        agent.created_at.as_str(),
        agent.workspace_id.as_str(),
        agent.creation_worktree_id().as_str(),
    ] {
        update_hash_material(&mut digest, value);
    }
    digest.finalize().into()
}

pub(super) fn parse_semantic(document: &Document) -> Result<SemanticCheckpoint, SandboxStoreError> {
    let checkpoint: SemanticCheckpoint = mongodb::bson::deserialize_from_document(document.clone())
        .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
    checkpoint.validate()?;
    Ok(checkpoint)
}
