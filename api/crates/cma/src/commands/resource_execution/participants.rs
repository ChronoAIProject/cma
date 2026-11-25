use super::*;
use layer2_core::resource_access::execution::{MemberCompletionV1, ResourceReleaseGate};

/// Resolve cancellation authority for the original audience, including its
/// boot. The returned identity must come from authenticated transport binding.
#[async_trait::async_trait]
pub(crate) trait ParticipantDirectory: Send + Sync {
    async fn resolve(&self, member: &wire::MemberKey) -> ExecutionResult<Participant>;
}

pub(crate) struct Participant {
    audience: wire::ReceiverKey,
    control: Control,
}

enum Control {
    Execution(Arc<dyn wire::RuntimeExecutionControl>),
    Release(Arc<dyn ResourceReleaseGate>),
}

impl Participant {
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "awaits authenticated runtime audience producer")
    )]
    pub(crate) fn execution(
        audience: wire::ReceiverKey,
        control: Arc<dyn wire::RuntimeExecutionControl>,
    ) -> Self {
        Self {
            audience,
            control: Control::Execution(control),
        }
    }

    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "awaits authenticated HTTP transport audience producer"
        )
    )]
    pub(crate) fn release(
        audience: wire::ReceiverKey,
        control: Arc<dyn ResourceReleaseGate>,
    ) -> Self {
        Self {
            audience,
            control: Control::Release(control),
        }
    }

    pub(super) fn require_member(&self, member: &wire::MemberKey) -> ExecutionResult<()> {
        if &self.audience != member.receiver()
            || !matches!(
                (&self.control, member),
                (Control::Execution(_), wire::MemberKey::Execution { .. })
                    | (Control::Release(_), wire::MemberKey::Release { .. })
            )
        {
            return Err(ExecutionError::EvidenceMismatch);
        }
        Ok(())
    }

    pub(super) async fn receipt(
        &self,
        request: &wire::BarrierRequestV1,
    ) -> ExecutionResult<Option<MemberCompletionV1>> {
        self.require_member(&request.member)?;
        match &self.control {
            Control::Execution(control) => Ok(control
                .stop_receipt(request)
                .await?
                .map(|receipt| MemberCompletionV1::Execution { receipt })),
            Control::Release(control) => Ok(control
                .receipt(request)
                .await?
                .map(|receipt| MemberCompletionV1::Release { receipt })),
        }
    }

    pub(super) async fn barrier(
        &self,
        request: &wire::BarrierRequestV1,
    ) -> ExecutionResult<wire::BarrierReceiptV1> {
        self.require_member(&request.member)?;
        match &self.control {
            Control::Execution(control) => Ok(control.install_barrier(request).await?),
            Control::Release(control) => control.install_barrier(request).await,
        }
    }

    pub(super) async fn stop(
        &self,
        request: &wire::BarrierRequestV1,
    ) -> ExecutionResult<MemberCompletionV1> {
        self.require_member(&request.member)?;
        match &self.control {
            Control::Execution(control) => Ok(MemberCompletionV1::Execution {
                receipt: control.cancel(request).await?,
            }),
            Control::Release(control) => Ok(MemberCompletionV1::Release {
                receipt: control.close(request).await?,
            }),
        }
    }
}
