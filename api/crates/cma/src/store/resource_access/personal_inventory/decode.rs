//! Complete source decoders are reused once per resource per snapshot.
use super::*;
use crate::store::{
    agents::read_agent, resource_access::records, sandboxes::read_sandbox,
    workspaces::read_workspace, worktrees::read_worktree,
};
use layer2_core::resource_access::{ResourceLineage, ResourceSelector, ResourceTarget};
use std::collections::BTreeMap;

#[derive(Clone, Copy)]
pub(super) enum Kind {
    Agent,
    Worktree,
    Workspace,
    Sandbox,
    Policy,
    Share,
}
impl Kind {
    pub fn parse(value: &str) -> Result<Self, ResourceAccessStoreError> {
        match value {
            "agent" => Ok(Self::Agent),
            "worktree" => Ok(Self::Worktree),
            "workspace" => Ok(Self::Workspace),
            "sandbox" => Ok(Self::Sandbox),
            "policy" => Ok(Self::Policy),
            "share" => Ok(Self::Share),
            _ => Err(ResourceAccessStoreError::Corrupt),
        }
    }
}
#[derive(Default)]
pub(super) struct Sources {
    agents: BTreeMap<String, Agent>,
    trees: BTreeMap<String, Document>,
    workspaces: BTreeMap<String, Document>,
    sandboxes: BTreeMap<String, Document>,
    policies: BTreeMap<String, Document>,
    shares: BTreeMap<String, Document>,
}
impl Sources {
    pub fn captured(agents: &[Agent]) -> Self {
        Self {
            agents: agents
                .iter()
                .map(|agent| (agent.id.to_string(), agent.clone()))
                .collect(),
            ..Self::default()
        }
    }

    pub fn insert(
        &mut self,
        kind: Kind,
        document: Document,
        max_rows: usize,
    ) -> Result<(), ResourceAccessStoreError> {
        let id = document
            .get_str("_id")
            .map_err(|_| ResourceAccessStoreError::Corrupt)?
            .to_owned();
        let (map, limit) = match kind {
            Kind::Agent => {
                let agent = tracing::debug_span!(target: "cma.agent_inventory", "record_decode")
                    .in_scope(|| read_agent(&document))
                    .map_err(|_| ResourceAccessStoreError::Corrupt)?;
                if self.agents.insert(id, agent).is_some() || self.agents.len() > max_rows {
                    return Err(ResourceAccessStoreError::Corrupt);
                }
                return Ok(());
            }
            Kind::Worktree => (&mut self.trees, max_rows),
            Kind::Workspace => (&mut self.workspaces, max_rows),
            Kind::Sandbox => (&mut self.sandboxes, max_rows),
            Kind::Policy => (&mut self.policies, max_rows),
            Kind::Share => (
                &mut self.shares,
                max_rows * (MAX_ACTIVE_RESOURCE_SHARES + 1),
            ),
        };
        if map.insert(id, document).is_some() || map.len() > limit {
            return Err(ResourceAccessStoreError::Corrupt);
        }
        Ok(())
    }
    #[tracing::instrument(
        level = "debug",
        target = "cma.agent_inventory",
        name = "record_decode",
        skip_all
    )]
    pub fn page(self) -> Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError> {
        let Sources {
            agents,
            trees: source_trees,
            workspaces: source_workspaces,
            sandboxes: source_sandboxes,
            policies: source_policies,
            shares,
        } = self;
        let mut sandbox_shares: BTreeMap<String, Vec<_>> = BTreeMap::new();
        let mut agent_shares: BTreeMap<(String, String), Vec<_>> = BTreeMap::new();
        for document in shares.values() {
            let parent = document
                .get_str("sandbox_id")
                .map_err(|_| ResourceAccessStoreError::Corrupt)?;
            let share = records::read_share(&layer2_core::SandboxId(parent.into()), document)?;
            match &share.selector {
                ResourceSelector::Sandbox { sandbox_id } if sandbox_id.as_str() == parent => {
                    sandbox_shares.entry(parent.into()).or_default().push(share);
                }
                ResourceSelector::Agent { agent_id } => {
                    agent_shares
                        .entry((parent.into(), agent_id.to_string()))
                        .or_default()
                        .push(share);
                }
                _ => return Err(ResourceAccessStoreError::Corrupt),
            }
        }
        let mut rows = Vec::new();
        let mut trees = BTreeMap::new();
        let mut workspaces = BTreeMap::new();
        let mut sandboxes = BTreeMap::new();
        let mut policies = BTreeMap::new();
        for (_, agent) in agents {
            let tree = cached(&mut trees, &source_trees, agent.worktree_id.as_str(), |d| {
                read_worktree(d).map_err(|_| ResourceAccessStoreError::Corrupt)
            })?;
            let workspace = cached(
                &mut workspaces,
                &source_workspaces,
                agent.workspace_id.as_str(),
                |d| read_workspace(d).map_err(|_| ResourceAccessStoreError::Corrupt),
            )?;
            let sandbox = cached(
                &mut sandboxes,
                &source_sandboxes,
                agent.sandbox_id.as_str(),
                |d| read_sandbox(d).map_err(|_| ResourceAccessStoreError::Corrupt),
            )?;
            let policy = source_policies
                .get(agent.sandbox_id.as_str())
                .map(|_| {
                    cached(
                        &mut policies,
                        &source_policies,
                        agent.sandbox_id.as_str(),
                        records::read_policy,
                    )
                })
                .transpose()?;
            let mut shares = sandbox_shares
                .get(agent.sandbox_id.as_str())
                .cloned()
                .unwrap_or_default();
            shares.extend(
                agent_shares
                    .get(&(agent.sandbox_id.to_string(), agent.id.to_string()))
                    .into_iter()
                    .flatten()
                    .cloned(),
            );
            shares.sort_by(|a, b| a.id.cmp(&b.id));
            let target = ResourceTarget::Agent {
                agent_id: agent.id.clone(),
            };
            let snapshot = ResourceAccessSnapshot {
                lineage: ResourceLineage {
                    agent: Some(agent),
                    worktree: Some(tree),
                    workspace: Some(workspace),
                    sandbox,
                },
                policy,
                shares,
            };
            snapshot.scope(&target)?;
            rows.push(snapshot);
        }
        Ok(rows)
    }
}
fn cached<T: Clone>(
    cache: &mut BTreeMap<String, T>,
    source: &BTreeMap<String, Document>,
    id: &str,
    decode: impl FnOnce(&Document) -> Result<T, ResourceAccessStoreError>,
) -> Result<T, ResourceAccessStoreError> {
    if let Some(record) = cache.get(id) {
        return Ok(record.clone());
    }
    let record = decode(source.get(id).ok_or(ResourceAccessStoreError::Corrupt)?)?;
    cache.insert(id.to_owned(), record.clone());
    Ok(record)
}
