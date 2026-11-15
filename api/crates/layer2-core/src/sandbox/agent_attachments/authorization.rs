use super::*;
use crate::resource_access::{AccessError, AccessPurpose, ResourceActor};
use std::time::SystemTime;

impl AgentAttachmentService {
    /// Reuse the upload policy before routing or receiving an attachment body.
    pub async fn authorize_runtime_upload(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        agent_id: &AgentId,
    ) -> Result<(), AttachmentError> {
        let authorized = self
            .authorize_upload(&context.caller, Some(context), credential, agent_id)
            .await?;
        self.access
            .current_read_snapshot(
                &authorized,
                &ResourceTarget::Agent {
                    agent_id: agent_id.clone(),
                },
                ResourceCapability::OperateAgent,
            )
            .await?;
        Ok(())
    }

    pub(super) async fn authorize_upload(
        &self,
        caller: &Caller,
        context: Option<&CallerContext>,
        credential: Option<&str>,
        agent_id: &AgentId,
    ) -> Result<AuthorizedResource, AttachmentError> {
        let target = ResourceTarget::Agent {
            agent_id: agent_id.clone(),
        };
        let authorized = if let Some(context) = context {
            if caller != &context.caller
                || !context.validate_principal()
                || !matches!(&context.actor, ResourceActor::Human { subject } | ResourceActor::Api { subject, delegation_id: None } if subject == &caller.subject)
            {
                return Err(ResourceAccessError::from(AccessError::NotFound).into());
            }
            self.access
                .authorize(
                    context,
                    credential,
                    &target,
                    ResourceCapability::OperateAgent,
                    AccessPurpose::Mutation {
                        started_at: SystemTime::now(),
                    },
                )
                .await?
        } else {
            self.access
                .authorize_personal(caller, &target, ResourceCapability::OperateAgent)
                .await?
        };
        let owns_home = authorized
            .decision
            .allows(ResourceCapability::ManageSharing);
        let linked_write = authorized.snapshot.shares.iter().any(|share| {
            share.mode == crate::resource_access::ShareMode::ViewWrite
                && authorized
                    .decision
                    .share_revisions
                    .contains(&(share.id.clone(), share.revision))
                && share
                    .link
                    .as_ref()
                    .is_some_and(|link| link.accepted_subjects.contains(&caller.subject))
        });
        if !owns_home && !linked_write {
            return Err(ResourceAccessError::Access(AccessError::Forbidden).into());
        }
        Ok(authorized)
    }
}
