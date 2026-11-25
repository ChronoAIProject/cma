use super::{HostedSubscriptionBinding, HostedSubscriptionSource};
use crate::ingress_budget::{
    sizing, AdmissionRefusal, AdmittedError, AdmittedOperation, AllocationPlan, IngressClass,
    IngressOwnershipPolicy, SharedOwned,
};
use crate::{AgentSlot, RuntimeHandle};
use serde_json::Value;

pub(in crate::hosted) enum SubscriptionBinding {
    Legacy(HostedSubscriptionBinding),
    Admitted(AdmittedSubscriptionBinding),
}

#[derive(Clone)]
pub(in crate::hosted) struct AdmittedSubscriptionBinding(SharedOwned<Binding>);

pub(super) struct Binding {
    pub(super) source: SharedOwned<dyn HostedSubscriptionSource>,
    pub(super) sandbox_id: String,
    pub(super) runtime: RuntimeHandle,
    pub(super) agent: AgentSlot,
}

impl AdmittedSubscriptionBinding {
    /// Own new identity copies before allocation. The caller retains its input
    /// storage separately; the source must already own its allocation and data.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "admitted producers are supplied by the transport package"
        )
    )]
    pub(in crate::hosted) fn new(
        policy: &IngressOwnershipPolicy,
        source: SharedOwned<dyn HostedSubscriptionSource>,
        sandbox_id: &str,
        runtime: &RuntimeHandle,
        agent: &AgentSlot,
    ) -> Result<Self, AdmissionRefusal> {
        if !source.belongs_to(policy) {
            return Err(invalid_owner());
        }
        let storage = policy.admit(
            IngressClass::Data,
            AllocationPlan::checked(
                "subscription binding",
                &[
                    sizing::shared_owner_bytes::<Binding>(),
                    sandbox_id.len(),
                    runtime.as_str().len(),
                    agent.as_str().len(),
                ],
            )?,
        )?;
        Ok(Self(storage.initialize(|| Binding {
            source,
            sandbox_id: sandbox_id.to_owned(),
            runtime: RuntimeHandle(runtime.as_str().to_owned()),
            agent: AgentSlot(agent.as_str().to_owned()),
        })))
    }

    pub(in crate::hosted) fn belongs_to(&self, policy: &IngressOwnershipPolicy) -> bool {
        self.0.belongs_to(policy) && self.0.source.belongs_to(policy)
    }

    pub(super) fn value(&self) -> &Binding {
        &self.0
    }
}

fn invalid_owner() -> AdmissionRefusal {
    AdmissionRefusal::InvalidPlan {
        phase: "subscription binding ownership",
    }
}

impl SubscriptionBinding {
    pub(in crate::hosted) async fn credential(&self, params: &Value) -> crate::Result<Value> {
        match self {
            Self::Legacy(binding) => binding.credential(params).await,
            Self::Admitted(_) => Err(crate::AdapterError::Unsupported(
                crate::ingress_budget::diagnostic::text(
                    "admitted subscription binding requires an ownership policy",
                ),
            )),
        }
    }

    pub(super) fn reply_admitted<'a>(
        &'a self,
        id: &'a Value,
        params: &'a Value,
        policy: &IngressOwnershipPolicy,
    ) -> Result<AdmittedOperation<'a, crate::hosted::retained_json::JsonFrame>, AdmittedError> {
        match self {
            Self::Admitted(binding) => binding.reply_admitted(id, params, policy),
            Self::Legacy(_) => Err(invalid_owner().into()),
        }
    }
}

impl From<HostedSubscriptionBinding> for SubscriptionBinding {
    fn from(binding: HostedSubscriptionBinding) -> Self {
        Self::Legacy(binding)
    }
}

impl From<AdmittedSubscriptionBinding> for SubscriptionBinding {
    fn from(binding: AdmittedSubscriptionBinding) -> Self {
        Self::Admitted(binding)
    }
}

#[cfg(test)]
#[path = "subscriptions_binding_tests.rs"]
mod tests;
