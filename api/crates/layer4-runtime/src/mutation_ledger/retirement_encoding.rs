//! Versioned archive payloads retain raw source columns, including NULL and
//! noncanonical JSON bytes. Decoding proves shape, never retirement eligibility.
use layer3_adapter::runtime_ledger_retirement::{
    ledger_retired_agent_key, ledger_retired_worktree_key,
};

use super::{
    invalid, validate, Auxiliary, Context, Operation, ResourceKey, Result, RuntimeConfig,
    SourceRow, BYTES,
};

// Earlier held preparations used numeric arrays. Retain their read budget;
// new writes must also fit SQLite's complete row, including keys and headers.
const ENCODED_LIMIT: usize = 8 * 1024 * 1024;

impl SourceRow {
    pub(crate) fn encode(&self, config: &RuntimeConfig) -> Result<Vec<u8>> {
        if self.bytes() > BYTES {
            return Err(invalid());
        }
        validate(config, self)?;
        let bytes = serde_json::to_vec(self).map_err(|_| invalid())?;
        let (scope, key) = self.key();
        // Two 64-byte digests, the integer kind and six SQLite serial/length
        // headers fit in 256 bytes; scope and source key are stored separately.
        if bytes.len() + scope.len() + key.len() + 256
            > crate::ledger_storage::MAX_VALUE_BYTES as usize
        {
            return Err(invalid());
        }
        Ok(bytes)
    }

    pub(crate) fn decode(bytes: &[u8], config: &RuntimeConfig) -> Result<Self> {
        if bytes.len() > ENCODED_LIMIT {
            return Err(invalid());
        }
        let row: Self = serde_json::from_slice(bytes).map_err(|_| invalid())?;
        if row.bytes() > BYTES {
            return Err(invalid());
        }
        validate(config, &row)?;
        Ok(row)
    }

    pub(crate) fn ordering(&self) -> (u8, Vec<u8>, String) {
        let kind = match self {
            Self::Operation { .. } => 3,
            Self::Authority { .. } => 4,
            Self::Auxiliary(Auxiliary::Cleanup { .. }) => 5,
            Self::Auxiliary(Auxiliary::Scope { .. }) => 6,
            Self::Auxiliary(Auxiliary::Process { .. }) => 7,
        };
        let (scope, key) = self.key();
        (kind, scope.into(), key.into())
    }

    /// Scope counters are shared context, not independently retired resources.
    pub(crate) fn group(&self) -> Result<Option<String>> {
        Ok(Some(match self {
            Self::Operation { identity, .. } => {
                let op: Operation = serde_json::from_slice(identity).map_err(|_| invalid())?;
                crate::retirement_archive::group_key(&op)
            }
            Self::Authority { resource, .. }
            | Self::Auxiliary(Auxiliary::Cleanup { resource, .. }) => {
                match serde_json::from_str::<ResourceKey>(resource).map_err(|_| invalid())? {
                    ResourceKey::Worktree { workspace, name } => {
                        ledger_retired_worktree_key(&workspace, &name)
                    }
                    ResourceKey::AgentHome { agent_id }
                    | ResourceKey::AgentProcess { agent_id, .. }
                    | ResourceKey::AgentSkill { agent_id, .. } => {
                        ledger_retired_agent_key(&agent_id)
                    }
                }
            }
            Self::Auxiliary(Auxiliary::Process { agent, .. }) => ledger_retired_agent_key(agent),
            Self::Auxiliary(Auxiliary::Scope { .. }) => return Ok(None),
        }))
    }

    /// Every folded authority identity remains indexed, including the first
    /// Absent fence when it differs from the maximum retained Absent fence.
    pub(crate) fn operation_ids(&self) -> Result<Vec<String>> {
        match self {
            Self::Operation { id, .. } => Ok(vec![id.clone()]),
            Self::Authority { records, .. } => records
                .iter()
                .flatten()
                .map(|bytes| {
                    let record: crate::mutation_authority::Record =
                        serde_json::from_slice(bytes).map_err(|_| invalid())?;
                    Ok(record.operation().operation_id)
                })
                .collect(),
            Self::Auxiliary(_) => Ok(Vec::new()),
        }
    }
}

impl Context {
    pub(crate) fn encode(&self, config: &RuntimeConfig) -> Result<Vec<u8>> {
        self.scoped_config(config)?;
        let bytes = serde_json::to_vec(self).map_err(|_| invalid())?;
        if bytes.len() > BYTES {
            return Err(invalid());
        }
        Ok(bytes)
    }

    pub(crate) fn decode(bytes: &[u8], config: &RuntimeConfig) -> Result<Self> {
        if bytes.len() > BYTES {
            return Err(invalid());
        }
        let context: Self = serde_json::from_slice(bytes).map_err(|_| invalid())?;
        context.scoped_config(config)?;
        Ok(context)
    }

    pub(crate) fn scoped_config(&self, config: &RuntimeConfig) -> Result<RuntimeConfig> {
        let mut scoped = config.clone();
        scoped.generation = super::fence::decode(&self.generation)?;
        if scoped.generation > config.generation {
            return Err(invalid());
        }
        self.validate(&scoped)?;
        Ok(scoped)
    }
}
