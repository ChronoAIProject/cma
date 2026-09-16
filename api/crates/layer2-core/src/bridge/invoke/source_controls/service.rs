use super::super::{BridgeInvokeError as Error, BridgeInvokeGrantId, BridgeInvokeService};
use super::*;
use crate::bridge::BridgeFeature;
use crate::resource_access::{AccessPurpose, CallerContext, ResourceCapability};
use std::{
    sync::Arc,
    time::{Duration, SystemTime},
};

#[derive(Clone)]
pub struct BridgeSourceControlService {
    invokes: BridgeInvokeService,
    store: Arc<dyn BridgeSourceControlStore>,
}
impl BridgeSourceControlService {
    pub fn new(invokes: BridgeInvokeService, store: Arc<dyn BridgeSourceControlStore>) -> Self {
        Self { invokes, store }
    }
    pub async fn policy(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        grant: &BridgeInvokeGrantId,
    ) -> Result<Option<SourceControlPolicyView>, Error> {
        let access = self
            .access(context, credential, grant, AccessPurpose::Read)
            .await?;
        let policy = self.store.source_control_policy(&access).await?;
        if let Some(policy) = &policy {
            access.require_policy(policy)?;
        }
        self.revalidate(&access).await?;
        Ok(policy.map(|p| p.view))
    }
    pub async fn submit(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        grant: &BridgeInvokeGrantId,
        key: &str,
        request: SourceControlRequest,
    ) -> Result<SourceControlReceipt, Error> {
        request.validate()?;
        if key.is_empty() || key.len() > 200 || !key.bytes().all(|b| b.is_ascii_graphic()) {
            return Err(Error::Invalid("idempotency_key"));
        }
        let access = self
            .access(
                context,
                credential,
                grant,
                AccessPurpose::Mutation {
                    started_at: SystemTime::now(),
                },
            )
            .await?;
        let receipt_id = format!(
            "brscc_{}",
            crate::bridge::store::digest(&(
                "cma.source-control.command.v1",
                &context.credential.issuer,
                &context.actor,
                grant,
                key,
            ))?
        );
        let fingerprint = source_control_request_digest(&request)?;
        if let Some(receipt) = self
            .store
            .recover_source_control(&access, &receipt_id, &fingerprint)
            .await?
        {
            access.require_receipt(&receipt)?;
            if receipt.id != receipt_id || receipt.request != request {
                return Err(Error::Corrupt);
            }
            self.revalidate(&access).await?;
            return Ok(receipt);
        }
        self.invokes.definitions.check_feature(
            BridgeFeature::Management,
            &access.grant.config.connector.platform,
        )?;
        let previous = self.store.source_control_policy(&access).await?;
        if let Some(previous) = &previous {
            access.require_policy(previous)?;
        }
        if access.management.record().revision != request.expected_bridge_revision
            || access.grant.revision != request.expected_grant_revision
            || previous.as_ref().map_or(0, |policy| policy.view.revision)
                != request.expected_policy_revision
        {
            return Err(Error::Conflict);
        }
        if request.action == SourceControlAction::AllowSenderCancelAndSteer {
            access.grant.require_active()?;
            if access.management.record().admission_pin()? != access.grant.pin {
                return Err(Error::Suspended);
            }
        } else if previous.as_ref().is_none_or(|policy| !policy.view.enabled) {
            return Err(Error::Conflict);
        }
        let view = SourceControlPolicyView {
            id: super::model::policy_id(grant)?,
            grant_id: grant.clone(),
            owner: access.grant.owner.clone(),
            pin: access.grant.pin.clone(),
            target: access.grant.target.clone(),
            revision: request.expected_policy_revision + 1,
            enabled: request.action == SourceControlAction::AllowSenderCancelAndSteer,
            scope: SourceControlScope::SourceSender,
            approval_mode: SourceControlApprovalMode::DashboardOnly,
            updated_at: crate::now_rfc3339(),
            updated_by: context.actor.clone(),
        };
        let audits = [
            ResourceCapability::ManageBridge,
            ResourceCapability::Interact,
        ]
        .into_iter()
        .map(|capability| {
            access
                .target
                .authorization_audit(&context.actor, &access.grant.target.resource(), capability)
                .map_err(super::super::service::resource_error)
        })
        .collect::<Result<_, _>>()?;
        let policy = SourceControlPolicy {
            view: view.clone(),
            receipt_id: receipt_id.clone(),
            request_sha256: fingerprint,
            resource_audit: audits,
        };
        let receipt = SourceControlReceipt {
            id: receipt_id,
            request,
            result: view,
        };
        self.revalidate(&access).await?;
        let write = SourceControlWrite {
            access,
            previous,
            policy,
            receipt,
        };
        write.require_current()?;
        let expected_request = write.request().clone();
        let expected_id = write.receipt().id.clone();
        let receipt = self.store.apply_source_control(write).await?;
        if receipt.id != expected_id || receipt.request != expected_request {
            return Err(Error::Corrupt);
        }
        let current = self
            .access(context, credential, grant, AccessPurpose::Read)
            .await?;
        current.require_receipt(&receipt)?;
        self.revalidate(&current).await?;
        Ok(receipt)
    }
    async fn access(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        id: &BridgeInvokeGrantId,
        purpose: AccessPurpose,
    ) -> Result<SourceControlAccess, Error> {
        id.validate()?;
        let mut deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        let grant = self.invokes.store.grant(id).await?.ok_or(Error::NotFound)?;
        let mut management = self
            .invokes
            .management(context, credential, &grant.pin.bridge_id, deadline)
            .await?;
        super::super::service::require_owner(&grant, management.record())?;
        let target = self
            .invokes
            .access
            .authorize(
                context,
                credential,
                &grant.target.resource(),
                ResourceCapability::ManageBridge,
                purpose,
            )
            .await
            .map_err(super::super::service::resource_error)?;
        for capability in [
            ResourceCapability::ManageBridge,
            ResourceCapability::Interact,
        ] {
            target
                .decision
                .require(capability)
                .map_err(|_| Error::Forbidden)?;
            if let Some(bound) = target.decision.revalidation_deadline(capability) {
                deadline = deadline.min(bound);
            }
        }
        management.bound_deadline(deadline);
        let access = SourceControlAccess {
            management,
            target,
            grant,
        };
        access.require_current()?;
        Ok(access)
    }
    async fn revalidate(&self, access: &SourceControlAccess) -> Result<(), Error> {
        access.require_current()?;
        self.invokes
            .definitions
            .validate_management(&access.management)
            .await?;
        for capability in [
            ResourceCapability::ManageBridge,
            ResourceCapability::Interact,
        ] {
            self.invokes
                .access
                .validate_read_result(&access.target, &access.grant.target.resource(), capability)
                .await
                .map_err(super::super::service::resource_error)?;
        }
        let current = self
            .invokes
            .store
            .grant(&access.grant.id)
            .await?
            .ok_or(Error::NotFound)?;
        if current != access.grant {
            return Err(Error::Conflict);
        }
        access.require_current()
    }
}
