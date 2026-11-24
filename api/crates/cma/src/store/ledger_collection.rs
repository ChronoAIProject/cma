//! Bounded, indexed authority reads for active runtime group candidates.
//! No initializer, DDL, history read, retention write or effect authority here.
use crate::store::transaction_terminal::GuardedTerminal as _;
use std::{collections::BTreeMap, time::Duration};

use layer2_core::sandbox::{Agent, SandboxHierarchyRecord, Workspace, Worktree};
use layer3_adapter::runtime_ledger_collection::{CollectionObservation, CollectionRequest};
use mongodb::{bson::Document, Client, ClientSession, Database};

#[path = "ledger_collection_reads.rs"]
mod reads;
use reads::ReadBudget;
#[path = "ledger_collection_command.rs"]
mod command;
pub(crate) use command::command;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(crate) enum CollectionReadRefusal {
    #[error("bounded ledger collection evidence is invalid")]
    InvalidEvidence,
    #[error("bounded ledger collection eligibility is unknown")]
    UnknownEligibility,
    #[error("bounded ledger collection authority changed")]
    StaleAuthority,
    #[error("bounded ledger collection read budget is exhausted")]
    BudgetExceeded,
    #[error("bounded ledger collection authority is unavailable")]
    AuthorityUnavailable,
}
use CollectionReadRefusal::{
    AuthorityUnavailable, InvalidEvidence, StaleAuthority, UnknownEligibility,
};
pub(super) type Result<T> = std::result::Result<T, CollectionReadRefusal>;

/// The caller supplies runtime candidates, never a scan of historical Agents.
/// Abort the read-only snapshot even on success; drop cancels it on timeout.
pub(crate) async fn observe(
    client: &Client,
    database: &Database,
    request: &CollectionRequest,
) -> Result<CollectionObservation> {
    if !request.validate() {
        return Err(InvalidEvidence);
    }
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut session = client
            .start_session()
            .await
            .map_err(|_| AuthorityUnavailable)?;
        super::sandbox_mongo::start_transaction(&mut session)
            .await
            .map_err(|_| AuthorityUnavailable)?;
        let result = read(database, request, &mut session).await;
        session
            .abort_guarded()
            .await
            .map_err(|_| AuthorityUnavailable)?;
        result
    })
    .await
    .map_err(|_| AuthorityUnavailable)?
}

async fn read(
    database: &Database,
    request: &CollectionRequest,
    session: &mut ClientSession,
) -> Result<CollectionObservation> {
    use layer3_adapter::runtime_ledger_collection::CollectionGroup;
    let mut budget = ReadBudget::new(database, session, request);
    let parent = budget.one("sandboxes", &request.binding.sandbox_id).await?;
    let sandbox = super::sandboxes::read_sandbox(&parent).map_err(|_| UnknownEligibility)?;
    if sandbox.id.as_str() != request.binding.sandbox_id
        || sandbox.revision != request.binding.sandbox_revision
        || sandbox.retained_runtime_binding().is_none_or(|runtime| {
            runtime.boot_id.as_ref() != Some(&request.binding.boot_id)
                || runtime.provision_generation != Some(request.binding.generation)
        })
        || sandbox
            .lifecycle
            .home
            .as_ref()
            .and_then(|home| home.attachment())
            .and_then(|home| home.identity().ok())
            .as_deref()
            != Some(request.binding.home_identity.as_str())
    {
        return Err(StaleAuthority);
    }
    let mut closure = Dependencies::default();
    for group in &request.groups {
        let (workspace, worktree) = match group {
            CollectionGroup::Agent { agent_id } => {
                let document = budget.one("agents", agent_id).await?;
                let agent = read_agent(&document)?;
                let document = budget.one("worktrees", agent.worktree_id.as_str()).await?;
                let worktree =
                    super::worktrees::read_worktree(&document).map_err(|_| UnknownEligibility)?;
                let document = budget
                    .one("workspaces", worktree.workspace_id.as_str())
                    .await?;
                let workspace =
                    super::workspaces::read_workspace(&document).map_err(|_| UnknownEligibility)?;
                closure.agent(agent)?;
                (workspace, worktree)
            }
            CollectionGroup::Worktree { workspace, name } => {
                budget
                    .named_worktree(&request.binding.sandbox_id, workspace, name)
                    .await?
            }
        };
        // Resolve both physical names for Agent candidates too. A corrupt or
        // duplicate parent cannot hide behind a correct id lookup.
        let (named_workspace, named_worktree) = budget
            .named_worktree(&request.binding.sandbox_id, &workspace.name, &worktree.name)
            .await?;
        if workspace != named_workspace || worktree != named_worktree {
            return Err(UnknownEligibility);
        }
        for document in budget.agents_for_worktree(worktree.id.as_str()).await? {
            closure.agent(read_agent(&document)?)?;
        }
        closure.workspace(workspace)?;
        closure.worktree(worktree)?;
    }
    let (digest, count, bytes) = budget.finish();
    SandboxHierarchyRecord {
        sandbox,
        workspaces: closure.workspaces.into_values().collect(),
        worktrees: closure.worktrees.into_values().collect(),
        agents: closure.agents.into_values().collect(),
    }
    .ledger_collection_observation(request, layer2_core::now_rfc3339(), digest, count, bytes)
    .map_err(|_| UnknownEligibility)
}

fn read_agent(document: &Document) -> Result<Agent> {
    if document.contains_key(super::agent_log_integrity::QUARANTINE_FIELD) {
        return Err(UnknownEligibility);
    }
    super::agents::read_agent(document).map_err(|_| UnknownEligibility)
}

#[derive(Default)]
struct Dependencies {
    workspaces: BTreeMap<String, Workspace>,
    worktrees: BTreeMap<String, Worktree>,
    agents: BTreeMap<String, Agent>,
}

impl Dependencies {
    fn workspace(&mut self, value: Workspace) -> Result<()> {
        insert(&mut self.workspaces, value.id.to_string(), value)
    }
    fn worktree(&mut self, value: Worktree) -> Result<()> {
        insert(&mut self.worktrees, value.id.to_string(), value)
    }
    fn agent(&mut self, value: Agent) -> Result<()> {
        insert(&mut self.agents, value.id.to_string(), value)
    }
}

fn insert<T: PartialEq>(map: &mut BTreeMap<String, T>, key: String, value: T) -> Result<()> {
    if map.get(&key).is_some_and(|prior| prior != &value) {
        return Err(UnknownEligibility);
    }
    map.insert(key, value);
    Ok(())
}
