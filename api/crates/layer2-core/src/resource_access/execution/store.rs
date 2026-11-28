use super::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum GrantStateV1 {
    Open,
    Closing { operation: wire::Digest },
    Closed { operation: wire::Digest },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrantControlV1 {
    pub protocol: u32,
    pub key: wire::GrantFamilyKey,
    pub epoch: wire::Generation,
    pub revision: wire::Generation,
    pub members: u32,
    pub state: GrantStateV1,
}

impl GrantControlV1 {
    pub fn validate(&self) -> ExecutionResult<()> {
        wire::version(self.protocol)?;
        if self.members > MAX_FAMILY_MEMBERS {
            return Err(ExecutionError::Corrupt);
        }
        Ok(())
    }

    pub fn require_open(
        &self,
        family: &wire::GrantFamilyKey,
        epoch: wire::Generation,
    ) -> ExecutionResult<()> {
        self.validate()?;
        if &self.key != family || self.epoch != epoch {
            return Err(ExecutionError::AuthorityChanged);
        }
        match &self.state {
            GrantStateV1::Open => Ok(()),
            GrantStateV1::Closing { operation } | GrantStateV1::Closed { operation } => {
                Err(ExecutionError::Closing {
                    operation: operation.clone(),
                })
            }
        }
    }

    /// Reserve the membership and its eventual control/evidence slot together.
    /// Effects have a separate quota and cannot consume this reservation.
    pub fn reserve_member(&mut self) -> ExecutionResult<u32> {
        self.require_open(&self.key, self.epoch)?;
        if self.members == MAX_FAMILY_MEMBERS {
            return Err(ExecutionError::Capacity);
        }
        self.revision = self.revision.successor()?;
        self.members += 1;
        Ok(self.members)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MemberPermitV1 {
    Execution { ticket: wire::ExecutionTicketV1 },
    Release { permit: wire::ReleasePermitV1 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MemberCompletionV1 {
    Execution { receipt: wire::StopReceiptV1 },
    Release { receipt: wire::ReleaseReceiptV1 },
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionRecordV1 {
    pub protocol: u32,
    pub registration: wire::Digest,
    pub epoch: wire::Generation,
    pub target: ResourceTarget,
    pub basis: wire::ExecutionBasisV1,
    pub member: wire::MemberKey,
    pub ordinal: u32,
    pub permit: Option<MemberPermitV1>,
    pub continuation: Option<wire::ContinuationV1>,
    pub started: Option<wire::StartObservationV1>,
    pub barrier: Option<wire::BarrierReceiptV1>,
    pub completion: Option<MemberCompletionV1>,
    pub effect_count: u32,
}

impl ExecutionRecordV1 {
    pub fn validate_snapshot(&self, snapshot: &ResourceAccessSnapshot) -> ExecutionResult<()> {
        self.validate()?;
        let ancestry = ancestry_digest(snapshot)?;
        match &self.basis {
            wire::ExecutionBasisV1::Share { grant } => {
                let share = snapshot
                    .shares
                    .iter()
                    .find(|share| {
                        share.id == grant.family.share_id.as_str()
                            && share.revision == grant.share_revision.get()
                            && share.revoked_at.is_none()
                    })
                    .ok_or(ExecutionError::AuthorityChanged)?;
                if ancestry != grant.target_ancestry
                    || origin_digest(snapshot, share)? != grant.family.origin
                    || snapshot.fence().policy_revision < grant.policy_revision.get()
                {
                    return Err(ExecutionError::AuthorityChanged);
                }
            }
            wire::ExecutionBasisV1::IndependentOwner {
                owner,
                target_ancestry,
                ..
            }
            | wire::ExecutionBasisV1::Independent {
                owner,
                target_ancestry,
                ..
            } => {
                if ancestry != *target_ancestry
                    || digest(&snapshot.scope(&self.target)?.owner)? != *owner
                {
                    return Err(ExecutionError::AuthorityChanged);
                }
            }
        }
        Ok(())
    }

    pub fn validate(&self) -> ExecutionResult<()> {
        wire::version(self.protocol)?;
        self.member.validate()?;
        self.basis.validate()?;
        if self.registration != registration_id(&self.member)?
            || self.ordinal == 0
            || self.ordinal > MAX_FAMILY_MEMBERS
            || self.effect_count > MAX_MEMBER_EFFECTS
        {
            return Err(ExecutionError::Corrupt);
        }
        match (&self.member, &self.permit) {
            (_, None) if self.started.is_none() && self.continuation.is_none() => {}
            (
                wire::MemberKey::Execution { execution },
                Some(MemberPermitV1::Execution { ticket }),
            ) => {
                ticket.validate()?;
                if &ticket.execution != execution
                    || ticket.basis != self.basis
                    || ticket.registration != self.registration
                {
                    return Err(ExecutionError::Corrupt);
                }
                if let Some(lease) = &self.continuation {
                    lease.validate_for(ticket)?;
                }
                if let Some(start) = &self.started {
                    start.validate_for(ticket)?;
                }
            }
            (wire::MemberKey::Release { release }, Some(MemberPermitV1::Release { permit })) => {
                permit.validate()?;
                if &permit.release != release
                    || permit.basis != self.basis
                    || permit.registration != self.registration
                    || self.started.is_some()
                    || self.continuation.is_some()
                {
                    return Err(ExecutionError::Corrupt);
                }
            }
            _ => return Err(ExecutionError::Corrupt),
        }
        if let Some(barrier) = &self.barrier {
            barrier.validate_for(&barrier.request)?;
            if barrier.request.member != self.member
                || barrier.request.registration != self.registration
                || self.basis.family() != Some(&barrier.request.revocation.family)
            {
                return Err(ExecutionError::Corrupt);
            }
            if self.started.as_ref().is_some_and(|start| {
                start.state == wire::NativeStartState::Entered
                    && start.sequence >= barrier.barrier_sequence
            }) {
                return Err(ExecutionError::EvidenceMismatch);
            }
        }
        if let Some(completion) = &self.completion {
            let barrier = self.barrier.as_ref().ok_or(ExecutionError::Corrupt)?;
            match completion {
                MemberCompletionV1::Execution { receipt } => {
                    receipt.validate_for(&barrier.request)?;
                    if &receipt.barrier != barrier
                        || receipt.quiescence.effects.high_water != u64::from(self.effect_count)
                    {
                        return Err(ExecutionError::Corrupt);
                    }
                    if receipt.evidence == wire::StopEvidenceKind::RejectedBeforeStart
                        && self
                            .started
                            .as_ref()
                            .is_some_and(|start| start.state == wire::NativeStartState::Entered)
                    {
                        return Err(ExecutionError::EvidenceMismatch);
                    }
                }
                MemberCompletionV1::Release { receipt } => {
                    receipt.validate_for(&barrier.request)?;
                    if &receipt.barrier != barrier {
                        return Err(ExecutionError::Corrupt);
                    }
                }
            }
        }
        Ok(())
    }

    pub fn require_authority(&self, authority: &ExecutionAuthority) -> ExecutionResult<()> {
        self.validate()?;
        if self.basis != *authority.basis()
            || self.epoch != authority.epoch()
            || self.target != *authority.target()
        {
            return Err(ExecutionError::AuthorityChanged);
        }
        Ok(())
    }

    pub fn barrier_request(
        &self,
        operation: &RevocationOperationV1,
    ) -> ExecutionResult<wire::BarrierRequestV1> {
        self.validate()?;
        if self.basis.family() != Some(&operation.key.family)
            || self.ordinal > operation.frozen_members
        {
            return Err(ExecutionError::EvidenceMismatch);
        }
        Ok(wire::BarrierRequestV1 {
            protocol: wire::PROTOCOL_VERSION,
            revocation: operation.key.clone(),
            member: self.member.clone(),
            registration: self.registration.clone(),
            reason: operation.intent.reason(),
        })
    }

    pub fn observe_barrier(
        &mut self,
        request: &wire::BarrierRequestV1,
        receipt: &wire::BarrierReceiptV1,
    ) -> ExecutionResult<()> {
        self.validate()?;
        receipt.validate_for(request)?;
        if request.member != self.member
            || request.registration != self.registration
            || self.basis.family() != Some(&request.revocation.family)
        {
            return Err(ExecutionError::EvidenceMismatch);
        }
        if let Some(old) = &self.barrier {
            if old != receipt {
                return Err(ExecutionError::EvidenceMismatch);
            }
        } else {
            let mut candidate = self.clone();
            candidate.barrier = Some(receipt.clone());
            candidate.validate()?;
            *self = candidate;
        }
        Ok(())
    }

    pub fn observe_completion(&mut self, completion: &MemberCompletionV1) -> ExecutionResult<()> {
        self.validate()?;
        if self
            .completion
            .as_ref()
            .is_some_and(|old| old != completion)
        {
            return Err(ExecutionError::EvidenceMismatch);
        }
        let mut candidate = self.clone();
        candidate.completion = Some(completion.clone());
        candidate.validate()?;
        *self = candidate;
        Ok(())
    }
}

pub fn registration_id(member: &wire::MemberKey) -> ExecutionResult<wire::Digest> {
    member.validate()?;
    digest(&("resource-execution-registration-v1", member))
}

/// Implementations own transaction boundaries. Every admission writes both
/// the stable grant row and existing lineage fence; evidence updates never
/// create authority. There is deliberately no all-purpose update method.
#[async_trait::async_trait]
pub trait ResourceExecutionStore: Send + Sync {
    async fn activation(&self) -> ExecutionResult<ActivationStateV1>;
    async fn grant(&self, origin: &wire::Digest) -> ExecutionResult<Option<GrantControlV1>>;
    async fn register(
        &self,
        authority: &ExecutionAuthority,
        member: &wire::MemberKey,
    ) -> ExecutionResult<ExecutionRecordV1>;
    async fn record(
        &self,
        registration: &wire::Digest,
    ) -> ExecutionResult<Option<ExecutionRecordV1>>;
    async fn issue_ticket(
        &self,
        authority: &ExecutionAuthority,
        ticket: &wire::ExecutionTicketV1,
    ) -> ExecutionResult<wire::ExecutionTicketV1>;
    async fn continue_execution(
        &self,
        authority: &ExecutionAuthority,
        lease: &wire::ContinuationV1,
    ) -> ExecutionResult<wire::ContinuationV1>;
    async fn issue_release(
        &self,
        authority: &ExecutionAuthority,
        permit: &wire::ReleasePermitV1,
    ) -> ExecutionResult<wire::ReleasePermitV1>;
    async fn observe_start(&self, observation: &wire::StartObservationV1) -> ExecutionResult<()>;
    async fn begin_revocation(
        &self,
        authority: &RevocationAuthority,
    ) -> ExecutionResult<RevocationOperationV1>;
    async fn revocation(
        &self,
        operation: &wire::Digest,
    ) -> ExecutionResult<Option<RevocationOperationV1>>;
    async fn pending_revocations(
        &self,
        after: Option<&wire::Digest>,
        limit: u32,
    ) -> ExecutionResult<Vec<RevocationOperationV1>>;
    /// Pending revocations page the original contiguous cohort. An effective
    /// revocation can have gaps after account erasure removes settled evidence.
    async fn frozen_members(
        &self,
        operation: &wire::Digest,
        after: u32,
        limit: u32,
    ) -> ExecutionResult<Vec<ExecutionRecordV1>>;
    async fn observe_barrier(&self, receipt: &wire::BarrierReceiptV1) -> ExecutionResult<()>;
    async fn observe_completion(&self, completion: &MemberCompletionV1) -> ExecutionResult<()>;
    async fn finish_revocation(
        &self,
        operation: &wire::Digest,
    ) -> ExecutionResult<RevocationOperationV1>;
    async fn prepare_effect(
        &self,
        intent: &wire::EffectIntentV1,
    ) -> ExecutionResult<wire::EffectPermissionV1>;
    async fn observe_effect(&self, observation: &wire::EffectObservationV1) -> ExecutionResult<()>;
    async fn effects_for(
        &self,
        registration: &wire::Digest,
        after: u32,
        limit: u32,
    ) -> ExecutionResult<Vec<EffectRecordV1>>;
}
