use super::Bridge;
use crate::ingress_budget::{
    AdmissionRefusal, AdmittedConversationSettingsSupport, AdmittedError, AdmittedOperation,
    AllocationPlan, IngressClass, IngressOwnershipPolicy,
};

impl AdmittedConversationSettingsSupport for Bridge {
    fn belongs_to(&self, policy: &IngressOwnershipPolicy) -> bool {
        self.ingress
            .as_ref()
            .is_some_and(|owner| owner.shares_root(policy))
    }

    fn system_prompt_supported(
        &self,
        policy: &IngressOwnershipPolicy,
    ) -> Result<AdmittedOperation<'_, bool>, AdmittedError> {
        self.settings_query_policy(policy)?;
        AdmittedOperation::new(
            policy,
            IngressClass::Data,
            // Request preparation, correlation and reply parsing each admit
            // their own storage. This query's only other result is a bool.
            AllocationPlan::checked("instruction support query", &[0])?,
            |_| async { Ok(self.instructions_supported().await) },
        )
    }

    fn managed_context_may_be_ready(
        &self,
        policy: &IngressOwnershipPolicy,
    ) -> Result<AdmittedOperation<'_, bool>, AdmittedError> {
        self.settings_query_policy(policy)?;
        AdmittedOperation::new(
            policy,
            IngressClass::Data,
            // context_readiness_session copies the validated active identity
            // once; its String allocation cannot exceed the accepted limit.
            AllocationPlan::checked(
                "local context readiness query",
                &[crate::hosted::acp_session_identity::MAX_BYTES],
            )?,
            |_| async { Ok(self.context_readiness_session().await.is_some()) },
        )
    }
}

impl Bridge {
    fn settings_query_policy(&self, policy: &IngressOwnershipPolicy) -> Result<(), AdmittedError> {
        if !AdmittedConversationSettingsSupport::belongs_to(self, policy) {
            return Err(AdmissionRefusal::InvalidPlan {
                phase: "bridge settings query ownership",
            }
            .into());
        }
        Ok(())
    }
}
