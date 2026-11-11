use layer3_adapter::runtime_ledger_retirement::{
    ledger_retired_agent_key, ledger_retired_worktree_key,
};
use sha2::{Digest, Sha256};

use super::invalid;
use crate::config::RuntimeConfig;
use crate::failure::Result;
use crate::mutation_authority::Record as Authority;
use crate::mutation_journal::{Operation, Record as Journal, Subject};

#[derive(Clone)]
pub(crate) struct SourceRecord {
    pub(crate) scope: u64,
    pub(crate) kind: u8,
    pub(crate) name: String,
    pub(crate) bytes: Vec<u8>,
}

impl SourceRecord {
    pub(super) fn operation(&self, config: &RuntimeConfig) -> Result<Operation> {
        if self.scope > config.generation || self.bytes.is_empty() || !canonical(&self.name) {
            return Err(invalid());
        }
        let mut scoped = config.clone();
        if self.scope != 0 {
            scoped.generation = self.scope;
        }
        let (operation, path) = match self.kind {
            1 => {
                let record = Journal::decode(&self.bytes, &scoped).map_err(|_| invalid())?;
                (record.operation(), record.inventory_name(&scoped))
            }
            2 => {
                if self.bytes.len() as u64 > crate::mutation_authority::MAX_AUTHORITY_BYTES {
                    return Err(invalid());
                }
                let record: Authority =
                    serde_json::from_slice(&self.bytes).map_err(|_| invalid())?;
                let path = record.inventory_name(&scoped);
                let record = Authority::decode_inventory(&self.bytes, &scoped, &path)
                    .map_err(|_| invalid())?;
                (record.operation(), path)
            }
            _ => return Err(invalid()),
        };
        if path.file_name().and_then(|name| name.to_str()) != Some(self.name.as_str())
            || crate::mutation_scope::generation(&scoped, &operation.subject).unwrap_or(0)
                != self.scope
        {
            return Err(invalid());
        }
        Ok(operation)
    }

    pub(crate) fn hash_into(&self, digest: &mut Sha256) {
        digest.update(self.scope.to_be_bytes());
        digest.update([self.kind]);
        digest.update((self.name.len() as u64).to_be_bytes());
        digest.update(self.name.as_bytes());
        digest.update((self.bytes.len() as u64).to_be_bytes());
        digest.update(&self.bytes);
    }
}

pub(crate) fn group_key(operation: &Operation) -> String {
    match &operation.subject {
        Subject::AddWorktree {
            workspace, name, ..
        }
        | Subject::RemoveWorktree { workspace, name } => {
            ledger_retired_worktree_key(workspace, name)
        }
        Subject::InstallAgentSkill { request } => {
            ledger_retired_agent_key(request.agent_id.as_str())
        }
        Subject::CreateAgentHome { agent_id }
        | Subject::RemoveAgentHome { agent_id }
        | Subject::OpenAgent { agent_id, .. }
        | Subject::CloseAgent { agent_id } => ledger_retired_agent_key(agent_id),
    }
}

pub(super) fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn canonical(name: &str) -> bool {
    name.strip_suffix(".json").is_some_and(digest)
}
