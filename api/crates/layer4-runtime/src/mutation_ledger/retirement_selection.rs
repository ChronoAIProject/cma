//! Bounded, settled source selection from a verified retained archive.
use std::collections::BTreeSet;

use layer3_adapter::runtime_ledger_retirement::{
    RuntimeLedgerRetirementSnapshot, MAX_LEDGER_RETIREMENT_SNAPSHOT_BYTES,
};
use sha2::Digest;

use super::{
    invalid, validate, Auxiliary, Context, Operation, ResourceKey, Result, RuntimeConfig, Sha256,
    SourceRow,
};
use crate::mutation_authority::{Disposition, Record as Authority};
use crate::retirement_archive::GroupProof;

pub(crate) const MAX_ROWS: usize = 16_384;
pub(crate) const MAX_BYTES: usize = 32 * 1024 * 1024;
type Resource = (u64, ResourceKey);

pub(crate) struct Selection {
    pub(super) proof: GroupProof,
    pub(super) context: Context,
    pub(super) rows: Vec<SourceRow>,
    pub(super) operations: i64,
    pub(super) authority: i64,
    pub(super) cleanup: i64,
    pub(super) group_digest: String,
    pub(super) worktrees: BTreeSet<String>,
}

impl Selection {
    /// The archive owner must verify durable payloads and every replay index
    /// before constructing this selection. Observation is not writer exclusion.
    pub(crate) fn new(
        config: &RuntimeConfig,
        proof: GroupProof,
        context: Context,
        rows: Vec<SourceRow>,
        observation: &[u8],
    ) -> Result<Self> {
        context.validate(config)?;
        if observation.len() > MAX_LEDGER_RETIREMENT_SNAPSHOT_BYTES
            || format!("{:x}", Sha256::digest(observation)) != proof.observation_sha256
        {
            return Err(invalid());
        }
        let observed: RuntimeLedgerRetirementSnapshot =
            serde_json::from_slice(observation).map_err(|_| invalid())?;
        if !observed.validate()
            || observed.sandbox_id != config.sandbox_id
            || Some(&observed.home_identity) != config.home_identity.as_ref()
            || observed.generation != config.generation
            || !(observed.deleted_agents.contains(&proof.key)
                || observed.removed_worktrees.contains(&proof.key))
        {
            return Err(invalid());
        }
        if rows.is_empty() || rows.len() > MAX_ROWS || rows.len() != proof.records as usize {
            return Err(invalid());
        }
        let mut result = Self {
            proof,
            context,
            rows,
            operations: 0,
            authority: 0,
            cleanup: 0,
            group_digest: String::new(),
            worktrees: BTreeSet::new(),
        };
        let mut resources = BTreeSet::<Resource>::new();
        let mut absent = BTreeSet::new();
        let mut completed = BTreeSet::new();
        let mut digest = Sha256::new();
        let mut full = Sha256::new();
        result.context.hash_into(&mut full);
        let mut bytes = result.context.bytes();
        let mut previous = None;
        for row in &result.rows {
            bytes = bytes.checked_add(row.bytes()).ok_or_else(invalid)?;
            if bytes > MAX_BYTES {
                return Err(invalid());
            }
            validate(config, row)?;
            let ordering = row.ordering();
            if previous.as_ref().is_some_and(|p| p >= &ordering) {
                return Err(invalid());
            }
            previous = Some(ordering);
            row.hash_into(&mut full);
            if let Some(key) = row.group()? {
                if key != result.proof.key {
                    return Err(invalid());
                }
                row.hash_into(&mut digest);
            }
            let scope = u64::from_be_bytes(row.key().0.try_into().map_err(|_| invalid())?);
            match row {
                SourceRow::Operation {
                    identity, status, ..
                } => {
                    if *status != 2 {
                        return Err(invalid());
                    }
                    let op: Operation = serde_json::from_slice(identity).map_err(|_| invalid())?;
                    let (resource, disposition) =
                        crate::mutation_authority::resource_operation(&op);
                    resources.insert((scope, resource.clone()));
                    if disposition == Disposition::Absent {
                        completed.insert((scope, resource));
                    }
                    result.operations += 1;
                }
                SourceRow::Authority {
                    resource, records, ..
                } => {
                    let resource: ResourceKey =
                        serde_json::from_str(resource).map_err(|_| invalid())?;
                    if let ResourceKey::Worktree { workspace, name } = &resource {
                        result.worktrees.insert(
                            crate::worktree::expected_path(&config.home, workspace, name)
                                .to_string_lossy()
                                .into(),
                        );
                    }
                    resources.insert((scope, resource.clone()));
                    for bytes in records.iter().flatten() {
                        let record: Authority =
                            serde_json::from_slice(bytes).map_err(|_| invalid())?;
                        if record.disposition == Disposition::Absent {
                            absent.insert((scope, resource.clone()));
                        }
                    }
                    result.authority += 1;
                }
                SourceRow::Auxiliary(Auxiliary::Cleanup {
                    resource,
                    available,
                    ..
                }) => {
                    if *available {
                        return Err(invalid());
                    }
                    resources.insert((
                        scope,
                        serde_json::from_str(resource).map_err(|_| invalid())?,
                    ));
                    result.cleanup += 1;
                }
                SourceRow::Auxiliary(Auxiliary::Process { .. }) => return Err(invalid()),
                // Shared generation counters are checked but never removed.
                SourceRow::Auxiliary(Auxiliary::Scope { .. }) => {}
            }
        }
        if result.operations == 0
            || result.authority == 0
            || format!("{:x}", full.finalize()) != result.proof.source_sha256
            || !resources.iter().all(|resource| {
                let parent;
                let resource = if let ResourceKey::AgentSkill { agent_id, .. } = &resource.1 {
                    parent = (
                        0,
                        ResourceKey::AgentHome {
                            agent_id: agent_id.clone(),
                        },
                    );
                    &parent
                } else {
                    resource
                };
                absent.contains(resource) && completed.contains(resource)
            })
        {
            return Err(invalid());
        }
        result.group_digest = format!("{:x}", digest.finalize());
        Ok(result)
    }

    pub(super) fn after_accounting(&self) -> Result<[i64; 5]> {
        let mut after = self.context.accounting;
        after[0] = after[0].checked_add(1).ok_or_else(invalid)?;
        for (slot, removed) in [
            (2, self.operations),
            (3, self.authority),
            (4, self.operations),
        ] {
            after[slot] = after[slot]
                .checked_sub(removed)
                .filter(|n| *n >= 0)
                .ok_or_else(invalid)?;
        }
        Ok(after)
    }
}
