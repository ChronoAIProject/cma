use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use crate::resource_access::{AccessPurpose, CallerContext, ResourceCapability, ResourceTarget};

use super::super::{
    BridgeAgentTarget, BridgeId, BridgePlacementPolicy, BridgeState, BridgeWorkPin,
};
use super::service::{require_owner, resource_error};
use super::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IssueBridgeInvokeGrant {
    pub bridge_id: BridgeId,
    pub expected_bridge_revision: u64,
    pub target: BridgeAgentTarget,
    pub shared_context: BridgeSharedContextConsent,
    pub idempotency_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RevokeBridgeInvokeGrant {
    pub grant_id: BridgeInvokeGrantId,
    pub expected_bridge_revision: u64,
    pub expected_revision: u64,
    pub idempotency_key: String,
}

impl BridgeInvokeService {
    pub async fn issue(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        request: IssueBridgeInvokeGrant,
    ) -> Result<BridgeInvokeReceipt, BridgeInvokeError> {
        let started_at = SystemTime::now();
        let mut deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        key(&request.idempotency_key)?;
        if request.shared_context != BridgeSharedContextConsent::UseRetainedAgentContext {
            return Err(BridgeInvokeError::Invalid("shared_context"));
        }
        let mut management = self
            .management(context, credential, &request.bridge_id, deadline)
            .await?;
        deadline = deadline.min(management.deadline());
        let receipt_id = receipt_id(context, &request.bridge_id, &request.idempotency_key)?;
        let request_digest = super::super::store::digest(&("issue", &request))?;
        if let Some(prior) = self.store.receipt(&receipt_id).await? {
            return replay(prior, &management, &request_digest);
        }
        let bridge = management.record();
        if bridge.revision != request.expected_bridge_revision {
            return Err(BridgeInvokeError::Conflict);
        }
        if matches!(bridge.state, BridgeState::Retired { .. }) {
            return Err(BridgeInvokeError::Suspended);
        }
        let config = &bridge.current()?.config;
        if !matches!(&config.placement, BridgePlacementPolicy::ExistingAgent { target } if target == &request.target)
        {
            return Err(BridgeInvokeError::TargetMismatch);
        }
        let target = ResourceTarget::Agent {
            agent_id: request.target.agent_id.clone(),
        };
        let authorized = self
            .access
            .authorize(
                context,
                credential,
                &target,
                ResourceCapability::ManageBridge,
                AccessPurpose::Mutation { started_at },
            )
            .await
            .map_err(resource_error)?;
        authorized
            .decision
            .require(ResourceCapability::Interact)
            .map_err(|_| BridgeInvokeError::Forbidden)?;
        let target = BridgeInvokeTarget::capture(&authorized.snapshot)?;
        if target.ids != request.target
            || authorized
                .snapshot
                .scope(&target.resource())
                .map_err(super::service::resource_store_error)?
                .owner
                != bridge.owner
        {
            return Err(BridgeInvokeError::TargetMismatch);
        }
        let connection = self.definitions.connection(&bridge.owner, config).await?;
        deadline = deadline.min(connection.deadline());
        for capability in [
            ResourceCapability::ManageBridge,
            ResourceCapability::Interact,
        ] {
            if let Some(bound) = authorized.decision.revalidation_deadline(capability) {
                deadline = deadline.min(bound);
            }
        }
        let record = BridgeInvokeGrant {
            id: BridgeInvokeGrantId(format!("brinv_{receipt_id}")),
            revision: 1,
            owner: bridge.owner.clone(),
            pin: BridgeWorkPin {
                bridge_id: bridge.id.clone(),
                config_revision: bridge.config_revision,
            },
            config: config.clone(),
            target,
            consent: request.shared_context,
            issued_at: crate::now_rfc3339(),
            issued_by: context.actor.clone(),
            resource_audit: [
                ResourceCapability::ManageBridge,
                ResourceCapability::Interact,
            ]
            .into_iter()
            .map(|capability| {
                authorized
                    .authorization_audit(
                        &context.actor,
                        &ResourceTarget::Agent {
                            agent_id: request.target.agent_id.clone(),
                        },
                        capability,
                    )
                    .map_err(resource_error)
            })
            .collect::<Result<_, _>>()?,
            state: BridgeInvokeGrantState::Active,
            channel: None,
        };
        record.validate()?;
        super::service::require_resource_owner(&record, &authorized.snapshot)?;
        self.access
            .validate_read_result(
                &authorized,
                &record.target.resource(),
                ResourceCapability::ManageBridge,
            )
            .await
            .map_err(resource_error)?;
        management.bound_deadline(deadline);
        self.definitions.validate_management(&management).await?;
        let receipt = BridgeInvokeReceipt {
            id: receipt_id,
            request_digest,
            bridge_id: record.pin.bridge_id.clone(),
            actor: context.actor.clone(),
            result: record.view(),
        };
        let write = BridgeInvokeWrite {
            management,
            previous: None,
            record,
            receipt,
            resources: Some(authorized.snapshot),
            custody: Some(connection.custody_pin().clone()),
            deadline,
            channel_authority: None,
            channel_control_plane: None,
        };
        write.require_current()?;
        self.store.apply(write).await
    }

    pub async fn revoke(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        request: RevokeBridgeInvokeGrant,
    ) -> Result<BridgeInvokeReceipt, BridgeInvokeError> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        key(&request.idempotency_key)?;
        request.grant_id.validate()?;
        let previous = self
            .store
            .grant(&request.grant_id)
            .await?
            .ok_or(BridgeInvokeError::NotFound)?;
        let management = self
            .management(context, credential, &previous.pin.bridge_id, deadline)
            .await?;
        require_owner(&previous, management.record())?;
        let receipt_id = receipt_id(context, &previous.pin.bridge_id, &request.idempotency_key)?;
        let request_digest = super::super::store::digest(&("revoke", &request))?;
        if let Some(prior) = self.store.receipt(&receipt_id).await? {
            return replay(prior, &management, &request_digest);
        }
        if management.record().revision != request.expected_bridge_revision
            || previous.revision != request.expected_revision
        {
            return Err(BridgeInvokeError::Conflict);
        }
        previous.require_active()?;
        let mut record = previous.clone();
        record.revision = 2;
        record.state = BridgeInvokeGrantState::Revoked {
            at: crate::now_rfc3339(),
            actor: context.actor.clone(),
        };
        let receipt = BridgeInvokeReceipt {
            id: receipt_id,
            request_digest,
            bridge_id: record.pin.bridge_id.clone(),
            actor: context.actor.clone(),
            result: record.view(),
        };
        let write = BridgeInvokeWrite {
            management,
            previous: Some(previous),
            record,
            receipt,
            resources: None,
            custody: None,
            deadline,
            channel_authority: None,
            channel_control_plane: None,
        };
        write.require_current()?;
        self.store.apply(write).await
    }
}

fn key(value: &str) -> Result<(), BridgeInvokeError> {
    if value.is_empty() || value.len() > 200 || !value.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(BridgeInvokeError::Invalid("idempotency_key"));
    }
    Ok(())
}
fn receipt_id(
    context: &CallerContext,
    id: &BridgeId,
    key: &str,
) -> Result<String, BridgeInvokeError> {
    Ok(super::super::store::digest(&(
        "bridge-invoke-command-v1",
        &context.credential.issuer,
        id,
        &context.actor,
        key,
    ))?)
}
fn replay(
    prior: BridgeInvokeReceipt,
    management: &super::super::BridgeManagementLease,
    request_digest: &str,
) -> Result<BridgeInvokeReceipt, BridgeInvokeError> {
    management.require_current()?;
    prior.validate()?;
    if prior.request_digest != request_digest
        || prior.bridge_id != management.record().id
        || prior.actor != *management.actor()
        || prior.result.owner != management.record().owner
    {
        return Err(BridgeInvokeError::IdempotencyConflict);
    }
    Ok(prior)
}
