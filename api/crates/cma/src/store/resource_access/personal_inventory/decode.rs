//! Complete source decoders are reused once per resource per snapshot.
use super::*;
use crate::store::{
    agents::read_agent, resource_access::records, sandboxes::read_sandbox,
    workspaces::read_workspace, worktrees::read_worktree,
};
use layer2_core::resource_access::{ResourceLineage, ResourceTarget};
use std::collections::BTreeMap;

#[derive(Clone, Copy)]
pub(super) enum Kind {
    Agent,
    Worktree,
    Workspace,
    Sandbox,
    Policy,
    Share,
    ShareTargets,
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
            "share_targets" => Ok(Self::ShareTargets),
            _ => Err(ResourceAccessStoreError::Corrupt),
        }
    }
}
#[derive(Default)]
pub(super) struct Sources {
    agents: BTreeMap<String, Document>,
    trees: BTreeMap<String, Document>,
    workspaces: BTreeMap<String, Document>,
    sandboxes: BTreeMap<String, Document>,
    policies: BTreeMap<String, Document>,
    shares: BTreeMap<String, Document>,
    share_targets: BTreeMap<String, Document>,
}
impl Sources {
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
            Kind::Agent => (&mut self.agents, max_rows),
            Kind::Worktree => (&mut self.trees, max_rows),
            Kind::Workspace => (&mut self.workspaces, max_rows),
            Kind::Sandbox => (&mut self.sandboxes, max_rows),
            Kind::Policy => (&mut self.policies, max_rows),
            Kind::ShareTargets => (
                &mut self.share_targets,
                max_rows * (MAX_ACTIVE_RESOURCE_SHARES + 1),
            ),
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
    pub fn page(self) -> Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError> {
        let mut share_ids: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        if self.share_targets.len() != self.shares.len() {
            return Err(ResourceAccessStoreError::Corrupt);
        }
        for (id, targets) in &self.share_targets {
            if !self.shares.contains_key(id) {
                return Err(ResourceAccessStoreError::Corrupt);
            }
            let targets = targets
                .get_array("agents")
                .map_err(|_| ResourceAccessStoreError::Corrupt)?;
            if targets.is_empty() || targets.len() > self.agents.len() {
                return Err(ResourceAccessStoreError::Corrupt);
            }
            for target in targets {
                let target = target.as_str().ok_or(ResourceAccessStoreError::Corrupt)?;
                if !self.agents.contains_key(target) {
                    return Err(ResourceAccessStoreError::Corrupt);
                }
                share_ids.entry(target).or_default().push(id);
            }
        }
        let mut rows = Vec::new();
        let mut trees = BTreeMap::new();
        let mut workspaces = BTreeMap::new();
        let mut sandboxes = BTreeMap::new();
        let mut policies = BTreeMap::new();
        let mut decoded_shares = BTreeMap::new();
        for document in self.agents.values() {
            let agent = read_agent(document).map_err(|_| ResourceAccessStoreError::Corrupt)?;
            let tree = cached(&mut trees, &self.trees, agent.worktree_id.as_str(), |d| {
                read_worktree(d).map_err(|_| ResourceAccessStoreError::Corrupt)
            })?;
            let workspace = cached(
                &mut workspaces,
                &self.workspaces,
                agent.workspace_id.as_str(),
                |d| read_workspace(d).map_err(|_| ResourceAccessStoreError::Corrupt),
            )?;
            let sandbox = cached(
                &mut sandboxes,
                &self.sandboxes,
                agent.sandbox_id.as_str(),
                |d| read_sandbox(d).map_err(|_| ResourceAccessStoreError::Corrupt),
            )?;
            let policy = self
                .policies
                .get(agent.sandbox_id.as_str())
                .map(|_| {
                    cached(
                        &mut policies,
                        &self.policies,
                        agent.sandbox_id.as_str(),
                        records::read_policy,
                    )
                })
                .transpose()?;
            let mut shares = Vec::new();
            for id in share_ids.get(agent.id.as_str()).into_iter().flatten() {
                // Cache by both parent and share identity: malformed projections
                // that match multiple parents cannot reuse another parent's proof.
                let key = (agent.sandbox_id.clone(), (*id).to_owned());
                let share = match decoded_shares.get(&key) {
                    Some(share) => share,
                    None => {
                        let share = records::read_share(&agent.sandbox_id, &self.shares[*id])?;
                        decoded_shares.entry(key).or_insert(share)
                    }
                };
                shares.push(share.clone());
            }
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
