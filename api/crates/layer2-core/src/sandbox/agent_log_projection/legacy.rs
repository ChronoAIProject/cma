use super::*;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub(super) fn payload_digest(content: &str) -> String {
    format!("{:x}", Sha256::digest(content.as_bytes()))
}

/// Streaming bootstrap for one physically bounded legacy log. Retains only
/// semantic state and queued input hashes; message and tool text is discarded.
/// Unlimited sealed history uses durable indexed evidence between audit units.
#[derive(Default)]
pub struct BoundedAgentLogFold {
    state: AgentLogProjectionState,
    queued: BTreeMap<u64, String>,
}

impl BoundedAgentLogFold {
    pub fn observe(
        &mut self,
        agent: &Agent,
        entry: &AgentLogEntry,
    ) -> Result<Vec<AgentLogEvidenceChange>, String> {
        if self.state.through_seq >= 10_128 {
            return Err("legacy Agent projection exceeds one physical segment".into());
        }
        let source = AgentLogProjectionState::queued_owner(entry);
        if source.is_some_and(|seq| seq >= entry.seq || !self.queued.contains_key(&seq)) {
            return Err("queued turn has no matching original owner message".into());
        }
        if let AgentLogKind::OwnerMessage { content } = &entry.kind {
            if self.state.active.is_none() {
                self.queued.insert(entry.seq, payload_digest(content));
            }
        }
        self.state.observe(agent, entry, source)
    }

    pub fn finish(self, agent: &Agent) -> Result<AgentLogProjectionState, String> {
        self.state.validate_aggregate(agent)?;
        if agent
            .conversation
            .queued_messages
            .iter()
            .any(|queued| self.queued.get(&queued.sequence) != Some(&queued.payload_sha256))
        {
            return Err("queued owner messages disagree with durable queue state".into());
        }
        Ok(self.state)
    }
}
