//! Classify durable inventory in bounded pages before charging personal capacity.
use super::{Context, Error, Set, PAGE};
use crate::store::durability::majority_collection;
use futures_util::TryStreamExt;
use layer2_core::account_deletion::ErasureInventory;
use mongodb::{
    bson::{doc, Document},
    Database,
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Default)]
struct Candidate {
    foreign: bool,
    roots: BTreeSet<String>,
    lineage: BTreeSet<(Set, String)>,
}

pub(super) async fn merge(
    db: &Database,
    context: &mut Context,
    inventory: &ErasureInventory,
) -> Result<bool, Error> {
    let mut changed = false;
    for (set, ids) in [
        (Set::Sandboxes, &inventory.sandboxes),
        (Set::Workspaces, &inventory.workspaces),
        (Set::Worktrees, &inventory.worktrees),
        (Set::Agents, &inventory.agents),
        (Set::Launches, &inventory.launches),
        (Set::Bindings, &inventory.bindings),
        (Set::Triggers, &inventory.triggers),
        (Set::Documents, &inventory.documents),
    ] {
        for page in ids.chunks(PAGE as usize) {
            let ids: BTreeSet<_> = page
                .iter()
                .filter(|id| {
                    !context.protected.contains(*id)
                        && !context
                            .sets
                            .get(&set)
                            .is_some_and(|known| known.contains(*id))
                })
                .map(String::as_str)
                .collect();
            if ids.is_empty() {
                continue;
            }
            for id in &ids {
                if id.is_empty() || id.len() > 4096 {
                    return Err(Error::InvalidRecord("historical resource identity")
                        .attribute(&context.subject));
                }
            }
            classify(db, context, set, &ids).await?;
            for id in ids {
                if !context.protected.contains(id) {
                    context.add(set, id)?;
                    changed = true;
                }
            }
            context.bounds()?;
        }
    }
    Ok(changed)
}

async fn classify(
    db: &Database,
    context: &mut Context,
    set: Set,
    ids: &BTreeSet<&str>,
) -> Result<(), Error> {
    let (name, owner) = match set {
        Set::Sandboxes => ("sandboxes", "owner"),
        Set::Workspaces => ("workspaces", "owner"),
        Set::Worktrees => ("worktrees", "owner"),
        Set::Agents => ("agents", "owner"),
        Set::Launches => ("trigger_launches", "owner"),
        Set::Bindings => ("provider_bindings", "record.owner"),
        Set::Triggers => ("trigger_definitions", "owner"),
        Set::Documents => ("cma_document_artifacts", "owner_subject"),
        _ => return Err(Error::InvalidRecord("inventory class").attribute(&context.subject)),
    };
    let ids: Vec<_> = ids.iter().copied().collect();
    let mut candidates: BTreeMap<_, _> = ids
        .iter()
        .map(|id| ((*id).to_owned(), Candidate::default()))
        .collect();
    if set == Set::Sandboxes {
        for (id, candidate) in &mut candidates {
            candidate.roots.insert(id.clone());
        }
    }
    let mut present = BTreeSet::new();
    let mut rows = majority_collection::<Document>(db, name)
        .find(doc! {"_id":{"$in":&ids}})
        .projection(projection())
        .batch_size(PAGE as u32)
        .await?;
    while let Some(row) = rows.try_next().await? {
        let id = row
            .get_str("_id")
            .map_err(|_| Error::InvalidRecord(name).attribute(&context.subject))?;
        present.insert(id.to_owned());
        if let Some(candidate) = candidates.get_mut(id) {
            observe(candidate, &row, owner, &context.subject);
            if set == Set::Documents {
                candidate.foreign |= strings(&row, "audit_owner_subject")
                    .iter()
                    .any(|owner| *owner != context.subject);
            }
            if matches!(set, Set::Launches | Set::Bindings) && strings(&row, owner).is_empty() {
                candidate.foreign = true;
            }
        }
    }
    let sidecar = match set {
        Set::Agents if present.len() < ids.len() => Some((
            "agent_creation_provenance",
            doc! {"_id":{"$in":ids.iter().filter(|id| !present.contains(**id)).copied().collect::<Vec<_>>()}},
            "record.owner.subject",
        )),
        Set::Launches => Some((
            "trigger_launch_input_protocols",
            doc! {"_id":{"$in":&ids}},
            "owner",
        )),
        Set::Bindings => Some((
            "binding_conversations",
            doc! {"$or":[{"binding_id":{"$in":&ids}},{"record.binding_id":{"$in":&ids}}]},
            "record.owner",
        )),
        _ => None,
    };
    if let Some((name, filter, owner)) = sidecar {
        let mut rows = majority_collection::<Document>(db, name)
            .find(filter)
            .projection(projection())
            .batch_size(PAGE as u32)
            .await?;
        while let Some(row) = rows.try_next().await? {
            let keys = if set == Set::Bindings {
                ["binding_id", "record.binding_id"]
                    .into_iter()
                    .flat_map(|path| strings(&row, path))
                    .collect::<Vec<_>>()
            } else {
                strings(&row, "_id")
            };
            for id in keys {
                if let Some(candidate) = candidates.get_mut(id) {
                    observe(candidate, &row, owner, &context.subject);
                    candidate.foreign |= super::value(&row, "record.owner.kind")
                        .and_then(|value| value.as_str())
                        == Some("organization");
                }
            }
        }
    }
    protect_roots(db, context, &candidates).await?;
    for (id, candidate) in candidates {
        if candidate.foreign
            || candidate
                .roots
                .iter()
                .any(|id| context.protected.contains(id))
        {
            context.protect(set, &id);
            for (kind, id) in candidate.lineage {
                context.protect(kind, &id);
            }
        }
    }
    Ok(())
}

fn strings<'a>(row: &'a Document, path: &str) -> Vec<&'a str> {
    super::super::selectors::strings(row, path)
}

fn observe(candidate: &mut Candidate, row: &Document, owner: &str, subject: &str) {
    candidate.foreign |= strings(row, owner).iter().any(|owner| *owner != subject);
    for (set, path) in [
        (Set::Sandboxes, "sandbox_id"),
        (Set::Sandboxes, "record.sandbox_id"),
        (Set::Sandboxes, "record.resolution.sandbox.id"),
        (Set::Sandboxes, "record.previous.sandbox_id"),
        (Set::Workspaces, "workspace_id"),
        (Set::Workspaces, "record.workspace_id"),
        (Set::Worktrees, "worktree_id"),
        (Set::Worktrees, "record.worktree_id"),
        (Set::Agents, "agent_id"),
        (Set::Agents, "record.agent_id"),
    ] {
        for id in strings(row, path) {
            candidate.lineage.insert((set, id.to_owned()));
            if set == Set::Sandboxes {
                candidate.roots.insert(id.to_owned());
            }
        }
    }
}

async fn protect_roots(
    db: &Database,
    context: &mut Context,
    candidates: &BTreeMap<String, Candidate>,
) -> Result<(), Error> {
    let ids: BTreeSet<_> = candidates
        .values()
        .flat_map(|candidate| &candidate.roots)
        .filter(|id| !context.protected.contains(*id))
        .collect();
    if ids.is_empty() {
        return Ok(());
    }
    let ids: Vec<_> = ids.into_iter().collect();
    for (name, owner) in [
        ("resource_policies", "owner_subject"),
        ("sandboxes", "owner"),
    ] {
        let mut rows = majority_collection::<Document>(db, name)
            .find(doc! {"_id":{"$in":&ids}})
            .projection(doc! {"owner":1,"owner_kind":1,"owner_subject":1})
            .batch_size(PAGE as u32)
            .await?;
        while let Some(row) = rows.try_next().await? {
            let id = row
                .get_str("_id")
                .map_err(|_| Error::InvalidRecord(name).attribute(&context.subject))?;
            let organization = row.get_str("owner_kind").ok() == Some("organization");
            if organization
                || strings(&row, owner)
                    .iter()
                    .any(|owner| *owner != context.subject)
            {
                context.protect(Set::Sandboxes, id);
                if organization {
                    context.organization_sandboxes.insert(id.to_owned());
                }
            }
        }
    }
    Ok(())
}

fn projection() -> Document {
    doc! {"owner":1,"owner_subject":1,"audit_owner_subject":1,"record.owner":1,"sandbox_id":1,
    "record.sandbox_id":1,"record.resolution.sandbox.id":1,"record.previous.sandbox_id":1,
    "workspace_id":1,"record.workspace_id":1,"worktree_id":1,"record.worktree_id":1,
    "agent_id":1,"record.agent_id":1,"binding_id":1,"record.binding_id":1}
}
