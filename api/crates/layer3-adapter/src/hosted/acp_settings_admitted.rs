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

    fn managed_context_receipt(
        &self,
        sequence: u64,
        policy: &IngressOwnershipPolicy,
    ) -> Result<AdmittedOperation<'_, Option<String>>, AdmittedError> {
        self.context_receipt_query(sequence, false, policy)
    }

    fn managed_transport_receipt(
        &self,
        sequence: u64,
        policy: &IngressOwnershipPolicy,
    ) -> Result<AdmittedOperation<'_, Option<String>>, AdmittedError> {
        self.context_receipt_query(sequence, true, policy)
    }
}

impl Bridge {
    fn context_receipt_query(
        &self,
        sequence: u64,
        transport: bool,
        policy: &IngressOwnershipPolicy,
    ) -> Result<AdmittedOperation<'_, Option<String>>, AdmittedError> {
        self.settings_query_policy(policy)?;
        let owned_policy = policy.clone();
        AdmittedOperation::from_owned(
            policy,
            IngressClass::Data,
            AllocationPlan::checked(
                "context receipt session copy",
                &[crate::hosted::acp_session_identity::MAX_BYTES],
            )?,
            move |scope| async move {
                let policy = owned_policy;
                let session = scope
                    .initialize_owned(|| async { Ok(self.session().await.ok()) })
                    .resolve_admitted_result(&policy, IngressClass::Data)
                    .await?;
                let Some(session) = session.as_deref() else {
                    return Ok(scope.initialize_owned(|| None));
                };
                if sequence == 0 || self.context_busy().await {
                    return Ok(scope.initialize_owned(|| None));
                }
                let Some(binding) = &self.context_refresh else {
                    return Ok(scope.initialize_owned(|| None));
                };
                let observed = self
                    .context_goal
                    .lock()
                    .await
                    .as_ref()
                    .map(|goal| goal.is_safe_for(session));
                match observed {
                    Some(true) => {}
                    None if transport => {
                        let absent = binding.transport_goal_absent_admitted(session, &policy)?;
                        if !*absent.await? {
                            return Ok(scope.initialize_owned(|| None));
                        }
                    }
                    _ => return Ok(scope.initialize_owned(|| None)),
                }
                let receipt = binding.readiness_receipt_admitted(session, sequence, &policy)?;
                receipt.await
            },
        )
    }

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
