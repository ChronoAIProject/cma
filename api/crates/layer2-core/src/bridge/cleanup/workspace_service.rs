use super::*;
use crate::{
    bridge::{BridgeFeature, BridgeId},
    resource_access::{AccessPurpose, CallerContext},
};
use std::time::SystemTime;

impl BridgeCleanupService {
    pub async fn workspace_status(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        bridge: &BridgeId,
        id: &str,
    ) -> Result<WorkspaceCleanupStatus, CleanupError> {
        let access = self
            .access(context, credential, bridge, id, AccessPurpose::Read)
            .await?;
        let status = self.store.workspace_status(&access).await?;
        access.require_workspace_status(&status)?;
        self.revalidate(context, credential, &access).await?;
        Ok(status)
    }

    pub async fn workspace_submit(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        bridge: &BridgeId,
        command: WorkspaceCleanupCommand,
    ) -> Result<WorkspaceCleanupStatus, CleanupError> {
        let WorkspaceCleanupCommand {
            allocation_id,
            idempotency_key,
            request,
        } = command;
        request.validate()?;
        if idempotency_key.is_empty()
            || idempotency_key.len() > 200
            || !idempotency_key.bytes().all(|b| b.is_ascii_graphic())
        {
            return Err(CleanupError::Invalid);
        }
        let access = self
            .access(
                context,
                credential,
                bridge,
                &allocation_id,
                AccessPurpose::Mutation {
                    started_at: SystemTime::now(),
                },
            )
            .await?;
        let receipt_id = format!(
            "bwcl_{}",
            digest(&(
                "cma.bridge.owned-workspace-cleanup.v1",
                &context.credential.issuer,
                &context.actor,
                bridge,
                &allocation_id,
                &idempotency_key,
            ))?
        );
        let request_sha256 = digest(&request)?;
        // Recovery remains available after revision, feature or runtime changes.
        if let Some(status) = self
            .store
            .workspace_recover(&access, &receipt_id, &request_sha256)
            .await?
        {
            access.require_workspace_status(&status)?;
            let receipt = status.receipt.as_ref().ok_or(CleanupError::Corrupt)?;
            if receipt.id != receipt_id
                || receipt.request != request
                || receipt.requested_by != context.actor
            {
                return Err(CleanupError::Corrupt);
            }
            self.revalidate(context, credential, &access).await?;
            return Ok(status);
        }
        self.definitions.check_feature(
            BridgeFeature::Management,
            &access.allocation().config.connector.platform,
        )?;
        let write = WorkspaceCleanupWrite {
            access,
            request,
            receipt_id,
            request_sha256,
            issuer: context.credential.issuer.clone(),
        };
        write.require_current()?;
        let status = self.store.workspace_submit(write).await?;
        let access = self
            .access(
                context,
                credential,
                bridge,
                &allocation_id,
                AccessPurpose::Read,
            )
            .await?;
        access.require_workspace_status(&status)?;
        self.revalidate(context, credential, &access).await?;
        Ok(status)
    }
}
