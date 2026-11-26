//! Audited durable ownership sources that outlive ordinary Sandbox purges.
use super::{Context, Error, Set, PAGE};
use futures_util::TryStreamExt;
use mongodb::{
    bson::{doc, Document},
    Database,
};

#[path = "history_activity.rs"]
mod activity;

struct Source {
    collection: &'static str,
    owner: &'static str,
    ids: &'static [(Set, &'static str)],
}
macro_rules! source {
    ($name:literal, $owner:literal, $($set:ident => $path:literal),+ $(,)?) => {
        Source { collection: $name, owner: $owner, ids: &[$((Set::$set, $path)),+] }
    };
}
const SOURCES: &[Source] = &[
    source!("resource_policies", "owner_subject", Sandboxes => "_id"),
    source!("sandbox_usage", "owner", Sandboxes => "_id"),
    source!("sandbox_usage_events", "owner", Sandboxes => "sandbox_id"),
    source!("sandbox_billing_state", "owner_id", Sandboxes => "_id"),
    source!("agent_creation_provenance", "record.owner.subject", Agents => "_id",
        Sandboxes => "record.sandbox_id", Workspaces => "record.workspace_id", Worktrees => "record.worktree_id"),
    source!("agent_lifecycle_operations", "owner", Sandboxes => "sandbox_id", Agents => "agent_id",
        Sandboxes => "input.request.sandbox_id", Workspaces => "input.request.workspace_id"),
    source!("sandbox_metric_bindings", "owner", Sandboxes => "sandbox_id"),
    source!("resource_shares", "record.owner", Sandboxes => "sandbox_id", Agents => "record.selector.agent_id"),
    source!("resource_shares", "record.owner.subject", Sandboxes => "sandbox_id", Agents => "record.selector.agent_id"),
    source!("trigger_launch_input_protocols", "owner", Launches => "_id"),
    source!("agent_reuse_bindings", "owner", Sandboxes => "sandbox_id", Agents => "agent_id"),
    source!("provider_bindings", "record.owner", Bindings => "_id", Sandboxes => "record.sandbox_id",
        Workspaces => "record.workspace_id", Agents => "record.existing_agent.agent_id",
        Worktrees => "record.existing_agent.worktree_id", Sandboxes => "record.previous.sandbox_id",
        Workspaces => "record.previous.workspace_id"),
    source!("binding_conversations", "record.owner", Bindings => "record.binding_id", Sandboxes => "record.sandbox_id",
        Workspaces => "record.workspace_id", Agents => "record.agent_id", Worktrees => "record.worktree_id"),
    source!("provider_binding_preparations", "owner", Bindings => "binding_id",
        Sandboxes => "record.previous.sandbox_id", Workspaces => "record.previous.workspace_id",
        Agents => "record.previous.existing_agent.agent_id", Worktrees => "record.previous.existing_agent.worktree_id",
        Sandboxes => "record.reserved.sandbox_id", Workspaces => "record.reserved.workspace_id",
        Agents => "record.reserved.existing_agent.agent_id", Worktrees => "record.reserved.existing_agent.worktree_id"),
    source!("agents", "owner", Agents => "_id", Sandboxes => "sandbox_id", Workspaces => "workspace_id", Worktrees => "worktree_id"),
    source!("workspaces", "owner", Workspaces => "_id", Sandboxes => "sandbox_id"),
    source!("worktrees", "owner", Worktrees => "_id", Sandboxes => "sandbox_id", Workspaces => "workspace_id"),
    source!("trigger_launches", "owner", Launches => "_id", Sandboxes => "record.resolution.sandbox.id",
        Workspaces => "record.resolution.workspace.id", Agents => "record.resolution.agent.id"),
    source!("native_prepared_sandboxes", "record.ownership.owner", Sandboxes => "record.payload.id",
        Workspaces => "record.payload.workspaces.id", Agents => "record.payload.workspaces.agents.id",
        Worktrees => "record.payload.workspaces.agents.worktree_id"),
    source!("sandbox_usage_report_control", "record.owner_subject", Sandboxes => "record.sandbox_id"),
    source!("service_connection_handoffs", "owner", Launches => "launch_id"),
    source!("trigger_definitions", "owner", Triggers => "_id"),
    source!("cma_document_artifacts", "owner_subject", Documents => "_id"),
    source!("cma_document_artifacts", "audit_owner_subject", Documents => "_id"),
    source!("agent_profiles", "owner", AgentProfiles => "_id"),
    source!("workspace_profiles", "owner", WorkspaceProfiles => "_id"),
    source!("sandbox_profiles", "owner", SandboxProfiles => "_id"),
    source!("sandbox_usage_reports", "principal", Reports => "_id"),
];

pub(super) async fn discover(
    db: &Database,
    contexts: &mut [Context],
    usage_projections_present: bool,
) -> Result<(), Error> {
    let subjects: Vec<_> = contexts
        .iter()
        .map(|context| context.subject.clone())
        .collect();
    for source in SOURCES {
        let mut filter = doc! {source.owner:{"$in":&subjects}};
        match source.collection {
            "resource_policies" => {
                filter.insert("owner_kind", "person");
            }
            "resource_shares" if source.owner == "record.owner.subject" => {
                filter.insert("record.owner.kind", "person");
            }
            "agent_creation_provenance" => {
                filter.insert("record.owner.kind", "person");
            }
            "sandbox_usage_report_control" => {
                filter.insert("_id", doc! {"$regex":"^diagnostic:"});
            }
            _ => {}
        }
        scan(db, contexts, source, filter, &[source.owner], false).await?;
    }
    if usage_projections_present {
        discover_usage_projections(db, contexts).await?;
    }
    activity::protect_organizations(db, contexts).await?;
    // Actor attribution never grants personal ownership of organization data.
    let policy = source!("resource_policies", "owner_subject", Sandboxes => "_id");
    scan(
        db,
        contexts,
        &policy,
        doc! {"owner_kind":"organization",
        "record.organization_evidence.admitted_by.subject":{"$in":&subjects}},
        &["record.organization_evidence.admitted_by.subject"],
        true,
    )
    .await?;
    let provenance = source!("agent_creation_provenance", "record.owner.subject", Agents => "_id",
        Sandboxes => "record.sandbox_id", Workspaces => "record.workspace_id", Worktrees => "record.worktree_id");
    scan(
        db,
        contexts,
        &provenance,
        doc! {"record.owner.kind":"organization","$or":[
            {"record.created_by.subject":{"$in":&subjects}},
            {"record.creation_origin.launching_actor.subject":{"$in":&subjects}}
        ]},
        &[
            "record.created_by.subject",
            "record.creation_origin.launching_actor.subject",
        ],
        true,
    )
    .await?;
    // A retained organizational creation fact also protects a candidate derived
    // from an administrator's lifecycle operation after its parent is purged.
    if contexts
        .iter()
        .any(|context| !context.ids(Set::Sandboxes).is_empty())
    {
        scan(
            db,
            contexts,
            &provenance,
            doc! {"record.owner.kind":"organization", "record.sandbox_id":{"$in":contexts.iter().flat_map(|context|context.ids(Set::Sandboxes)).collect::<Vec<_>>() }},
            &[],
            true,
        )
        .await?;
    }
    Ok(())
}

pub(in crate::store::account_erasure) async fn discover_usage_projections(
    db: &Database,
    contexts: &mut [Context],
) -> Result<bool, Error> {
    let subjects: Vec<_> = contexts
        .iter()
        .map(|context| context.subject.clone())
        .collect();
    let report_count = |contexts: &[Context]| {
        contexts
            .iter()
            .map(|context| context.ids(Set::Reports).len())
            .sum::<usize>()
    };
    let before = report_count(contexts);
    let source = Source {
        collection: crate::store::migrations::usage_projections::COLLECTION,
        owner: "contributor",
        ids: &[(Set::Reports, "report_id")],
    };
    scan(
        db,
        contexts,
        &source,
        doc! {"kind":"contributor","contributor":{"$in":subjects}},
        &["contributor"],
        false,
    )
    .await?;
    Ok(report_count(contexts) != before)
}

async fn scan(
    db: &Database,
    contexts: &mut [Context],
    source: &Source,
    filter: Document,
    attribution: &[&str],
    protect: bool,
) -> Result<(), Error> {
    let collection =
        crate::store::durability::majority_collection::<Document>(db, source.collection);
    let mut projection = doc! {"_id":1};
    for (_, path) in source.ids {
        projection.insert(*path, 1);
    }
    for path in attribution {
        projection.insert(*path, 1);
    }
    // Each durable source streams once for the whole cohort and is then
    // cached. A projected page validates parent authority before capacity is
    // charged, so an administrator's organization children never become data
    // owned by that person's deletion.
    let mut cursor = collection
        .find(filter)
        .projection(projection)
        .batch_size(PAGE as u32)
        .await?;
    loop {
        let mut rows = Vec::new();
        while rows.len() < PAGE as usize {
            let Some(row) = cursor.try_next().await? else {
                break;
            };
            rows.push(row);
        }
        protect_parents(db, contexts, source, &rows, attribution).await?;
        for row in &rows {
            for context in contexts.iter_mut() {
                let matched = if attribution.is_empty() {
                    super::super::selectors::strings(row, "record.sandbox_id")
                        .iter()
                        .any(|id| {
                            context
                                .sets
                                .get(&Set::Sandboxes)
                                .is_some_and(|ids| ids.contains(*id))
                        })
                } else {
                    attribution.iter().any(|path| {
                        super::super::selectors::strings(row, path)
                            .contains(&context.subject.as_str())
                    })
                };
                if !matched {
                    continue;
                }
                let foreign = source
                    .ids
                    .iter()
                    .filter(|(set, _)| *set == Set::Sandboxes)
                    .any(|(_, path)| {
                        super::super::selectors::strings(row, path)
                            .iter()
                            .any(|id| context.protected.contains(*id))
                    });
                if foreign {
                    continue;
                }
                for (set, path) in source.ids {
                    if *path == "_id" && row.get_str("_id").is_err() {
                        return Err(
                            Error::InvalidRecord(source.collection).attribute(&context.subject)
                        );
                    }
                    for id in super::super::selectors::strings(row, path) {
                        if protect {
                            if *set == Set::Sandboxes {
                                context.protect(*set, id);
                            }
                        } else {
                            context.add(*set, id)?;
                        }
                    }
                }
                context.bounds()?;
            }
        }
        if rows.len() < PAGE as usize {
            break;
        }
    }
    Ok(())
}
async fn protect_parents(
    db: &Database,
    contexts: &mut [Context],
    source: &Source,
    rows: &[Document],
    attribution: &[&str],
) -> Result<(), Error> {
    let ids: std::collections::BTreeSet<_> = rows
        .iter()
        .flat_map(|row| {
            source
                .ids
                .iter()
                .filter(|(set, _)| *set == Set::Sandboxes)
                .flat_map(move |(_, path)| super::super::selectors::strings(row, path))
        })
        .collect();
    if ids.is_empty() {
        return Ok(());
    }
    let ids: Vec<_> = ids.into_iter().collect();
    let policies: Vec<_> =
        crate::store::durability::majority_collection::<Document>(db, "resource_policies")
            .find(doc! {"_id":{"$in":&ids}})
            .projection(doc! {"owner_kind":1,"owner_subject":1})
            .await?
            .try_collect()
            .await?;
    let roots: Vec<_> = crate::store::durability::majority_collection::<Document>(db, "sandboxes")
        .find(doc! {"_id":{"$in":&ids}})
        .projection(doc! {"owner":1})
        .await?
        .try_collect()
        .await?;
    for context in contexts {
        let interested: std::collections::BTreeSet<_> = rows
            .iter()
            .filter(|row| {
                if attribution.is_empty() {
                    super::super::selectors::strings(row, "record.sandbox_id")
                        .iter()
                        .any(|id| {
                            context
                                .sets
                                .get(&Set::Sandboxes)
                                .is_some_and(|ids| ids.contains(*id))
                        })
                } else {
                    attribution.iter().any(|path| {
                        super::super::selectors::strings(row, path)
                            .contains(&context.subject.as_str())
                    })
                }
            })
            .flat_map(|row| {
                source
                    .ids
                    .iter()
                    .filter(|(set, _)| *set == Set::Sandboxes)
                    .flat_map(move |(_, path)| super::super::selectors::strings(row, path))
            })
            .collect();
        for row in policies.iter().chain(&roots) {
            if !row.get_str("_id").is_ok_and(|id| interested.contains(id)) {
                continue;
            }
            let organization = row.get_str("owner_kind").ok() == Some("organization");
            let foreign = row
                .get_str("owner_subject")
                .or_else(|_| row.get_str("owner"))
                .is_ok_and(|owner| owner != context.subject);
            if organization || foreign {
                if let Ok(id) = row.get_str("_id") {
                    context.protect(Set::Sandboxes, id);
                    if organization {
                        context.organization_sandboxes.insert(id.into());
                    }
                }
            }
        }
        context.exclude_protected();
    }
    Ok(())
}

pub(super) fn indexes() -> Vec<(&'static str, &'static str)> {
    SOURCES
        .iter()
        .map(|source| (source.collection, source.owner))
        .collect()
}
