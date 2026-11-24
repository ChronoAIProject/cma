use super::*;

/// The exact home, runtime and authorization captured before forwarding an edit.
#[derive(Clone)]
pub struct FileWriteTarget {
    context: CallerContext,
    original: Arc<AuthorizedResource>,
}

impl FileWriteTarget {
    pub(super) fn capture(
        context: &CallerContext,
        original: AuthorizedResource,
        sandbox: &Sandbox,
    ) -> Option<Self> {
        let runtime = sandbox.runtime.as_ref()?;
        runtime.boot_id.as_ref()?;
        runtime.provision_generation?;
        let home = sandbox.lifecycle.home.as_ref()?;
        if home.released
            || home.receipt.is_none()
            || !home.request.policy.mounts_full_home()
            || !same_runtime(&original.snapshot.lineage.sandbox, sandbox)
        {
            return None;
        }
        Some(Self {
            context: context.clone(),
            original: Arc::new(original),
        })
    }

    pub(super) async fn check(
        &self,
        service: &AgentFileService,
        context: Option<&CallerContext>,
        agent_id: &AgentId,
        current: &AuthorizedResource,
    ) -> Result<(), AgentFileError> {
        let Some(context) = context else {
            return Err(AgentFileError::Conflict);
        };
        if context.caller != self.context.caller
            || context.actor != self.context.actor
            || context.credential != self.context.credential
            || context.credential_source != self.context.credential_source
            || current.decision.share_revisions != self.original.decision.share_revisions
            || current.decision.allows(ResourceCapability::ManageSharing)
                != self
                    .original
                    .decision
                    .allows(ResourceCapability::ManageSharing)
            || !same_runtime(
                &self.original.snapshot.lineage.sandbox,
                &current.snapshot.lineage.sandbox,
            )
        {
            return Err(AgentFileError::Conflict);
        }
        service
            .access
            .validate_current_read_snapshot(
                &self.original,
                &ResourceTarget::Agent {
                    agent_id: agent_id.clone(),
                },
                ResourceCapability::OperateAgent,
                &current.snapshot,
            )
            .await?;
        Ok(())
    }
}

fn same_runtime(original: &Sandbox, current: &Sandbox) -> bool {
    current.id == original.id
        && current.status == SandboxStatus::Active
        && current.lifecycle.operation.is_none()
        && current.runtime == original.runtime
        && current.lifecycle.home == original.lifecycle.home
}
