//! Read-only retirement observations. This artifact grants no runtime effects.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const LEDGER_RETIREMENT_SNAPSHOT_FORMAT: &str = "runtime-ledger-retirement-observation-v1";
pub const MAX_LEDGER_RETIREMENT_RESOURCES: usize = 16_384;
pub const MAX_LEDGER_RETIREMENT_SNAPSHOT_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeLedgerRetirementSnapshot {
    pub format: String,
    pub sandbox_id: String,
    pub home_identity: String,
    pub generation: u64,
    pub sandbox_revision: u64,
    pub observed_at: String,
    pub source_sha256: String,
    pub agents_observed: u64,
    pub worktrees_observed: u64,
    /// Only complete, settled deletion observations enter these sorted sets.
    /// Missing records and terminal resources with outstanding work stay out.
    pub deleted_agents: Vec<String>,
    pub removed_worktrees: Vec<String>,
}

impl RuntimeLedgerRetirementSnapshot {
    pub fn validate(&self) -> bool {
        self.format == LEDGER_RETIREMENT_SNAPSHOT_FORMAT
            && !self.sandbox_id.is_empty()
            && self.sandbox_id.len() <= 256
            && !self.sandbox_id.chars().any(char::is_control)
            && crate::valid_home_identity(&self.home_identity)
            && self.generation > 0
            && self.sandbox_revision > 0
            && !self.observed_at.is_empty()
            && self.observed_at.len() <= 64
            && !self.observed_at.chars().any(char::is_control)
            && time::OffsetDateTime::parse(
                &self.observed_at,
                &time::format_description::well_known::Rfc3339,
            )
            .is_ok()
            && digest(&self.source_sha256)
            && self.agents_observed <= MAX_LEDGER_RETIREMENT_RESOURCES as u64
            && self.worktrees_observed <= MAX_LEDGER_RETIREMENT_RESOURCES as u64
            && self.deleted_agents.len() as u64 <= self.agents_observed
            && self.removed_worktrees.len() as u64 <= self.worktrees_observed
            && ordered(&self.deleted_agents)
            && ordered(&self.removed_worktrees)
    }

    pub fn contains_agent(&self, agent: &str) -> bool {
        self.deleted_agents
            .binary_search(&ledger_retired_agent_key(agent))
            .is_ok()
    }

    pub fn contains_worktree(&self, workspace: &str, name: &str) -> bool {
        self.removed_worktrees
            .binary_search(&ledger_retired_worktree_key(workspace, name))
            .is_ok()
    }
}

fn ordered(values: &[String]) -> bool {
    values.len() <= MAX_LEDGER_RETIREMENT_RESOURCES
        && values.iter().all(|value| digest(value))
        && values.windows(2).all(|pair| pair[0] < pair[1])
}

fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Length-prefix every component; separators in names cannot alias another key.
fn key(parts: &[&str]) -> String {
    let mut digest = Sha256::new();
    digest.update(b"chrono.runtime.ledger.retirement.v1\0");
    for part in parts {
        digest.update((part.len() as u64).to_be_bytes());
        digest.update(part.as_bytes());
    }
    format!("{:x}", digest.finalize())
}

pub fn ledger_retired_agent_key(agent: &str) -> String {
    key(&["agent", agent])
}

pub fn ledger_retired_worktree_key(workspace: &str, name: &str) -> String {
    key(&["worktree", workspace, name])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retirement_keys_are_namespaced_and_component_boundaries_are_unambiguous() {
        assert_ne!(
            ledger_retired_agent_key("a/b"),
            ledger_retired_worktree_key("a", "b")
        );
        assert_ne!(
            ledger_retired_worktree_key("a/b", "c"),
            ledger_retired_worktree_key("a", "b/c")
        );
        assert!(digest(&ledger_retired_agent_key("fixture-agent")));
    }

    #[test]
    fn duplicate_unsorted_and_malformed_retirement_sets_are_refused() {
        let first = "a".repeat(64);
        let second = "b".repeat(64);
        assert!(ordered(&[first.clone(), second.clone()]));
        assert!(!ordered(&[first.clone(), first.clone()]));
        assert!(!ordered(&[second, first]));
        assert!(!ordered(&["A".repeat(64)]));
        assert!(!ordered(&["a".repeat(63)]));
    }

    #[test]
    fn snapshot_validation_rejects_invalid_scope_time_counts_and_format() {
        let snapshot = RuntimeLedgerRetirementSnapshot {
            format: LEDGER_RETIREMENT_SNAPSHOT_FORMAT.into(),
            sandbox_id: "fixture".into(),
            home_identity: "a".repeat(43),
            generation: 1,
            sandbox_revision: 1,
            observed_at: "2026-11-11T00:00:00Z".into(),
            source_sha256: "b".repeat(64),
            agents_observed: 1,
            worktrees_observed: 0,
            deleted_agents: vec![ledger_retired_agent_key("agent")],
            removed_worktrees: vec![],
        };
        assert!(snapshot.validate());
        for case in 0..7 {
            let mut invalid = snapshot.clone();
            match case {
                0 => invalid.generation = 0,
                1 => invalid.sandbox_revision = 0,
                2 => invalid.agents_observed = 0,
                3 => invalid.agents_observed = MAX_LEDGER_RETIREMENT_RESOURCES as u64 + 1,
                4 => invalid.observed_at = "invalid timestamp".into(),
                5 => invalid.home_identity = "invalid home".into(),
                _ => invalid.format = "future observation".into(),
            }
            assert!(!invalid.validate(), "case {case}");
        }
    }
}
