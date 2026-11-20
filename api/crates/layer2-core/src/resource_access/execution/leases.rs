use super::*;
use crate::resource_access::{AccessPurpose, CallerContext, ResourceAuthorizationBasis};
use std::time::SystemTime;

impl ResourceExecutionCoordinator {
    pub async fn authorize_execution(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        target: &ResourceTarget,
        requested: ResourceCapability,
        original: Option<&wire::ExecutionBasisV1>,
    ) -> ExecutionResult<ExecutionAuthority> {
        self.policy.require_active().await?;
        let credential_deadline = context.credential_deadline();
        if credential_deadline.is_some_and(|deadline| tokio::time::Instant::now() >= deadline) {
            return Err(ExecutionError::AuthorityChanged);
        }
        let authorized = self
            .access
            .authorize(
                context,
                credential,
                target,
                requested,
                match requested {
                    ResourceCapability::ViewDetails
                    | ResourceCapability::ViewHistory
                    | ResourceCapability::ViewArtifacts
                    | ResourceCapability::ViewFiles => AccessPurpose::Read,
                    _ => AccessPurpose::Mutation {
                        started_at: SystemTime::now(),
                    },
                },
            )
            .await?;
        let actor = actor_digest(&context.actor)?;
        let ancestry = ancestry_digest(&authorized.snapshot)?;
        let protected = capability(requested)?;
        let selected = selected_observation(&authorized, requested)?;
        let basis = if let Some(basis) = independent_basis(
            selected,
            digest(&authorized.snapshot.scope(target)?.owner)?,
            actor.clone(),
            ancestry.clone(),
            protected,
        )? {
            basis
        } else {
            if !self.policy.admitting {
                return Err(ExecutionError::Disabled);
            }
            let observation = match original {
                Some(wire::ExecutionBasisV1::Share { grant }) => authorized
                    .decision
                    .audit_observations()
                    .iter()
                    .filter(|row| row.capabilities.contains(&requested))
                    .filter_map(|row| share_observation(&row.basis))
                    .find(|row| {
                        row.id == grant.family.share_id.as_str()
                            && row.revision == grant.share_revision.get()
                    }),
                _ => share_observation(selected),
            }
            .ok_or(ExecutionError::AuthorityChanged)?;
            let share = authorized
                .snapshot
                .shares
                .iter()
                .find(|row| row.id == observation.id && row.revision == observation.revision)
                .ok_or(ExecutionError::AuthorityChanged)?;
            let family = self.policy.open_family(&authorized.snapshot, share).await?;
            let policy_revision = match original {
                Some(wire::ExecutionBasisV1::Share { grant }) => grant.policy_revision,
                _ => wire::Generation::new(authorized.snapshot.fence().policy_revision)?,
            };
            wire::ExecutionBasisV1::Share {
                grant: wire::GrantKey {
                    family,
                    share_revision: wire::Generation::new(share.revision)?,
                    target_ancestry: ancestry,
                    capability: protected,
                    policy_revision,
                    actor,
                },
            }
        };
        if original.is_some_and(|old| old != &basis) {
            return Err(ExecutionError::AuthorityChanged);
        }
        Ok(ExecutionAuthority {
            authorized,
            access: self.access.clone(),
            target: target.clone(),
            capability: requested,
            basis,
            epoch: self.policy.epoch,
            credential_deadline,
        })
    }

    pub async fn register_execution(
        &self,
        authority: &ExecutionAuthority,
        execution: &wire::ExecutionKey,
    ) -> ExecutionResult<ExecutionRecordV1> {
        self.policy.require_admitting(authority)?;
        authority.validate_live().await?;
        let ResourceTarget::Agent { agent_id } = authority.target() else {
            return Err(ExecutionError::AuthorityChanged);
        };
        if agent_id.as_str() != execution.agent_id.as_str() {
            return Err(ExecutionError::AuthorityChanged);
        }
        self.policy
            .store
            .register(
                authority,
                &wire::MemberKey::Execution {
                    execution: execution.clone(),
                },
            )
            .await
    }

    pub async fn issue_ticket(
        &self,
        authority: &ExecutionAuthority,
        ticket: &wire::ExecutionTicketV1,
    ) -> ExecutionResult<wire::ExecutionTicketV1> {
        self.policy.require_admitting(authority)?;
        authority.validate_live().await?;
        ticket.validate()?;
        if &ticket.basis != authority.basis() {
            return Err(ExecutionError::AuthorityChanged);
        }
        self.policy.store.issue_ticket(authority, ticket).await
    }

    pub async fn continue_execution(
        &self,
        authority: &ExecutionAuthority,
        lease: &wire::ContinuationV1,
    ) -> ExecutionResult<wire::ContinuationV1> {
        self.policy.require_admitting(authority)?;
        authority.validate_live().await?;
        if &lease.basis != authority.basis() {
            return Err(ExecutionError::AuthorityChanged);
        }
        self.policy.store.continue_execution(authority, lease).await
    }

    pub async fn observe_start(
        &self,
        authenticated: &wire::ReceiverKey,
        observation: &wire::StartObservationV1,
    ) -> ExecutionResult<()> {
        if authenticated != &observation.execution.receiver {
            return Err(ExecutionError::EvidenceMismatch);
        }
        self.policy.store.observe_start(observation).await
    }
}

pub(super) fn independent_basis(
    selected: &ResourceAuthorizationBasis,
    owner: wire::Digest,
    actor: wire::Digest,
    target_ancestry: wire::Digest,
    capability: wire::ProtectedCapability,
) -> ExecutionResult<Option<wire::ExecutionBasisV1>> {
    let authority = match selected {
        ResourceAuthorizationBasis::PersonalOwner => {
            return Ok(Some(wire::ExecutionBasisV1::IndependentOwner {
                owner,
                actor,
                target_ancestry,
                capability,
            }))
        }
        ResourceAuthorizationBasis::GlobalOperator => wire::IndependentAuthorityV1::GlobalOperator,
        ResourceAuthorizationBasis::Organization {
            issuer,
            organization,
            role,
            share: None,
            ..
        } => wire::IndependentAuthorityV1::OwningOrganization {
            issuer: digest(issuer)?,
            organization: digest(organization)?,
            role: digest(role)?,
        },
        ResourceAuthorizationBasis::ExternalBinding {
            binding_id,
            generation,
        } => wire::IndependentAuthorityV1::ExternalBinding {
            binding: digest(binding_id)?,
            generation: wire::Generation::new(*generation)?,
        },
        _ => return Ok(None),
    };
    Ok(Some(wire::ExecutionBasisV1::Independent {
        authority,
        owner,
        actor,
        target_ancestry,
        capability,
    }))
}

impl ResourceExecutionPolicy {
    pub(in crate::resource_access) async fn require_active(&self) -> ExecutionResult<()> {
        if self.store.activation().await? != (ActivationStateV1::Activated { epoch: self.epoch }) {
            return Err(ExecutionError::Corrupt);
        }
        Ok(())
    }

    pub async fn open_family(
        &self,
        snapshot: &ResourceAccessSnapshot,
        share: &ResourceShare,
    ) -> ExecutionResult<wire::GrantFamilyKey> {
        self.require_active().await?;
        let origin = origin_digest(snapshot, share)?;
        match self.store.grant(&origin).await? {
            Some(control) => {
                if control.key.origin != origin || control.key.share_id.as_str() != share.id {
                    return Err(ExecutionError::Corrupt);
                }
                control.require_open(&control.key, self.epoch)?;
                Ok(control.key)
            }
            None => Ok(wire::GrantFamilyKey {
                origin,
                share_id: wire::OpaqueId::new(&share.id)?,
                generation: wire::Generation::new(1)?,
            }),
        }
    }

    pub(in crate::resource_access) async fn filter_shares(
        &self,
        snapshot: &mut ResourceAccessSnapshot,
    ) -> ExecutionResult<()> {
        self.require_active().await?;
        let mut kept = Vec::with_capacity(snapshot.shares.len());
        for share in &snapshot.shares {
            if !self.admitting {
                continue;
            }
            match self.open_family(snapshot, share).await {
                Ok(_) => kept.push(share.clone()),
                Err(ExecutionError::Closing { .. }) => {}
                Err(error) => return Err(error),
            }
        }
        snapshot.shares = kept;
        Ok(())
    }

    pub(in crate::resource_access) async fn validate_read(
        &self,
        authorized: &AuthorizedResource,
        capability: ResourceCapability,
    ) -> ExecutionResult<()> {
        self.require_active().await?;
        let basis = selected_observation(authorized, capability)?;
        let Some(observation) = share_observation(basis) else {
            return Ok(());
        };
        if !self.admitting {
            return Err(ExecutionError::Disabled);
        }
        let share = authorized
            .snapshot
            .shares
            .iter()
            .find(|share| share.id == observation.id && share.revision == observation.revision)
            .ok_or(ExecutionError::AuthorityChanged)?;
        self.open_family(&authorized.snapshot, share).await?;
        Ok(())
    }
}

fn selected_observation(
    authorized: &AuthorizedResource,
    capability: ResourceCapability,
) -> ExecutionResult<&ResourceAuthorizationBasis> {
    authorized
        .decision
        .require(capability)
        .map_err(|_| ExecutionError::AuthorityChanged)?;
    let candidates = authorized
        .decision
        .audit_observations()
        .iter()
        .filter(|row| row.capabilities.contains(&capability));
    // Stable server-side selection: independent authority first, otherwise
    // the same share ID at admission and final read. No fallback on closing.
    candidates
        .clone()
        .find(|row| share_observation(&row.basis).is_none())
        .or_else(|| {
            candidates.min_by_key(|row| share_observation(&row.basis).map(|share| &share.id))
        })
        .map(|row| &row.basis)
        .ok_or(ExecutionError::AuthorityChanged)
}

fn share_observation(
    basis: &ResourceAuthorizationBasis,
) -> Option<&crate::resource_access::ResourceGrantObservation> {
    match basis {
        ResourceAuthorizationBasis::ShareRecipient { share, .. }
        | ResourceAuthorizationBasis::Organization {
            share: Some(share), ..
        } => Some(share),
        _ => None,
    }
}

impl ExecutionRecordV1 {
    pub fn observe_start(&mut self, observation: &wire::StartObservationV1) -> ExecutionResult<()> {
        self.validate()?;
        let Some(MemberPermitV1::Execution { ticket }) = &self.permit else {
            return Err(ExecutionError::EvidenceMismatch);
        };
        observation.validate_for(ticket)?;
        if self.started.as_ref().is_some_and(|old| old != observation) {
            return Err(ExecutionError::EvidenceMismatch);
        }
        let mut candidate = self.clone();
        candidate.started = Some(observation.clone());
        candidate.validate()?;
        *self = candidate;
        Ok(())
    }

    pub fn set_continuation(&mut self, lease: &wire::ContinuationV1) -> ExecutionResult<()> {
        self.validate()?;
        if self.barrier.is_some() || self.completion.is_some() {
            return Err(ExecutionError::AuthorityChanged);
        }
        let Some(MemberPermitV1::Execution { ticket }) = &self.permit else {
            return Err(ExecutionError::EvidenceMismatch);
        };
        lease.validate_for(ticket)?;
        if let Some(old) = &self.continuation {
            if old == lease {
                return Ok(());
            }
            lease.validate_after(old)?;
        } else if lease.sequence.get() != 2 {
            return Err(ExecutionError::EvidenceMismatch);
        }
        self.continuation = Some(lease.clone());
        Ok(())
    }
}
