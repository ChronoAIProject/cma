use super::*;
use crate::resource_access::{
    AccessError, AccessPurpose, AuthorizedResource, CallerContext, ResourceAccessStoreError,
    ResourceCapability, ResourceLineage, ResourceOwner, ResourceTarget,
};

impl HierarchyReadService {
    pub async fn sandbox_authorized(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        id: &SandboxId,
    ) -> Result<SandboxHierarchyReading, HierarchyReadError> {
        let target = ResourceTarget::Sandbox {
            sandbox_id: id.clone(),
        };
        let held = self
            .detail_phase(SandboxDetailPhase::Auth)
            .result(self.authorize(context, credential, &target))
            .await?;
        let result = self.sandbox_held(&held, id).await?;
        Ok(if personal(&held, &context.caller) {
            SandboxHierarchyReading::Personal(Box::new(result))
        } else {
            SandboxHierarchyReading::Shared(result.into())
        })
    }

    pub async fn workspace_authorized(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        id: &WorkspaceId,
    ) -> Result<WorkspaceHierarchyReading, HierarchyReadError> {
        let target = ResourceTarget::Workspace {
            workspace_id: id.clone(),
        };
        let held = self
            .detail_phase(SandboxDetailPhase::Auth)
            .result(self.authorize(context, credential, &target))
            .await?;
        let result = self.workspace_held(&held, id).await?;
        Ok(if personal(&held, &context.caller) {
            WorkspaceHierarchyReading::Personal(Box::new(result))
        } else {
            WorkspaceHierarchyReading::Shared(Box::new(result.into()))
        })
    }

    pub async fn worktree_authorized(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        id: &WorktreeId,
    ) -> Result<WorktreeHierarchyReading, HierarchyReadError> {
        let target = ResourceTarget::Worktree {
            worktree_id: id.clone(),
        };
        let held = self.authorize(context, credential, &target).await?;
        let result = self.worktree_held(&held, id).await?;
        Ok(if personal(&held, &context.caller) {
            WorktreeHierarchyReading::Personal(result)
        } else {
            WorktreeHierarchyReading::Shared(result.into())
        })
    }

    pub(super) async fn sandbox_personal(
        &self,
        caller: &Caller,
        id: &SandboxId,
    ) -> Result<SandboxHierarchyView, HierarchyReadError> {
        let target = ResourceTarget::Sandbox {
            sandbox_id: id.clone(),
        };
        let held = self
            .detail_phase(SandboxDetailPhase::Auth)
            .result(self.personal(caller, &target))
            .await?;
        self.sandbox_held(&held, id).await
    }
    pub(super) async fn workspace_personal(
        &self,
        caller: &Caller,
        id: &WorkspaceId,
    ) -> Result<WorkspaceHierarchyView, HierarchyReadError> {
        let target = ResourceTarget::Workspace {
            workspace_id: id.clone(),
        };
        let held = self
            .detail_phase(SandboxDetailPhase::Auth)
            .result(self.personal(caller, &target))
            .await?;
        self.workspace_held(&held, id).await
    }
    pub(super) async fn worktree_personal(
        &self,
        caller: &Caller,
        id: &WorktreeId,
    ) -> Result<WorktreeHierarchySummary, HierarchyReadError> {
        let target = ResourceTarget::Worktree {
            worktree_id: id.clone(),
        };
        let held = self.personal(caller, &target).await?;
        self.worktree_held(&held, id).await
    }

    pub(super) fn access(&self) -> Result<&ResourceAccessService, HierarchyReadError> {
        self.access.as_deref().ok_or_else(|| {
            HierarchyReadError::Unavailable("resource access service is not composed".into())
        })
    }
    async fn authorize(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        target: &ResourceTarget,
    ) -> Result<AuthorizedResource, HierarchyReadError> {
        if ![
            Capability::UseOwnSandboxes,
            Capability::ReadAllSandboxes,
            Capability::ControlAnySandbox,
        ]
        .into_iter()
        .any(|capability| context.caller.authorization.allows(capability))
        {
            return Err(HierarchyReadError::Forbidden);
        }
        tokio::time::timeout(
            std::time::Duration::from_secs(15),
            self.access()?.authorize(
                context,
                credential,
                target,
                ResourceCapability::ViewDetails,
                AccessPurpose::Read,
            ),
        )
        .await
        .map_err(|_| HierarchyReadError::Unavailable("hierarchy authorization timed out".into()))?
        .map_err(access_error)
    }
    async fn personal(
        &self,
        caller: &Caller,
        target: &ResourceTarget,
    ) -> Result<AuthorizedResource, HierarchyReadError> {
        tokio::time::timeout(
            std::time::Duration::from_secs(15),
            self.access()?
                .authorize_personal(caller, target, ResourceCapability::ViewDetails),
        )
        .await
        .map_err(|_| HierarchyReadError::Unavailable("hierarchy authorization timed out".into()))?
        .map_err(access_error)
    }
    async fn finish(
        &self,
        held: &AuthorizedResource,
        target: &ResourceTarget,
    ) -> Result<(), HierarchyReadError> {
        tokio::time::timeout(
            std::time::Duration::from_secs(15),
            self.access()?
                .validate_read_result(held, target, ResourceCapability::ViewDetails),
        )
        .await
        .map_err(|_| HierarchyReadError::Unavailable("hierarchy validation timed out".into()))?
        .map_err(access_error)
    }

    pub(super) async fn sandbox_held(
        &self,
        held: &AuthorizedResource,
        id: &SandboxId,
    ) -> Result<SandboxHierarchyView, HierarchyReadError> {
        let timer = self.detail_phase(SandboxDetailPhase::Hierarchy);
        let result = timer
            .scope(tokio::time::timeout(
                std::time::Duration::from_secs(15),
                self.store.read_sandbox_hierarchy(id),
            ))
            .await
            .map_err(|_| HierarchyReadError::Unavailable("hierarchy read timed out".into()))
            .and_then(|result| result.map_err(store_error));
        let projected = (|| {
            let record = result?.ok_or(HierarchyReadError::NotFound)?;
            if record.sandbox.id != *id
                || record.sandbox.owner != held.snapshot.lineage.sandbox.owner
                || record.sandbox.created_at != held.snapshot.lineage.sandbox.created_at
            {
                return Err(corrupt("Sandbox hierarchy crossed its authorized root"));
            }
            sandbox_view(record)
        })();
        timer.record(&projected);
        self.detail_phase(SandboxDetailPhase::FinalFence)
            .result(self.finish(
                held,
                &ResourceTarget::Sandbox {
                    sandbox_id: id.clone(),
                },
            ))
            .await?;
        projected
    }

    async fn workspace_held(
        &self,
        held: &AuthorizedResource,
        id: &WorkspaceId,
    ) -> Result<WorkspaceHierarchyView, HierarchyReadError> {
        let timer = self.detail_phase(SandboxDetailPhase::Hierarchy);
        let result = timer
            .scope(tokio::time::timeout(
                std::time::Duration::from_secs(15),
                self.store.read_workspace_hierarchy(id),
            ))
            .await
            .map_err(|_| HierarchyReadError::Unavailable("hierarchy read timed out".into()))
            .and_then(|result| result.map_err(store_error));
        let projected = (|| {
            let record = result?.ok_or(HierarchyReadError::NotFound)?;
            let lineage = ResourceLineage {
                sandbox: held.snapshot.lineage.sandbox.clone(),
                workspace: Some(record.workspace.clone()),
                worktree: None,
                agent: None,
            };
            lineage
                .validate(&ResourceTarget::Workspace {
                    workspace_id: id.clone(),
                })
                .map_err(|e| access_error(e.into()))?;
            let original = held
                .snapshot
                .lineage
                .workspace
                .as_ref()
                .ok_or_else(|| corrupt("authorized Workspace missing"))?;
            if original.created_at != record.workspace.created_at {
                return Err(corrupt("Workspace hierarchy crossed its incarnation"));
            }
            workspace_view(record)
        })();
        timer.record(&projected);
        self.detail_phase(SandboxDetailPhase::FinalFence)
            .result(self.finish(
                held,
                &ResourceTarget::Workspace {
                    workspace_id: id.clone(),
                },
            ))
            .await?;
        projected
    }

    async fn worktree_held(
        &self,
        held: &AuthorizedResource,
        id: &WorktreeId,
    ) -> Result<WorktreeHierarchySummary, HierarchyReadError> {
        // The authoritative policy snapshot already contains the exact Worktree
        // and its full parent lineage. No broader hierarchy read is necessary.
        let value = held
            .snapshot
            .lineage
            .worktree
            .as_ref()
            .ok_or_else(|| corrupt("authorized Worktree missing"))?;
        self.finish(
            held,
            &ResourceTarget::Worktree {
                worktree_id: id.clone(),
            },
        )
        .await?;
        Ok(value.into())
    }
}

pub(super) fn personal(held: &AuthorizedResource, caller: &Caller) -> bool {
    match &held.snapshot.policy {
        Some(policy) => {
            matches!(&policy.owner, ResourceOwner::Person { subject } if subject == &caller.subject)
        }
        None => held.snapshot.lineage.sandbox.owner == caller.subject,
    }
}
pub(super) fn access_error(error: ResourceAccessError) -> HierarchyReadError {
    match error {
        ResourceAccessError::Access(AccessError::NotFound)
        | ResourceAccessError::Store(ResourceAccessStoreError::NotFound) => {
            HierarchyReadError::NotFound
        }
        ResourceAccessError::Access(AccessError::Forbidden) => HierarchyReadError::Forbidden,
        error => HierarchyReadError::Access(error),
    }
}
