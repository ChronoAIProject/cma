//! Nonmutating comparison with a service-owned retirement observation.
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use layer3_adapter::runtime_ledger_retirement::{
    ledger_retired_agent_key, ledger_retired_worktree_key, RuntimeLedgerRetirementSnapshot,
    MAX_LEDGER_RETIREMENT_SNAPSHOT_BYTES,
};
use sha2::{Digest, Sha256};

use super::inventory::{self, Kind};
use super::{changed, invalid, validation, Budget, Limits};
use crate::config::RuntimeConfig;
use crate::failure::Result;
use crate::mutation_authority::{Disposition, Record as Authority, ResourceKey};
use crate::mutation_journal::{Operation, Record as Journal, Status, Subject};

type Resource = (u64, ResourceKey);

#[derive(Default)]
struct Group {
    entries: Vec<usize>,
    journals: u64,
    authority: u64,
    current_journals: u64,
    current_authority: u64,
    observed_terminal: bool,
    uncertain: bool,
    resources: BTreeSet<Resource>,
    authority_absent: BTreeSet<Resource>,
    completed_absent: BTreeSet<Resource>,
}

impl Group {
    fn closed(&self) -> bool {
        self.observed_terminal
            && !self.uncertain
            && !self.resources.is_empty()
            && self.resources.iter().all(|resource| {
                // Skills have no independent removal RPC. Their retained home
                // must be authoritatively absent with a completed remove.
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
                self.authority_absent.contains(resource) && self.completed_absent.contains(resource)
            })
    }
}

pub(super) struct Analysis {
    pub(super) summary: serde_json::Value,
    pub(super) candidates: BTreeMap<String, Vec<usize>>,
}

pub(super) fn observation(
    config: &RuntimeConfig,
    snapshot_path: &Path,
    expected_snapshot_sha256: &str,
    budget: &mut Budget,
) -> Result<(RuntimeLedgerRetirementSnapshot, Vec<u8>)> {
    let parent = snapshot_path.parent().ok_or_else(invalid)?;
    let name = snapshot_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(invalid)?;
    let (_, bytes) = super::io::Directory::open(parent)?.read(
        name,
        MAX_LEDGER_RETIREMENT_SNAPSHOT_BYTES as u64,
        budget,
    )?;
    let snapshot_sha256 = format!("{:x}", Sha256::digest(&bytes));
    if snapshot_sha256 != expected_snapshot_sha256 {
        return Err(changed());
    }
    let snapshot: RuntimeLedgerRetirementSnapshot =
        serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    if !snapshot.validate()
        || snapshot.sandbox_id != config.sandbox_id
        || Some(snapshot.home_identity.as_str()) != config.home_identity.as_deref()
        || snapshot.generation != config.generation
    {
        return Err(invalid());
    }
    Ok((snapshot, bytes))
}

pub(super) fn preview(
    config: &RuntimeConfig,
    snapshot_path: &Path,
    expected_snapshot_sha256: &str,
) -> Result<serde_json::Value> {
    let mut budget = Budget::new(Limits::default());
    let (snapshot, _) = observation(config, snapshot_path, expected_snapshot_sha256, &mut budget)?;
    let inventory = inventory::scan(config, budget.remaining_limits()?)?;
    let analysis = analyze(
        config,
        &snapshot,
        expected_snapshot_sha256,
        &inventory,
        &mut budget,
    )?;
    if inventory::scan(config, budget.remaining_limits()?)?.digest != inventory.digest {
        return Err(changed());
    }
    Ok(analysis.summary)
}

pub(super) fn analyze(
    config: &RuntimeConfig,
    snapshot: &RuntimeLedgerRetirementSnapshot,
    snapshot_sha256: &str,
    inventory: &inventory::Inventory,
    budget: &mut Budget,
) -> Result<Analysis> {
    let mut summary =
        validation::analyze_budget(config, inventory, Budget::new(budget.remaining_limits()?))?
            .summary(inventory);
    // Drop the validation indexes before allocating the retirement index.
    // Bound even deliberately repetitive input independently of file bytes.
    budget.limits.metadata_bytes = 8 * 1024 * 1024;
    let mut groups = BTreeMap::<String, Group>::new();
    let mut unclassified = 0_u64;
    for (index, entry) in inventory.entries.iter().enumerate() {
        budget.check()?;
        let root = &inventory.roots[entry.directory];
        if !matches!(root.kind, Kind::Journal | Kind::Authority) || entry.temporary {
            unclassified += 1;
            continue;
        }
        let mut scoped = config.clone();
        if root.scope != 0 {
            scoped.generation = root.scope;
        }
        let bytes = inventory.read(index, budget)?;
        budget.path(512)?;
        let current = root.scope == 0 || root.scope == config.generation;
        match root.kind {
            Kind::Journal => {
                let record = Journal::decode(&bytes, &scoped).map_err(|_| invalid())?;
                let operation = record.operation();
                let (resource, disposition) =
                    crate::mutation_authority::resource_operation(&operation);
                let group = group(&mut groups, snapshot, &operation);
                group.entries.push(index);
                group.journals += 1;
                group.current_journals += u64::from(current);
                group.resources.insert((root.scope, resource.clone()));
                group.uncertain |= record.status() != Status::Completed;
                if record.status() == Status::Completed && disposition == Disposition::Absent {
                    group.completed_absent.insert((root.scope, resource));
                }
            }
            Kind::Authority => {
                let record =
                    Authority::decode_inventory(&bytes, &scoped, &root.path.join(&entry.name))
                        .map_err(|_| invalid())?;
                let group = group(&mut groups, snapshot, &record.operation());
                group.entries.push(index);
                group.authority += 1;
                group.current_authority += u64::from(current);
                group
                    .resources
                    .insert((root.scope, record.resource.clone()));
                if record.disposition == Disposition::Absent {
                    group.authority_absent.insert((root.scope, record.resource));
                }
            }
            _ => unreachable!("filtered source kind"),
        }
    }
    // A temporary might still contain a more recent outcome. Never select any
    // group while source replacements remain, even if ordinary reconciliation
    // could settle them. This command must not perform that reconciliation.
    let temporary = inventory.entries.iter().any(|entry| entry.temporary);
    let mut candidate_journals = 0_u64;
    let mut candidate_authority = 0_u64;
    let mut candidate_groups = 0_u64;
    let mut candidate_current_journals = 0_u64;
    let mut candidate_current_authority = 0_u64;
    let mut candidates = BTreeMap::new();
    for (key, group) in groups {
        if !temporary && group.closed() {
            candidate_groups += 1;
            candidate_journals += group.journals;
            candidate_authority += group.authority;
            candidate_current_journals += group.current_journals;
            candidate_current_authority += group.current_authority;
            candidates.insert(key, group.entries);
        }
    }
    summary["format"] = "runtime-ledger-retirement-preview-v1".into();
    summary["capacity"] = inventory.capacity_summary(config.generation);
    summary["api_snapshot_sha256"] = snapshot_sha256.into();
    summary["candidate_resource_groups"] = candidate_groups.into();
    summary["candidate_journal_files"] = candidate_journals.into();
    summary["candidate_authority_files"] = candidate_authority.into();
    summary["candidate_current_journal_files"] = candidate_current_journals.into();
    summary["candidate_current_authority_files"] = candidate_current_authority.into();
    summary["retained_unclassified_files"] = unclassified.into();
    summary["source_temporaries_present"] = temporary.into();
    summary["source_mutated"] = false.into();
    summary["apply_authority"] = false.into();
    summary["stale_replay_protection"] = "required_before_archival".into();
    summary["api_observation_is_maintenance_authority"] = false.into();
    Ok(Analysis {
        summary,
        candidates,
    })
}

fn group<'a>(
    groups: &'a mut BTreeMap<String, Group>,
    snapshot: &RuntimeLedgerRetirementSnapshot,
    operation: &Operation,
) -> &'a mut Group {
    let (key, observed_terminal) = match &operation.subject {
        Subject::AddWorktree {
            workspace, name, ..
        }
        | Subject::RemoveWorktree { workspace, name } => (
            ledger_retired_worktree_key(workspace, name),
            snapshot.contains_worktree(workspace, name),
        ),
        Subject::InstallAgentSkill { request } => (
            ledger_retired_agent_key(request.agent_id.as_str()),
            snapshot.contains_agent(request.agent_id.as_str()),
        ),
        Subject::CreateAgentHome { agent_id }
        | Subject::OpenAgent { agent_id, .. }
        | Subject::CloseAgent { agent_id }
        | Subject::RemoveAgentHome { agent_id } => (
            ledger_retired_agent_key(agent_id),
            snapshot.contains_agent(agent_id),
        ),
    };
    let group = groups.entry(key).or_default();
    group.observed_terminal = observed_terminal;
    group
}
