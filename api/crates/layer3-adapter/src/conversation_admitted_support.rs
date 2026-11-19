use super::*;
use crate::ingress_budget::AdmittedOperation;

/// Synchronous source ports admit before boxing or invoking a capability query.
/// Implementations retain their own source storage and return same-root owners;
/// these observations never confer authority to mutate the conversation.
pub trait AdmittedConversationSettingsSupport: Send + Sync {
    fn belongs_to(&self, policy: &IngressOwnershipPolicy) -> bool;

    fn system_prompt_supported(
        &self,
        _policy: &IngressOwnershipPolicy,
    ) -> Result<AdmittedOperation<'_, bool>, AdmittedError> {
        Err(AdmittedError::Unsupported(
            "conversation has no admitted instruction support query",
        ))
    }

    fn managed_context_may_be_ready(
        &self,
        _policy: &IngressOwnershipPolicy,
    ) -> Result<AdmittedOperation<'_, bool>, AdmittedError> {
        Err(AdmittedError::Unsupported(
            "conversation has no admitted context readiness query",
        ))
    }

    fn managed_context_receipt(
        &self,
        _sequence: u64,
        _policy: &IngressOwnershipPolicy,
    ) -> Result<AdmittedOperation<'_, Option<String>>, AdmittedError> {
        Err(AdmittedError::Unsupported(
            "conversation has no admitted context receipt query",
        ))
    }

    fn managed_transport_receipt(
        &self,
        _sequence: u64,
        _policy: &IngressOwnershipPolicy,
    ) -> Result<AdmittedOperation<'_, Option<String>>, AdmittedError> {
        Err(AdmittedError::Unsupported(
            "conversation has no admitted transport receipt query",
        ))
    }
}

impl AdmittedConversationInputSender {
    pub fn with_settings_support(
        mut self,
        support: Arc<dyn AdmittedConversationSettingsSupport>,
    ) -> Result<Self, AdmissionRefusal> {
        if !support.belongs_to(&self.policy) {
            return Err(AdmissionRefusal::InvalidPlan {
                phase: "conversation settings source ownership",
            });
        }
        self.settings_support = Some(support);
        Ok(self)
    }

    pub fn system_prompt_supported(&self) -> Result<AdmittedOperation<'_, bool>, AdmittedError> {
        self.checked_query(match &self.settings_support {
            Some(support) => support.system_prompt_supported(&self.policy),
            None => self.empty_query(false),
        })
    }

    /// Local negative-only hint. True still requires fresh operation admission
    /// and an exact receipt; callers must not cache it as authority.
    pub fn managed_context_may_be_ready(
        &self,
    ) -> Result<AdmittedOperation<'_, bool>, AdmittedError> {
        self.checked_query(match &self.settings_support {
            Some(support) => support.managed_context_may_be_ready(&self.policy),
            None => self.empty_query(false),
        })
    }

    pub fn managed_context_receipt(
        &self,
        sequence: u64,
    ) -> Result<AdmittedOperation<'_, Option<String>>, AdmittedError> {
        self.checked_query(match &self.settings_support {
            Some(support) => support.managed_context_receipt(sequence, &self.policy),
            None => self.empty_query(None),
        })
    }

    pub fn managed_transport_receipt(
        &self,
        sequence: u64,
    ) -> Result<AdmittedOperation<'_, Option<String>>, AdmittedError> {
        self.checked_query(match &self.settings_support {
            Some(support) => support.managed_transport_receipt(sequence, &self.policy),
            None => self.empty_query(None),
        })
    }

    fn empty_query<T: Send + 'static>(
        &self,
        value: T,
    ) -> Result<AdmittedOperation<'_, T>, AdmittedError> {
        AdmittedOperation::new(
            &self.policy,
            IngressClass::Data,
            AllocationPlan::checked("absent conversation settings", &[0])?,
            |_| async { Ok(value) },
        )
    }

    fn checked_query<'a, T>(
        &self,
        result: Result<AdmittedOperation<'a, T>, AdmittedError>,
    ) -> Result<AdmittedOperation<'a, T>, AdmittedError> {
        let valid = match &result {
            Ok(operation) => operation.belongs_to(&self.policy),
            Err(error) => error.belongs_to(&self.policy),
        };
        if !valid {
            return Err(AdmissionRefusal::InvalidPlan {
                phase: "conversation settings operation ownership",
            }
            .into());
        }
        result
    }
}

#[cfg(test)]
#[path = "conversation_admitted_support_tests.rs"]
mod tests;
