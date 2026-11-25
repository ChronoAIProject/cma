use std::collections::BTreeMap;
use std::path::Path;

use layer3_adapter::AgentProcessAction;
use serde::{Deserialize, Serialize};

use crate::config::RuntimeConfig;
use crate::failure::{Result, RuntimeError};
use crate::mutation_journal::{self, Operation, Subject};

#[path = "mutation_authority_fold.rs"]
mod fold;
#[path = "mutation_authority_io.rs"]
mod io;

const AUTHORITY_VERSION: u32 = 1;
pub(super) const MAX_AUTHORITY_BYTES: u64 = 64 * 1024;
pub(super) const MAX_AUTHORITY_RECORDS: usize = 4_096;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum ResourceKey {
    Worktree {
        workspace: String,
        name: String,
    },
    AgentHome {
        agent_id: String,
    },
    AgentSkill {
        agent_id: String,
        skill_id: String,
    },
    AgentProcess {
        agent_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        incarnation: Option<u64>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Disposition {
    Present,
    Absent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Record {
    version: u32,
    sandbox_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    runtime_generation: Option<u64>,
    pub(super) authority_fence: u64,
    pub(super) resource: ResourceKey,
    pub(super) disposition: Disposition,
    operation_id: String,
    request_sha256: String,
    payload_sha256: String,
    subject: Subject,
}

impl Record {
    pub(crate) fn inventory_name(&self, config: &RuntimeConfig) -> std::path::PathBuf {
        io::record_path(&crate::mutation_scope::root(config, &self.subject), self)
    }

    /// Validate a preserved V1 payload without invoking the mutating scanner.
    pub(crate) fn decode_inventory(
        bytes: &[u8],
        config: &RuntimeConfig,
        path: &Path,
    ) -> Result<Self> {
        if bytes.len() as u64 > MAX_AUTHORITY_BYTES {
            return Err(corrupt("mutation authority record is oversized"));
        }
        let record = serde_json::from_slice(bytes)
            .map_err(|_| corrupt("mutation authority record is invalid"))?;
        validate_record(&record, config, path)?;
        Ok(record)
    }

    pub(super) fn new(config: &RuntimeConfig, authority_fence: u64, operation: &Operation) -> Self {
        let (resource, disposition) = resource_operation(operation);
        Self {
            version: AUTHORITY_VERSION,
            sandbox_id: config.sandbox_id.clone(),
            runtime_generation: crate::mutation_scope::generation(config, &operation.subject),
            authority_fence,
            resource,
            disposition,
            operation_id: operation.operation_id.clone(),
            request_sha256: operation.request_sha256.clone(),
            payload_sha256: operation.payload_sha256.clone(),
            subject: operation.subject.clone(),
        }
    }

    pub(crate) fn operation(&self) -> Operation {
        Operation {
            operation_id: self.operation_id.clone(),
            request_sha256: self.request_sha256.clone(),
            payload_sha256: self.payload_sha256.clone(),
            subject: self.subject.clone(),
        }
    }
}

/// Durably admit mutation authority before recovery, preflight, or resource effects.
pub(crate) fn admit(
    config: &RuntimeConfig,
    authority_fence: u64,
    operation: &Operation,
) -> Result<()> {
    admit_many(config, authority_fence, std::slice::from_ref(operation))
}

/// Reserve the entire preparation pair before admitting either original intent.
pub(crate) fn admit_many(
    config: &RuntimeConfig,
    authority_fence: u64,
    operations: &[Operation],
) -> Result<()> {
    for operation in operations {
        mutation_journal::validate_operation(config, operation)?;
    }
    if authority_fence == 0 {
        return Err(RuntimeError::rejected(
            "mutation_authority_invalid",
            "runtime mutation authority fence must be nonzero",
        ));
    }
    admit_once(config, authority_fence, operations, true)
}

fn admit_once(
    config: &RuntimeConfig,
    authority_fence: u64,
    operations: &[Operation],
    retry_race: bool,
) -> Result<()> {
    let files = files(config)?;
    let records = files
        .iter()
        .map(|file| file.record.clone())
        .collect::<Vec<_>>();
    let mut proposed = records.clone();
    let mut additions = Vec::new();
    for operation in operations {
        require_skill_parent(&proposed, operation)?;
        let incoming = Record::new(config, authority_fence, operation);
        let relevant = proposed
            .iter()
            .filter(|record| record.resource == incoming.resource)
            .collect::<Vec<_>>();
        if authorize(&relevant, &incoming)? {
            proposed.push(incoming.clone());
            additions.push(incoming);
        }
    }
    crate::mutation_capacity::check_loaded(config, &records, operations)?;
    if config.legacy_journal_recovery_enabled {
        io::clear_unpublished(config)?;
    }
    let mut files = fold::prune(files)?;
    for incoming in additions {
        // The derived folded population plus one append is reserved above.
        if files.len() + 1 >= crate::mutation_recovery::authority_limit(config) {
            return Err(crate::mutation_capacity::exhausted());
        }
        let root = crate::mutation_scope::root(config, &incoming.subject);
        match io::persist_new(&root, &incoming) {
            Ok(()) => {}
            Err(error) if retry_race && error.code == "mutation_authority_raced" => {
                return admit_once(config, authority_fence, operations, false);
            }
            Err(error) => return Err(error),
        }
        files.push(io::AuthorityFile {
            path: io::record_path(&root, &incoming),
            record: incoming,
        });
        files = fold::prune(files)?;
    }
    Ok(())
}

/// Return the current acknowledgement fence when a started operation still owns authority.
pub(crate) fn recovery_fence(config: &RuntimeConfig, operation: &Operation) -> Result<Option<u64>> {
    mutation_journal::validate_operation(config, operation)?;
    let (resource, _) = resource_operation(operation);
    let records = records(config)?;
    if require_skill_parent(&records, operation).is_err() {
        return Ok(None);
    }
    Ok(
        current(records.iter().filter(|record| record.resource == resource))
            .filter(|record| record.operation() == *operation)
            .map(|record| record.authority_fence),
    )
}

pub(crate) fn has_process_writer(config: &RuntimeConfig, key: &ResourceKey) -> Result<bool> {
    Ok(
        files_with_temporary_policy(config, io::TemporaryFiles::Refuse)?
            .iter()
            .any(|file| &file.record.resource == key),
    )
}

pub(super) fn authorize(current_records: &[&Record], incoming: &Record) -> Result<bool> {
    let Some(latest) = current(current_records.iter().copied()) else {
        return Ok(true);
    };
    if latest.disposition == Disposition::Absent && incoming.disposition == Disposition::Present {
        return Err(RuntimeError::rejected(
            "mutation_authority_retired",
            "resource cleanup permanently dominates later create authority",
        ));
    }
    if incoming.authority_fence < latest.authority_fence {
        return Err(RuntimeError::rejected(
            "mutation_authority_stale",
            "runtime mutation authority fence is stale",
        ));
    }
    if skill_sequence(&incoming.subject)
        .zip(skill_sequence(&latest.subject))
        .is_some_and(|(next, previous)| next > previous)
        && incoming.authority_fence > latest.authority_fence
    {
        return Ok(true);
    }
    let same_disposition = current_records
        .iter()
        .filter(|record| record.disposition == incoming.disposition)
        .collect::<Vec<_>>();
    if let Some(identity) = same_disposition.first() {
        if identity.operation() != incoming.operation() {
            return Err(RuntimeError::rejected(
                "mutation_authority_conflict",
                "resource authority changed operation identity",
            ));
        }
    }
    if incoming.disposition == Disposition::Present
        && latest.disposition == Disposition::Present
        && latest.operation() != incoming.operation()
    {
        return Err(RuntimeError::rejected(
            "mutation_authority_conflict",
            "resource create authority changed operation identity",
        ));
    }
    Ok(!current_records.iter().any(|record| {
        record.authority_fence == incoming.authority_fence
            && record.disposition == incoming.disposition
            && record.operation() == incoming.operation()
    }))
}

pub(super) fn current<'a>(records: impl Iterator<Item = &'a Record>) -> Option<&'a Record> {
    records.max_by_key(|record| {
        (
            record.authority_fence,
            record.disposition == Disposition::Absent,
        )
    })
}

pub(crate) fn records(config: &RuntimeConfig) -> Result<Vec<Record>> {
    files(config).map(|files| files.into_iter().map(|file| file.record).collect())
}

fn files(config: &RuntimeConfig) -> Result<Vec<io::AuthorityFile>> {
    let temporary = if config.legacy_journal_recovery_enabled {
        io::TemporaryFiles::Preserve
    } else {
        io::TemporaryFiles::Remove
    };
    files_with_temporary_policy(config, temporary)
}

fn files_with_temporary_policy(
    config: &RuntimeConfig,
    temporary: io::TemporaryFiles,
) -> Result<Vec<io::AuthorityFile>> {
    let mut files = Vec::new();
    let mut budget = crate::mutation_recovery::ScanBudget::new(
        config,
        crate::mutation_recovery::authority_limit(config),
    );
    for root in crate::mutation_scope::roots(config)? {
        files.extend(io::scan(&root, &mut budget, temporary)?);
    }
    let mut grouped = BTreeMap::<ResourceKey, Vec<&Record>>::new();
    for file in &files {
        validate_record(&file.record, config, &file.path)?;
        grouped
            .entry(file.record.resource.clone())
            .or_default()
            .push(&file.record);
    }
    for history in grouped.values() {
        validate_history(history)?;
    }
    Ok(files)
}

fn validate_record(record: &Record, config: &RuntimeConfig, path: &Path) -> Result<()> {
    let operation = record.operation();
    let (resource, disposition) = resource_operation(&operation);
    if record.version != AUTHORITY_VERSION
        || record.sandbox_id != config.sandbox_id
        || record.runtime_generation != crate::mutation_scope::generation(config, &record.subject)
        || record.authority_fence == 0
        || record.resource != resource
        || record.disposition != disposition
        || mutation_journal::validate_operation(config, &operation).is_err()
        || path != record.inventory_name(config)
    {
        return Err(corrupt("runtime mutation authority record is invalid"));
    }
    Ok(())
}

pub(super) fn validate_history(records: &[&Record]) -> Result<()> {
    let present = records
        .iter()
        .filter(|record| record.disposition == Disposition::Present)
        .collect::<Vec<_>>();
    let absent = records
        .iter()
        .filter(|record| record.disposition == Disposition::Absent)
        .collect::<Vec<_>>();
    if !(same_identity(&present) || progressive_skills(&present)) || !same_identity(&absent) {
        return Err(corrupt(
            "runtime mutation authority has conflicting operation identities",
        ));
    }
    if let Some(first_absent) = absent.iter().map(|record| record.authority_fence).min() {
        if present
            .iter()
            .any(|record| record.authority_fence > first_absent)
        {
            return Err(corrupt(
                "runtime mutation authority recreates a retired resource",
            ));
        }
    }
    Ok(())
}

fn skill_sequence(subject: &Subject) -> Option<u64> {
    match subject {
        Subject::InstallAgentSkill { request } => Some(request.sequence()),
        _ => None,
    }
}
fn progressive_skills(records: &[&&Record]) -> bool {
    let mut ordered = records.to_vec();
    ordered.sort_by_key(|r| r.authority_fence);
    ordered.iter().all(|r| skill_sequence(&r.subject).is_some())
        && ordered.windows(2).all(|pair| {
            let a = pair[0];
            let b = pair[1];
            a.operation() == b.operation()
                || (a.authority_fence < b.authority_fence
                    && skill_sequence(&a.subject) < skill_sequence(&b.subject))
        })
}

fn same_identity(records: &[&&Record]) -> bool {
    records.first().is_none_or(|first| {
        records
            .iter()
            .all(|record| record.operation() == first.operation())
    })
}

pub(crate) fn resource_operation(operation: &Operation) -> (ResourceKey, Disposition) {
    match &operation.subject {
        Subject::AddWorktree {
            workspace, name, ..
        } => (
            ResourceKey::Worktree {
                workspace: workspace.clone(),
                name: name.clone(),
            },
            Disposition::Present,
        ),
        Subject::RemoveWorktree { workspace, name } => (
            ResourceKey::Worktree {
                workspace: workspace.clone(),
                name: name.clone(),
            },
            Disposition::Absent,
        ),
        Subject::InstallAgentSkill { request } => (
            ResourceKey::AgentSkill {
                agent_id: request.agent_id.as_str().into(),
                skill_id: request.skill.id.clone(),
            },
            Disposition::Present,
        ),
        Subject::CreateAgentHome { agent_id } => (
            ResourceKey::AgentHome {
                agent_id: agent_id.clone(),
            },
            Disposition::Present,
        ),
        Subject::RemoveAgentHome { agent_id } => (
            ResourceKey::AgentHome {
                agent_id: agent_id.clone(),
            },
            Disposition::Absent,
        ),
        Subject::OpenAgent { agent_id, .. } => (
            process_resource(agent_id, operation, AgentProcessAction::Open),
            Disposition::Present,
        ),
        Subject::CloseAgent { agent_id } => (
            process_resource(agent_id, operation, AgentProcessAction::Close),
            Disposition::Absent,
        ),
    }
}

fn process_resource(
    agent_id: &str,
    operation: &Operation,
    action: AgentProcessAction,
) -> ResourceKey {
    ResourceKey::AgentProcess {
        agent_id: agent_id.into(),
        incarnation: action
            .incarnation(agent_id, &operation.operation_id)
            .filter(|n| *n > 1),
    }
}

pub(super) fn corrupt(detail: &str) -> RuntimeError {
    RuntimeError::rejected("mutation_authority_corrupt", detail)
}

pub(super) fn io_error(operation: &'static str, error: std::io::Error) -> RuntimeError {
    RuntimeError::unavailable(
        "mutation_authority_unavailable",
        format!("{operation}: {error}"),
    )
}

#[cfg(test)]
#[path = "mutation_authority_tests.rs"]
mod tests;

pub(super) fn require_skill_parent(records: &[Record], operation: &Operation) -> Result<()> {
    let Subject::InstallAgentSkill { request } = &operation.subject else {
        return Ok(());
    };
    if records.iter().any(|record| matches!(&record.resource, ResourceKey::AgentHome { agent_id } if agent_id == request.agent_id.as_str()) && record.disposition == Disposition::Absent) {
        return Err(RuntimeError::rejected("mutation_authority_retired", "agent home cleanup dominates skill installation"));
    }
    Ok(())
}

#[cfg(test)]
#[path = "mutation_authority_capacity_tests.rs"]
mod capacity_tests;
