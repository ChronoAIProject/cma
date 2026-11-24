use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::{invalid, SandboxStoreError, RUNTIME_ACTIVITY_MAX_ROWS};
use crate::SandboxId;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeActivityCounter {
    Desktop,
    CommandAdmission,
    ConversationAdmission,
    ConversationReopen,
    WorkspaceAdmission,
    OwnedWorkspaceAdmission,
    Maintenance,
    WorkNextFence,
    CommandLastOrder,
    CommandNextFence,
    LogProjectionThrough,
}

/// Only allowlisted metadata contributes to this digest. It binds exact source
/// values (including absent/null/zero) without retaining prompt/session content.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeActivityRecord {
    pub id: String,
    pub metadata_sha256: String,
    pub revision: Option<u64>,
    pub last_seq: Option<u64>,
    pub counters: BTreeMap<RuntimeActivityCounter, Option<u64>>,
    pub active: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeActivityVector {
    pub sandbox_id: SandboxId,
    pub sandbox: RuntimeActivityRecord,
    pub workspaces: Vec<RuntimeActivityRecord>,
    pub agents: Vec<RuntimeActivityRecord>,
    pub command_orders: Vec<RuntimeActivityRecord>,
    pub commands: Vec<RuntimeActivityRecord>,
    pub log_heads: Vec<RuntimeActivityRecord>,
    pub log_tails: Vec<RuntimeActivityRecord>,
    /// Session keys are one-way digests, never returned raw session identifiers.
    pub desktop_leases: Vec<RuntimeActivityRecord>,
}

impl RuntimeActivityRecord {
    pub fn validate(&self) -> Result<(), SandboxStoreError> {
        if self.id.is_empty()
            || self.id.len() > 512
            || self.id.chars().any(char::is_control)
            || self.metadata_sha256.len() != 64
            || !self
                .metadata_sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || self.revision == Some(0)
        {
            return Err(invalid());
        }
        Ok(())
    }
}

impl RuntimeActivityVector {
    pub fn validate(&self) -> Result<(), SandboxStoreError> {
        self.sandbox.validate()?;
        if self.sandbox_id.as_str() != self.sandbox.id
            || self.sandbox.revision.is_none()
            || self.sandbox.last_seq.is_none()
        {
            return Err(invalid());
        }
        for rows in self.rows() {
            if rows.len() > RUNTIME_ACTIVITY_MAX_ROWS
                || rows.windows(2).any(|pair| pair[0].id >= pair[1].id)
            {
                return Err(invalid());
            }
            for row in rows {
                row.validate()?;
            }
        }
        let agent_ids = self
            .agents
            .iter()
            .map(|row| row.id.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        if self
            .command_orders
            .iter()
            .chain(&self.log_heads)
            .chain(&self.log_tails)
            .any(|row| !agent_ids.contains(row.id.as_str()))
        {
            return Err(invalid());
        }
        for agent in &self.agents {
            let logs = self
                .log_heads
                .iter()
                .chain(&self.log_tails)
                .filter(|row| row.id == agent.id)
                .collect::<Vec<_>>();
            if agent.revision.is_none()
                || agent.last_seq.is_none()
                || logs.len() > 1
                || (agent.last_seq != Some(0) && logs.len() != 1)
                || logs.iter().any(|row| row.last_seq != agent.last_seq)
            {
                return Err(invalid());
            }
        }
        Ok(())
    }

    pub fn has_activity(&self) -> bool {
        self.sandbox.active || self.rows().into_iter().flatten().any(|row| row.active)
    }

    fn rows(&self) -> [&[RuntimeActivityRecord]; 7] {
        [
            &self.workspaces,
            &self.agents,
            &self.command_orders,
            &self.commands,
            &self.log_heads,
            &self.log_tails,
            &self.desktop_leases,
        ]
    }
}
