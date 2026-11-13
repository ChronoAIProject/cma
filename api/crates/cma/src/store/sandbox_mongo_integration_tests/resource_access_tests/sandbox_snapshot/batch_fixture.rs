use super::*;
use layer2_core::sandbox::{HierarchyReadService, SandboxHierarchyRecord};

pub(super) fn mode(store: &MongoSandboxStore, mode: usize) -> MongoSandboxStore {
    store
        .clone()
        .with_sandbox_snapshot_reads(mode > 0)
        .with_sandbox_batched_reads(mode == 2)
}

pub(super) fn service(store: MongoSandboxStore) -> Arc<HierarchyReadService> {
    let store = Arc::new(store);
    HierarchyReadService::new(store.clone())
        .with_detail_timings(true)
        .with_resource_access(Arc::new(ResourceAccessService::new(
            store,
            Arc::new(NoOrganization),
        )))
}
struct NoOrganization;
#[async_trait::async_trait]
impl OrganizationAuthority for NoOrganization {
    async fn organization_choices(
        &self,
        _: &Caller,
        _: &str,
    ) -> Result<Vec<OrganizationChoice>, MembershipError> {
        Err(MembershipError::Unavailable)
    }
    async fn organization_context(
        &self,
        _: &Caller,
        _: &str,
        _: &SubjectId,
    ) -> Result<CallerContext, MembershipError> {
        Err(MembershipError::Unavailable)
    }
}

pub(super) async fn seed(
    store: &MongoSandboxStore,
    template: &SandboxHierarchyRecord,
    prefix: &str,
    workspace_count: u32,
    agents_per_workspace: u32,
) -> Sandbox {
    let mut root = template.sandbox.clone();
    root.id = SandboxId(format!("sbx_{prefix}"));
    root.name = prefix.into();
    // Admission permits at most 16 live Workspaces and 16 live Agents each.
    // Larger populations represent retained, removed/stopped history.
    root.limits.max_workspaces = workspace_count.clamp(1, 16).try_into().unwrap();
    root.limits.max_agents_per_workspace = agents_per_workspace.clamp(1, 16).try_into().unwrap();
    root.profile.content.max_workspaces = root.limits.max_workspaces;
    root.profile.content.max_agents_per_workspace = root.limits.max_agents_per_workspace;
    store.create_sandbox(root.clone()).await.unwrap();
    let mut workspaces = Vec::new();
    let mut trees = Vec::new();
    let mut agents = Vec::new();
    let mut logs = Vec::new();
    // Reverse insertion order makes accidental aggregate iteration order visible.
    for w in (0..workspace_count).rev() {
        let mut workspace = ready_workspace(
            &root,
            &format!("wks_{prefix}_{w:04}"),
            &format!("project-{w}"),
        );
        workspace.next_worktree_number = u64::from(agents_per_workspace) + 1;
        if w >= 16 {
            workspace.status = WorkspaceStatus::Removed;
            workspace.removed_at = Some(NOW.into());
        }
        workspaces.push(crate::store::workspaces::workspace_document(&workspace).unwrap());
        for a in (0..agents_per_workspace).rev() {
            let mut tree = template.worktrees[0].clone();
            let mut agent = template.agents[0].clone();
            tree.id = WorktreeId(format!("wkt_{prefix}_{w:04}_{a:04}"));
            tree.workspace_id = workspace.id.clone();
            tree.sandbox_id = root.id.clone();
            tree.name = format!("wt-{}", a + 1);
            tree.path = format!("{}/.worktrees/{}", workspace.path, tree.name);
            tree.branch = format!("cma/{}", tree.name);
            agent.id = AgentId(format!("agt_{prefix}_{w:04}_{a:04}"));
            agent.workspace_id = workspace.id.clone();
            agent.sandbox_id = root.id.clone();
            agent.worktree_id = tree.id.clone();
            agent.creation_key = agent.id.to_string();
            agent.conversation.conversation_id = Some(format!("conversation-{prefix}-{w}-{a}"));
            tree.agent_id = Some(agent.id.clone());
            let retained = w >= 16 || a >= 16;
            if retained {
                tree.status = WorktreeStatus::Removed;
                agent.status = AgentStatus::Stopped;
                agent.stopped_at = Some(NOW.into());
                agent.conversation.last_seq = 4;
            }
            agent.validate_conversation().unwrap();
            trees.push(crate::store::worktrees::worktree_document(&tree).unwrap());
            agents.push(crate::store::agents::agent_document(&agent).unwrap());
            let mut events = vec![
                (
                    1,
                    AgentLogKind::Created {
                        workspace_id: workspace.id.clone(),
                        worktree_id: tree.id.clone(),
                    },
                ),
                (
                    2,
                    AgentLogKind::ConversationOpened {
                        conversation_id: agent.conversation.conversation_id.clone().unwrap(),
                        runtime_generation: 1,
                    },
                ),
            ];
            if retained {
                events.extend([
                    (
                        3,
                        AgentLogKind::StatusChanged {
                            from: AgentStatus::Ready,
                            to: AgentStatus::Stopped,
                        },
                    ),
                    (
                        4,
                        AgentLogKind::Stopped {
                            reason: "retained fixture".into(),
                        },
                    ),
                ]);
            }
            for (seq, kind) in events {
                logs.push(
                    log_document(&AgentLogEntry {
                        agent_id: agent.id.clone(),
                        seq,
                        appended_at: NOW.into(),
                        kind,
                    })
                    .unwrap(),
                );
            }
        }
    }
    for (collection, rows) in [
        (&store.workspaces, workspaces),
        (&store.worktrees, trees),
        (&store.agents, agents),
        (&store.agent_log, logs),
    ] {
        // Bound fixture insertion memory/wire work; these are retained-record fixtures.
        for batch in rows.chunks(256) {
            collection.insert_many(batch.iter().cloned()).await.unwrap();
        }
    }
    root
}

pub(super) async fn template(
    store: &MongoSandboxStore,
    target: &ResourceTarget,
) -> SandboxHierarchyRecord {
    let root = store
        .resource_access_snapshot(target)
        .await
        .unwrap()
        .lineage
        .sandbox;
    store
        .read_sandbox_hierarchy(&root.id)
        .await
        .unwrap()
        .unwrap()
}

pub(super) fn clear(trace: &Trace) {
    trace.commands.lock().unwrap().clear();
    trace.checkouts.store(0, Ordering::SeqCst);
}
