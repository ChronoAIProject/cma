//! Explicit V1 recovery envelope. This never expands normal resource admission.
use std::collections::BTreeSet;

use crate::config::RuntimeConfig;
use crate::failure::Result;
use crate::mutation_authority::{self, Disposition, Record, ResourceKey};
use crate::mutation_capacity::exhausted;
use crate::mutation_journal::Operation;

pub(crate) const JOURNALS: usize = 8_192;
pub(crate) const AUTHORITIES: usize = 32_768;
const SCAN_BYTES: u64 = 64 * 1024 * 1024;

pub(crate) fn journal_limit(config: &RuntimeConfig) -> usize {
    if config.legacy_journal_recovery_enabled {
        JOURNALS
    } else {
        crate::mutation_journal::MAX_JOURNAL_ENTRIES
    }
}

pub(crate) fn authority_limit(config: &RuntimeConfig) -> usize {
    if config.legacy_journal_recovery_enabled {
        AUTHORITIES
    } else {
        mutation_authority::MAX_AUTHORITY_RECORDS
    }
}

/// Shared across persistent and active-process roots, charged before allocation.
/// Terminal payloads remain bounded even when the larger entry envelope is used.
pub(crate) struct ScanBudget {
    remaining_entries: usize,
    remaining_bytes: u64,
}

impl ScanBudget {
    pub(crate) fn new(config: &RuntimeConfig, entries: usize) -> Self {
        Self {
            remaining_entries: entries,
            remaining_bytes: if config.legacy_journal_recovery_enabled {
                SCAN_BYTES
            } else {
                u64::MAX
            },
        }
    }

    pub(crate) fn entry(&mut self) -> Result<()> {
        self.remaining_entries = self
            .remaining_entries
            .checked_sub(1)
            .ok_or_else(exhausted)?;
        Ok(())
    }

    pub(crate) fn bytes(&mut self, bytes: u64) -> Result<()> {
        self.remaining_bytes = self
            .remaining_bytes
            .checked_sub(bytes)
            .ok_or_else(exhausted)?;
        Ok(())
    }
}

/// Recovery may finish an admitted operation or consume an existing resource's
/// cleanup reservation. It cannot admit a new create, skill version, or resource.
pub(crate) fn eligible(
    journals: &[Operation],
    authority: &[Record],
    incoming: &[Operation],
) -> bool {
    let known = journals
        .iter()
        .cloned()
        .chain(authority.iter().map(Record::operation))
        .collect::<Vec<_>>();
    let mut resources = BTreeSet::new();
    for operation in &known {
        let (resource, _) = mutation_authority::resource_operation(operation);
        if let ResourceKey::AgentSkill { agent_id, .. } = &resource {
            resources.insert(ResourceKey::AgentHome {
                agent_id: agent_id.clone(),
            });
        }
        resources.insert(resource);
    }
    incoming.iter().all(|operation| {
        let (resource, disposition) = mutation_authority::resource_operation(operation);
        known.contains(operation)
            || (disposition == Disposition::Absent && resources.contains(&resource))
    })
}
