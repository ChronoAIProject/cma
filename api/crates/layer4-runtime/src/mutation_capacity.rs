//! Derived V1 reservations; all callers hold the supervisor operation gate.
//!
//! Receipts never expire. An authority-only admission owns its missing journal
//! slot, and every resource owns its eventual cleanup even after a failed or
//! indeterminate create. No counter or new on-disk format is necessary.
use std::collections::BTreeMap;

use crate::config::RuntimeConfig;
use crate::failure::{Result, RuntimeError};
use crate::mutation_authority::{self, Disposition, Record, ResourceKey};
use crate::mutation_journal::{self, Operation};

pub(crate) const DETAIL: &str = "runtime mutation ledger capacity is exhausted";

pub(crate) fn exhausted() -> RuntimeError {
    RuntimeError::rejected("mutation_ledger_capacity", DETAIL)
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Occupancy {
    pub journal_entries: usize,
    pub missing_receipts: usize,
    pub cleanup_receipts: usize,
    pub authority_records: usize,
    pub reserved_authority_records: usize,
}

impl Occupancy {
    fn fits(&self, journals: usize, authorities: usize) -> bool {
        self.journal_entries + self.missing_receipts + self.cleanup_receipts < journals
            && self.authority_records < authorities
            && self.reserved_authority_records < authorities
    }

    fn require_headroom(&self, journals: usize, authorities: usize) -> Result<()> {
        // Reserve one terminal replacement and one authority append before fold.
        if !self.fits(journals, authorities) {
            tracing::warn!(
                journal_entries = self.journal_entries,
                missing_receipts = self.missing_receipts,
                cleanup_receipts = self.cleanup_receipts,
                authority_records = self.authority_records,
                reserved_authority_records = self.reserved_authority_records,
                journal_limit = journals,
                authority_limit = authorities,
                "runtime mutation ledger admission held"
            );
            return Err(exhausted());
        }
        Ok(())
    }
}

pub(crate) fn check(config: &RuntimeConfig, incoming: &[Operation]) -> Result<()> {
    let authority = mutation_authority::records(config)?;
    check_loaded(config, &authority, incoming)
}

pub(crate) fn check_loaded(
    config: &RuntimeConfig,
    authority: &[Record],
    incoming: &[Operation],
) -> Result<()> {
    let journals = mutation_journal::inventory(config)?;
    let baseline = occupancy(&journals, authority, &[])?;
    let proposed = occupancy(&journals, authority, incoming)?;
    let normal_journals = mutation_journal::MAX_JOURNAL_ENTRIES;
    let normal_authorities = mutation_authority::MAX_AUTHORITY_RECORDS;
    if config.legacy_journal_recovery_enabled
        && crate::mutation_recovery::eligible(&journals, authority, incoming)
    {
        baseline.require_headroom(
            crate::mutation_recovery::JOURNALS,
            crate::mutation_recovery::AUTHORITIES,
        )?;
        proposed.require_headroom(
            crate::mutation_recovery::JOURNALS,
            crate::mutation_recovery::AUTHORITIES,
        )?;
        if !baseline.fits(normal_journals, normal_authorities) {
            tracing::info!(
                journal_entries = baseline.journal_entries,
                authority_records = baseline.authority_records,
                missing_receipts = baseline.missing_receipts,
                cleanup_receipts = baseline.cleanup_receipts,
                incoming_operations = incoming.len(),
                "runtime legacy ledger recovery envelope in use"
            );
        }
        return Ok(());
    }
    baseline.require_headroom(normal_journals, normal_authorities)?;
    proposed.require_headroom(normal_journals, normal_authorities)
}

/// Recovery validates the same identities/reservations before reconciling any
/// temporary or replaying effects, independently of normal admission headroom.
pub(crate) fn check_recovery(config: &RuntimeConfig) -> Result<()> {
    check(config, &[])
}

#[derive(Default)]
struct Resource {
    present: Option<Operation>,
    absent: Option<Operation>,
    cleanup: bool,
}

pub(crate) fn occupancy(
    journals: &[Operation],
    authority: &[Record],
    incoming: &[Operation],
) -> Result<Occupancy> {
    let mut operations = BTreeMap::<String, Operation>::new();
    for operation in journals
        .iter()
        .cloned()
        .chain(authority.iter().map(Record::operation))
        .chain(incoming.iter().cloned())
    {
        if let Some(previous) = operations.insert(operation.operation_id.clone(), operation.clone())
        {
            if previous != operation {
                return Err(RuntimeError::rejected(
                    "runtime_mutation_replay_mismatch",
                    "runtime mutation replay differs from the admitted request or rendered payload",
                ));
            }
        }
    }
    let mut resources = BTreeMap::<ResourceKey, Resource>::new();
    for operation in operations.values() {
        let (key, disposition) = mutation_authority::resource_operation(operation);
        // A historical skill can exist without an AgentHome journal. Its home
        // still needs removal capacity; skills have no independent delete RPC.
        if let ResourceKey::AgentSkill { agent_id, .. } = &key {
            resources
                .entry(ResourceKey::AgentHome {
                    agent_id: agent_id.clone(),
                })
                .or_default()
                .cleanup = true;
        }
        let cleanup = !matches!(key, ResourceKey::AgentSkill { .. });
        let resource = resources.entry(key).or_default();
        resource.cleanup |= cleanup;
        let identity = match disposition {
            Disposition::Present => &mut resource.present,
            Disposition::Absent => &mut resource.absent,
        };
        if let Some(old) = identity.as_ref().filter(|old| *old != operation) {
            match (&old.subject, &operation.subject) {
                (
                    mutation_journal::Subject::InstallAgentSkill { request: old },
                    mutation_journal::Subject::InstallAgentSkill { request: next },
                ) if old.sequence() != next.sequence() => {
                    // Every version keeps its journal reservation, while the
                    // resource's single authority slot advances monotonically.
                    if old.sequence() > next.sequence() {
                        continue;
                    }
                }
                _ => {
                    return Err(mutation_authority::corrupt(
                        "runtime mutation authority has conflicting operation identities",
                    ))
                }
            }
        }
        *identity = Some(operation.clone());
    }
    for operation in incoming {
        let (key, disposition) = mutation_authority::resource_operation(operation);
        if disposition == Disposition::Present
            && resources.get(&key).is_some_and(|row| row.absent.is_some())
        {
            return Err(RuntimeError::rejected(
                "mutation_authority_retired",
                "resource cleanup permanently dominates later create authority",
            ));
        }
    }
    let cleanup_receipts = resources
        .values()
        .filter(|row| row.cleanup && row.absent.is_none())
        .count();
    // A cleanup keeps its first and maximum Absent even if they coincide now;
    // future fresh-fence retries must never spend another resource's reserve.
    let reserved_authority_records = resources
        .values()
        .map(|row| usize::from(row.present.is_some()) + if row.cleanup { 2 } else { 0 })
        .sum();
    Ok(Occupancy {
        journal_entries: journals.len(),
        missing_receipts: operations.len() - journals.len(),
        cleanup_receipts,
        authority_records: authority.len(),
        reserved_authority_records,
    })
}
