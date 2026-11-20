use super::*;

/// The body owner keeps this permit until every producer and transport has
/// closed. Dropping a handler or timing out never constructs a closure ACK.
pub struct OwnedReleasePermit {
    permit: wire::ReleasePermitV1,
}

impl OwnedReleasePermit {
    pub fn wire(&self) -> &wire::ReleasePermitV1 {
        &self.permit
    }
}

#[async_trait::async_trait]
pub trait ResourceReleaseGate: Send + Sync {
    fn supported(&self) -> bool {
        false
    }
    async fn challenge(
        &self,
        _release: &wire::ReleaseKey,
    ) -> ExecutionResult<wire::ReceiverChallengeV1> {
        Err(ExecutionError::Disabled)
    }
    async fn admit(&self, _permit: OwnedReleasePermit) -> ExecutionResult<()> {
        Err(ExecutionError::Disabled)
    }
    async fn continue_release(&self, _permit: OwnedReleasePermit) -> ExecutionResult<()> {
        Err(ExecutionError::Disabled)
    }
    async fn install_barrier(
        &self,
        _request: &wire::BarrierRequestV1,
    ) -> ExecutionResult<wire::BarrierReceiptV1> {
        Err(ExecutionError::Disabled)
    }
    async fn close(
        &self,
        _request: &wire::BarrierRequestV1,
    ) -> ExecutionResult<wire::ReleaseReceiptV1> {
        Err(ExecutionError::Disabled)
    }
    async fn receipt(
        &self,
        _request: &wire::BarrierRequestV1,
    ) -> ExecutionResult<Option<wire::ReleaseReceiptV1>> {
        Err(ExecutionError::Disabled)
    }
}

#[derive(Default)]
pub struct UnsupportedReleaseGate;
impl ResourceReleaseGate for UnsupportedReleaseGate {}

impl ResourceExecutionCoordinator {
    pub async fn register_release(
        &self,
        authority: &ExecutionAuthority,
        release: &wire::ReleaseKey,
    ) -> ExecutionResult<ExecutionRecordV1> {
        self.policy.require_admitting(authority)?;
        authority.validate_live().await?;
        if !matches!(
            capability(authority.capability)?,
            wire::ProtectedCapability::ReadDetails
                | wire::ProtectedCapability::ReadHistory
                | wire::ProtectedCapability::ReadArtifacts
                | wire::ProtectedCapability::ReadFiles
        ) {
            return Err(ExecutionError::AuthorityChanged);
        }
        self.policy
            .store
            .register(
                authority,
                &wire::MemberKey::Release {
                    release: release.clone(),
                },
            )
            .await
    }

    pub async fn issue_release(
        &self,
        authority: &ExecutionAuthority,
        permit: &wire::ReleasePermitV1,
    ) -> ExecutionResult<OwnedReleasePermit> {
        self.policy.require_admitting(authority)?;
        authority.validate_live().await?;
        permit.validate()?;
        if &permit.basis != authority.basis() {
            return Err(ExecutionError::AuthorityChanged);
        }
        let permit = self.policy.store.issue_release(authority, permit).await?;
        Ok(OwnedReleasePermit { permit })
    }
}

impl ExecutionRecordV1 {
    pub fn set_release(&mut self, permit: &wire::ReleasePermitV1) -> ExecutionResult<()> {
        self.validate()?;
        permit.validate()?;
        if self.barrier.is_some() || self.completion.is_some() {
            return Err(ExecutionError::AuthorityChanged);
        }
        let mut candidate = self.clone();
        match &self.permit {
            Some(MemberPermitV1::Release { permit: old }) => {
                if old == permit {
                    return Ok(());
                }
                permit.validate_after(old)?;
            }
            None if permit.sequence.get() == 1 => {}
            _ => return Err(ExecutionError::EvidenceMismatch),
        }
        candidate.permit = Some(MemberPermitV1::Release {
            permit: permit.clone(),
        });
        candidate.validate()?;
        *self = candidate;
        Ok(())
    }
}
