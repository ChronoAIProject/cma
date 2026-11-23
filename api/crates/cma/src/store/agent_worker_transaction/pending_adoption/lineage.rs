//! The parent revisions and native identity belong to the same publication.
use super::*;

pub(super) struct Lineage {
    sandbox: Sandbox,
    workspace: Workspace,
    worktree: Worktree,
}

pub(super) async fn matches_retained(
    store: &MongoSandboxStore,
    agent: &Agent,
    sandbox: &Sandbox,
    expected: &Binary,
    session: &mut mongodb::ClientSession,
) -> Result<bool, TransactionLogError> {
    let workspace = store
        .workspaces
        .find_one(doc! {"_id":agent.workspace_id.as_str()})
        .session(&mut *session)
        .await
        .map_err(TransactionLogError::Driver)?;
    let worktree = store
        .worktrees
        .find_one(doc! {"_id":agent.worktree_id.as_str()})
        .session(&mut *session)
        .await
        .map_err(TransactionLogError::Driver)?;
    let (Some(workspace), Some(worktree)) = (workspace, worktree) else {
        return Ok(false);
    };
    let workspace = crate::store::workspaces::read_workspace(&workspace)?;
    let worktree = read_worktree(&worktree)?;
    Ok(publication::request_digest(&(sandbox, &workspace, &worktree))? == *expected)
}

pub(super) async fn read(
    store: &MongoSandboxStore,
    agent: &Agent,
    scope: &mut TransactionScope<'_>,
) -> Result<Lineage, ApplyError> {
    let sandbox = scope
        .find_one(&store.sandboxes, doc! {"_id":agent.sandbox_id.as_str()})
        .await
        .map_err(ApplyError::Driver)?
        .ok_or_else(witness::unresolved)?;
    let workspace = scope
        .find_one(&store.workspaces, doc! {"_id":agent.workspace_id.as_str()})
        .await
        .map_err(ApplyError::Driver)?
        .ok_or_else(witness::unresolved)?;
    let worktree = scope
        .find_one(&store.worktrees, doc! {"_id":agent.worktree_id.as_str()})
        .await
        .map_err(ApplyError::Driver)?
        .ok_or_else(witness::unresolved)?;
    Ok(Lineage {
        sandbox: crate::store::sandboxes::read_sandbox(&sandbox).map_err(ApplyError::Domain)?,
        workspace: crate::store::workspaces::read_workspace(&workspace)
            .map_err(ApplyError::Domain)?,
        worktree: read_worktree(&worktree).map_err(ApplyError::Domain)?,
    })
}

impl Lineage {
    pub(super) fn revisions(&self) -> AgentWorkPublicationParents {
        AgentWorkPublicationParents {
            sandbox_revision: self.sandbox.revision,
            workspace_revision: self.workspace.revision,
            worktree_revision: self.worktree.revision,
            lineage: layer2_core::sandbox::store::AgentWorkPublicationLineage::Authorized,
        }
    }

    pub(super) fn proof(&self) -> Result<Binary, SandboxStoreError> {
        publication::request_digest(&(&self.sandbox, &self.workspace, &self.worktree))
    }

    pub(super) fn require(
        &self,
        current: &Agent,
        mutation: &AgentWorkMutation,
        now: &str,
    ) -> Result<(), ApplyError> {
        if !witness::pending_step(current)
            || current.revision != mutation.expected_agent_revision
            || current.work.lease.as_ref().is_none_or(|lease| {
                lease.holder != mutation.holder
                    || lease.fence != mutation.fence
                    || lease.expires_at.as_str() <= now
            })
            || current.conversation.conversation_id != mutation.agent.conversation.conversation_id
            || current.conversation.runtime_generation
                != mutation.agent.conversation.runtime_generation
            || current.work.receipts != mutation.agent.work.receipts
            || !matches!(
                (current.work.step, mutation.agent_logs.as_slice()),
                (
                    AgentWorkStep::PublishReady,
                    [
                        AgentLogKind::ConversationOpened { .. },
                        AgentLogKind::StatusChanged { .. }
                    ]
                ) | (
                    AgentWorkStep::PublishConnection,
                    [AgentLogKind::WorkStepCompleted {
                        step: AgentWorkStep::PublishConnection
                    }]
                )
            )
        {
            return Err(witness::unresolved());
        }
        validate_agent_work_mutation(
            current,
            &self.worktree,
            current.conversation.last_seq,
            mutation,
        )
        .map_err(ApplyError::Domain)?;
        // The claimed lease was proved above. The dry-run classifier otherwise
        // treats every live lease as belonging to a previous holder.
        let mut agent = current.clone();
        agent.work.lease = None;
        let worktree = match (current.work.step, mutation.worktree.as_ref()) {
            (AgentWorkStep::PublishReady, Some(change)) => &change.worktree,
            _ => &self.worktree,
        };
        let context = CapacityRecoveryContext {
            agent: &agent,
            sandbox: &self.sandbox,
            workspace: &self.workspace,
            worktree,
        };
        if context.classify(now) != CapacityRecoveryDisposition::ContinueOriginalWork {
            return Err(witness::unresolved());
        }
        Ok(())
    }

    pub(super) async fn fence(
        &self,
        store: &MongoSandboxStore,
        current: &Agent,
        scope: &mut TransactionScope<'_>,
    ) -> Result<(), ApplyError> {
        for (collection, id, revision) in [
            (
                &store.sandboxes,
                self.sandbox.id.as_str(),
                self.sandbox.revision,
            ),
            (
                &store.workspaces,
                self.workspace.id.as_str(),
                self.workspace.revision,
            ),
            (
                &store.worktrees,
                self.worktree.id.as_str(),
                self.worktree.revision,
            ),
        ] {
            let revision = signed(revision, "pending publication parent revision")
                .map_err(|_| witness::unresolved())?;
            let result = scope.update_one(collection, doc! {
                "_id": id, "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                "revision": revision, "record.revision": revision, "owner": current.owner.as_str(),
                "channel_reuse_cleanup_execution": {"$exists":false},
                "owned_workspace_cleanup_admission_v1": {"$exists":false},
            }, doc! {"$inc": {"capacity_recovery_fence": 1_i64}})
                .await.map_err(ApplyError::Driver)?;
            if result.matched_count != 1 {
                return Err(witness::unresolved());
            }
        }
        Ok(())
    }
}
