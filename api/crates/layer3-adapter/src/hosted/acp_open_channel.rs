use super::*;
use crate::ingress_budget::{
    diagnostic::make, AdmissionRefusal, AdmittedConversationChannel,
    AdmittedConversationInputSender, DeliveryObservation, IngressClass, IngressOwnershipPolicy,
    ReservationBundle, SharedOwned,
};

pub(super) trait Mode {
    type Input: Send;
    type Channel: Send;
    type Handle: Clone + std::ops::Deref<Target = Bridge> + Send + Sync + 'static;

    fn own(
        bridge: Bridge,
        policy: Option<&IngressOwnershipPolicy>,
    ) -> Result<Self::Handle, AdmissionRefusal>;

    fn input(
        bridge: &Self::Handle,
        updates: &UpdateRelay,
    ) -> impl std::future::Future<Output = crate::Result<Self::Input>> + Send;

    fn finish(
        input: Self::Input,
        identity: String,
        storage: Option<ReservationBundle>,
    ) -> crate::Result<Self::Channel>;
}

pub(super) struct Legacy;

impl Mode for Legacy {
    type Input = ConversationInputSender;
    type Channel = ConversationChannel;
    type Handle = Arc<Bridge>;

    fn own(
        bridge: Bridge,
        _policy: Option<&IngressOwnershipPolicy>,
    ) -> Result<Self::Handle, AdmissionRefusal> {
        Ok(Arc::new(bridge))
    }

    async fn input(bridge: &Arc<Bridge>, updates: &UpdateRelay) -> crate::Result<Self::Input> {
        let writer = Arc::clone(bridge);
        let writer_updates = updates.clone();
        Ok(ConversationInputSender::spawn(INPUT_BUFFER, move |input| {
            let writer = Arc::clone(&writer);
            let updates = writer_updates.clone();
            async move { writer.drive(input, &updates).await }
        })
        .with_settings_support(bridge.clone()))
    }

    fn finish(
        input: Self::Input,
        identity: String,
        _storage: Option<ReservationBundle>,
    ) -> crate::Result<Self::Channel> {
        Ok(ConversationChannel {
            input,
            conversation_id: Some(identity),
        })
    }
}

pub(super) struct Admitted;

impl Mode for Admitted {
    type Input = AdmittedConversationInputSender;
    type Channel = AdmittedConversationChannel;
    type Handle = SharedOwned<Bridge>;

    fn own(
        bridge: Bridge,
        policy: Option<&IngressOwnershipPolicy>,
    ) -> Result<Self::Handle, AdmissionRefusal> {
        use crate::ingress_budget::{sizing, AllocationPlan};
        let policy = policy.ok_or(AdmissionRefusal::InvalidPlan {
            phase: "admitted bridge ownership",
        })?;
        if bridge
            .ingress
            .as_ref()
            .is_none_or(|owner| !owner.shares_root(policy))
        {
            return Err(AdmissionRefusal::InvalidPlan {
                phase: "admitted bridge ownership",
            });
        }
        Ok(policy
            .admit(
                IngressClass::Data,
                AllocationPlan::checked(
                    "shared bridge allocation",
                    &[sizing::shared_owner_bytes::<Bridge>()],
                )?,
            )?
            .initialize(|| bridge))
    }

    async fn input(bridge: &Self::Handle, updates: &UpdateRelay) -> crate::Result<Self::Input> {
        let policy = bridge.ingress.clone().ok_or_else(|| {
            make(
                AdapterError::Rejected,
                format_args!("admitted channel requires an ownership policy"),
            )
        })?;
        let (input, receiver) =
            AdmittedConversationInputSender::channel(INPUT_BUFFER, policy.clone())?;
        let input = input
            .with_observation(DeliveryObservation {
                progress: bridge.output_observation().await,
                history: Some(bridge.retained_history_deadline.observe()),
            })?
            .with_owned_settings_support(bridge.clone())?;
        let writer = bridge.clone();
        let writer_updates = updates.clone();
        tokio::spawn(receiver.run(move |input| {
            let writer = writer.clone();
            let updates = writer_updates.clone();
            let policy = policy.clone();
            async move {
                let class = match &*input {
                    crate::ConversationInput::Cancel
                    | crate::ConversationInput::PermissionAnswer { .. } => IngressClass::Control,
                    _ => IngressClass::Data,
                };
                input
                    .map(|input| async move { writer.drive_admitted(input, &updates).await })
                    .resolve_source_result(&policy, class)
                    .await
                    .map(|_| ())
            }
        }));
        Ok(input)
    }

    fn finish(
        input: Self::Input,
        identity: String,
        storage: Option<ReservationBundle>,
    ) -> crate::Result<Self::Channel> {
        let storage = storage.ok_or_else(|| {
            make(
                AdapterError::Rejected,
                format_args!("admitted identity requires allocation custody"),
            )
        })?;
        AdmittedConversationChannel::new(input, storage.initialize(|| identity)).map_err(Into::into)
    }
}

#[cfg(test)]
#[path = "acp_open_owner_tests.rs"]
mod owner_tests;
