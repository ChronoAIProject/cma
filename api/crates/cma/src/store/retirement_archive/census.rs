//! Typed ownership closure, never a generic identity/prefix scan.
use super::census_catalog::{Policy, AUDIT, DEPLOYMENT_ONLY, OUTSIDE_MANIFEST};
use super::*;
use std::collections::BTreeSet;
#[path = "census_activity.rs"]
mod activity;
#[path = "census_budget.rs"]
mod budget;
#[path = "census_retained.rs"]
mod retained;

pub(super) struct Closure {
    sandbox: Vec<String>,
    owner: String,
    agents: Vec<String>,
    resources: Vec<String>,
    commands: Vec<String>,
}
pub(super) fn validate_catalog() -> Result<()> {
    let names: BTreeSet<_> = AUDIT
        .iter()
        .map(|(name, _)| *name)
        .chain(OUTSIDE_MANIFEST.iter().copied())
        .collect();
    let declared: BTreeSet<_> = super::super::isolation::COLLECTIONS
        .iter()
        .copied()
        .collect();
    if names.len() != AUDIT.len() + OUTSIDE_MANIFEST.len()
        || names != declared
        || !DEPLOYMENT_ONLY.iter().all(|name| {
            AUDIT
                .iter()
                .any(|(audited, policy)| audited == name && matches!(policy, Policy::Independent))
        })
    {
        return Err(REFUSED);
    }
    Ok(())
}
pub(super) async fn selection(
    db: &Database,
    name: &str,
    closure: &Closure,
    session: &mut ClientSession,
) -> Result<Document> {
    let policy = AUDIT.iter().find(|(n, _)| *n == name).ok_or(REFUSED)?.1;
    if matches!(policy, Policy::Budget) {
        budget::require_known_shapes(db, session).await?;
    }
    // A future or unversioned ownership shape cannot disappear behind a v1
    // selector. Inspect schema admission for the whole supported namespace.
    if matches!(policy, Policy::Documents) {
        require_known_document_shapes(db, name, session).await?;
    }
    if !matches!(
        policy,
        Policy::Independent | Policy::Empty | Policy::Documents
    ) && db
        .collection::<Document>(name)
        .find_one(doc! {"document_schema_version":{"$ne":1_i64}})
        .projection(doc! {"_id":1})
        .max_time(std::time::Duration::from_secs(30))
        .session(&mut *session)
        .await
        .map_err(|_| REFUSED)?
        .is_some()
    {
        return Err(REFUSED);
    }
    Ok(match policy {
        Policy::Sandbox(paths) => fields(paths, &closure.sandbox),
        Policy::Agent(paths) => fields(paths, &closure.agents),
        Policy::Command => doc! {"$or":[
            fields(&["command_id"], &closure.commands),
            fields(&["record.agent_id"], &closure.agents),
            fields(&["record.resources.sandbox_id"], &closure.sandbox)
        ]},
        Policy::Activity => activity::selection(db, closure, session).await?,
        // Public document creation accepts an origin annotation. It is not
        // authority to archive another account's body under a guessed Agent ID.
        Policy::Documents => doc! {"$or":[
            {"$and":[
                {"document_schema_version":1_i64,"owner_subject":&closure.owner,"record.metadata.owner_subject":&closure.owner},
                fields(&["record.metadata.origin_agent_id", "record.runtime_author_agent_id"], &closure.agents)
            ]},
            {"$and":[
                {"document_schema_version":2_i64,"audit_owner_subject":&closure.owner,"audit.metadata.owner_subject":&closure.owner},
                fields(&["audit.metadata.origin_agent_id", "audit.runtime_author_agent_id"], &closure.agents)
            ]}
        ]},
        Policy::Budget => doc! {"$or":[
            // Full shapes were checked above. Only reservation families carry
            // record-level ownership; independent accounting stays excluded.
            {"$and":[
                {"_id":{"$regex":r"\A(?:command|turn|request):"}},
                fields(&["record.agent_id", "record.hold.scope.agent_id"], &closure.agents)
            ]},
            // Request-turn ledgers and stop evidence have no record-level owner.
            // Known shapes prove <family>:<Agent ID>:<decimal turn>; generated
            // Agent IDs (agt_ + UUID simple) contain no colon. Compare complete
            // segments so neighbouring IDs and owner baselines stay excluded.
            {"$expr":{"$and":[
                {"$in":[{"$arrayElemAt":[{"$split":["$_id", ":"]}, 0]}, ["request-turn", "stop"]]},
                {"$in":[{"$arrayElemAt":[{"$split":["$_id", ":"]}, 1]}, &closure.agents]}
            ]}}
        ]},
        Policy::Independent => doc! {"_id":{"$exists":false}},
        Policy::Empty => {
            // The removed/encrypted/indirect schema has no qualified closure.
            // Check the whole namespace in the same snapshot; even unrelated
            // rows refuse, because disjoint ownership cannot currently be proved.
            if db
                .collection::<Document>(name)
                .find_one(doc! {})
                .projection(doc! {"_id":1})
                .max_time(std::time::Duration::from_secs(30))
                .session(session)
                .await
                .map_err(|_| REFUSED)?
                .is_some()
            {
                return Err(REFUSED);
            }
            doc! {"_id":{"$exists":false}}
        }
    })
}
async fn require_known_document_shapes(
    db: &Database,
    name: &str,
    session: &mut ClientSession,
) -> Result<()> {
    let mut rows = db
        .collection::<Document>(name)
        .find(doc! {"document_schema_version":{"$ne":1_i64}})
        .batch_size(16)
        .max_time(std::time::Duration::from_secs(30))
        .session(&mut *session)
        .await
        .map_err(|_| REFUSED)?;
    while rows.advance(&mut *session).await.map_err(|_| REFUSED)? {
        let row = rows.deserialize_current().map_err(|_| REFUSED)?;
        if crate::document_artifacts::deletion::decode_audit(&row)
            .map_err(|_| REFUSED)?
            .is_none()
        {
            return Err(REFUSED);
        }
    }
    Ok(())
}

fn fields(paths: &[&str], ids: &[String]) -> Document {
    let clauses: Vec<_> = paths
        .iter()
        .map(|path| {
            let mut clause = Document::new();
            clause.insert(*path, doc! {"$in":ids});
            clause
        })
        .collect();
    doc! {"$or":clauses}
}
pub(super) async fn identities(
    db: &Database,
    sandbox: &str,
    session: &mut ClientSession,
) -> Result<Closure> {
    let parent = db
        .collection::<Document>("sandboxes")
        .find_one(doc! {"_id":sandbox})
        .session(&mut *session)
        .await
        .map_err(|_| REFUSED)?
        .ok_or(REFUSED)?;
    let owner = crate::store::sandboxes::read_sandbox(&parent)
        .map_err(|_| REFUSED)?
        .owner
        .to_string();
    let mut ids = BTreeSet::from([sandbox.to_string()]);
    let mut agents = BTreeSet::new();
    for name in ["workspaces", "worktrees", "agents"] {
        let mut cursor = db
            .collection::<Document>(name)
            .find(doc! {"$or":[{"sandbox_id":sandbox},{"record.sandbox_id":sandbox}]})
            .session(&mut *session)
            .await
            .map_err(|_| REFUSED)?;
        while cursor.advance(&mut *session).await.map_err(|_| REFUSED)? {
            let document = cursor.deserialize_current().map_err(|_| REFUSED)?;
            let id = document.get_str("_id").map_err(|_| REFUSED)?;
            let (owner_parent, workspace, worktree) = match name {
                "workspaces" => {
                    let value =
                        super::super::workspaces::read_workspace(&document).map_err(|_| REFUSED)?;
                    (value.sandbox_id.to_string(), None, None)
                }
                "worktrees" => {
                    let value =
                        super::super::worktrees::read_worktree(&document).map_err(|_| REFUSED)?;
                    (
                        value.sandbox_id.to_string(),
                        Some(value.workspace_id.to_string()),
                        None,
                    )
                }
                "agents" => {
                    let value = super::super::agents::read_agent(&document).map_err(|_| REFUSED)?;
                    agents.insert(value.id.to_string());
                    if document.contains_key(super::super::agent_log_integrity::QUARANTINE_FIELD) {
                        return Err(REFUSED);
                    }
                    super::super::agent_log_integrity::scan_collection(
                        super::super::agent_log_telemetry::ScanCaller::RecoveryProof,
                        &db.collection::<Document>("agent_log"),
                        &value.id,
                        value.conversation.last_seq,
                        session,
                        crate::store::agent_log_integrity::ScanOptions::new(
                            true,
                            Default::default(),
                        ),
                        |entry| {
                            if let layer2_core::sandbox::AgentLogKind::Created {
                                workspace_id,
                                worktree_id,
                            } = &entry.kind
                            {
                                if entry.seq != 1
                                    || workspace_id != &value.workspace_id
                                    || worktree_id != value.creation_worktree_id()
                                {
                                    return Err(
                                        layer2_core::sandbox::store::SandboxStoreError::Conflict,
                                    );
                                }
                            }
                            Ok(())
                        },
                    )
                    .await
                    .map_err(|_| REFUSED)?;
                    (
                        value.sandbox_id.to_string(),
                        Some(value.workspace_id.to_string()),
                        Some(value.worktree_id.to_string()),
                    )
                }
                _ => return Err(REFUSED),
            };
            if owner_parent != sandbox
                || workspace.as_ref().is_some_and(|id| !ids.contains(id))
                || worktree.as_ref().is_some_and(|id| !ids.contains(id))
            {
                return Err(REFUSED);
            }
            if !ids.insert(id.to_string()) || ids.len() > 100_000 {
                return Err(REFUSED);
            }
        }
    }
    retained::extend(db, sandbox, session, &mut ids, &mut agents).await?;
    let agents: Vec<_> = agents.into_iter().collect();
    let mut commands = Vec::new();
    let mut cursor = db
        .collection::<Document>("agent_commands")
        .find(fields(&["agent_id"], &agents))
        .session(&mut *session)
        .await
        .map_err(|_| REFUSED)?;
    while cursor.advance(&mut *session).await.map_err(|_| REFUSED)? {
        let row = cursor.deserialize_current().map_err(|_| REFUSED)?;
        commands.push(row.get_str("_id").map_err(|_| REFUSED)?.to_string());
        if commands.len() > 100_000 {
            return Err(REFUSED);
        }
    }
    Ok(Closure {
        sandbox: vec![sandbox.into()],
        owner,
        agents,
        resources: ids.into_iter().collect(),
        commands,
    })
}
