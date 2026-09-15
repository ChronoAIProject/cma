use std::{
    sync::Arc,
    time::{Duration, SystemTime},
};

use crate::bridge::allocation::AllocationAuthority;
use crate::resource_access::{AccessPurpose, ResourceCapability};

use super::*;

impl BridgeInvokeService {
    /// Trusted allocation worker only. Public grant issuance does not expose
    /// this operation. Store-proven creation and current human management are
    /// both required; a manually bound Agent cannot gain Channel provenance.
    pub async fn issue_channel(
        &self,
        control_plane: Arc<dyn ChannelInvokeAuthority>,
        allocation_id: &str,
        pin: &crate::bridge::BridgeWorkPin,
        authority: AllocationAuthority,
    ) -> Result<BridgeInvokeGrantView, BridgeInvokeError> {
        control_plane.require_current()?;
        let context = control_plane.actor_context();
        let credential = Some(control_plane.credential());
        let started_at = SystemTime::now();
        let mut deadline =
            (tokio::time::Instant::now() + Duration::from_secs(15)).min(control_plane.deadline());
        let snapshot = self.store.channel_target(allocation_id).await?;
        let allocation = &snapshot.basis.allocation;
        if allocation.id != allocation_id {
            return Err(BridgeInvokeError::TargetMismatch);
        }
        authority
            .require_record(allocation)
            .map_err(|_| BridgeInvokeError::Unavailable)?;
        let mut management = self
            .management(context, credential, &allocation.pin.bridge_id, deadline)
            .await?;
        if management.record() != authority.bridge() {
            return Err(BridgeInvokeError::Conflict);
        }
        let config = management.record().pinned(pin)?.config.clone();
        snapshot
            .basis
            .require_bridge(management.record(), pin, &config)?;
        deadline = deadline.min(management.deadline());
        let receipt_id = super::super::store::digest(&(
            "cma.channel.invoke-grant.v2",
            &allocation.id,
            &allocation.pin,
            pin,
            &allocation.first_source,
        ))?;
        let grant_id = BridgeInvokeGrantId(format!("brinv_{receipt_id}"));
        let target = BridgeInvokeTarget::capture(&snapshot.resources)?;
        if &target.ids != allocation.placement.target() {
            return Err(BridgeInvokeError::TargetMismatch);
        }
        let authorized = self
            .access
            .authorize(
                context,
                credential,
                &target.resource(),
                ResourceCapability::ManageBridge,
                AccessPurpose::Mutation { started_at },
            )
            .await
            .map_err(super::service::resource_error)?;
        authorized
            .decision
            .require(ResourceCapability::Interact)
            .map_err(|_| BridgeInvokeError::Forbidden)?;
        if authorized.snapshot != snapshot.resources {
            return Err(BridgeInvokeError::Conflict);
        }
        let connection = self
            .definitions
            .connection(&allocation.owner, &config)
            .await?;
        deadline = deadline.min(connection.deadline());
        // A new configuring human cannot replace a revoked grant. Replays
        // still require fresh management, resource access and inbound custody.
        if let Some(prior) = self.store.grant(&grant_id).await? {
            prior.validate()?;
            if prior.channel.as_deref() != Some(&snapshot.basis)
                || prior.owner != allocation.owner
                || &prior.pin != pin
                || prior.config != config
            {
                return Err(BridgeInvokeError::TargetMismatch);
            }
            super::service::require_resource_owner(&prior, &authorized.snapshot)?;
            self.access
                .validate_read_result(
                    &authorized,
                    &target.resource(),
                    ResourceCapability::ManageBridge,
                )
                .await
                .map_err(super::service::resource_error)?;
            management.bound_deadline(deadline);
            self.definitions.validate_management(&management).await?;
            authority
                .require_record(allocation)
                .map_err(|_| BridgeInvokeError::Unavailable)?;
            control_plane.require_current()?;
            self.store
                .validate_channel_replay(&prior, &management, control_plane.as_ref())
                .await?;
            return Ok(prior.view());
        }
        for capability in [
            ResourceCapability::ManageBridge,
            ResourceCapability::Interact,
        ] {
            if let Some(bound) = authorized.decision.revalidation_deadline(capability) {
                deadline = deadline.min(bound);
            }
        }
        let resource_audit = [
            ResourceCapability::ManageBridge,
            ResourceCapability::Interact,
        ]
        .into_iter()
        .map(|capability| {
            authorized
                .authorization_audit(&context.actor, &target.resource(), capability)
                .map_err(super::service::resource_error)
        })
        .collect::<Result<_, _>>()?;
        let record = BridgeInvokeGrant {
            id: grant_id,
            revision: 1,
            owner: allocation.owner.clone(),
            pin: pin.clone(),
            config,
            target,
            consent: BridgeSharedContextConsent::UseChannelCreatedAgent,
            issued_at: crate::now_rfc3339(),
            issued_by: context.actor.clone(),
            resource_audit,
            channel: Some(Box::new(snapshot.basis)),
            state: BridgeInvokeGrantState::Active,
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
            .map_err(super::service::resource_error)?;
        management.bound_deadline(deadline);
        self.definitions.validate_management(&management).await?;
        let receipt = BridgeInvokeReceipt {
            id: receipt_id,
            request_digest: super::super::store::digest(&(
                "channel-created",
                &record.channel,
                &record.target,
            ))?,
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
            channel_authority: Some(authority),
            channel_control_plane: Some(control_plane),
        };
        write.require_current()?;
        Ok(self.store.apply(write).await?.result)
    }
}
