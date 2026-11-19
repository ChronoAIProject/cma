use super::*;
use crate::ingress_budget::{
    diagnostic::make, AdmittedConversationChannel, AdmittedConversationInputSender,
    DeliveryObservation, IngressClass, ReservationBundle,
};

pub(super) trait Mode {
    type Input: Send;
    type Channel: Send;

    fn input(
        bridge: &Arc<Bridge>,
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

    async fn input(bridge: &Arc<Bridge>, updates: &UpdateRelay) -> crate::Result<Self::Input> {
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
            .with_settings_support(bridge.clone())?;
        let writer = Arc::clone(bridge);
        let writer_updates = updates.clone();
        tokio::spawn(receiver.run(move |input| {
            let writer = Arc::clone(&writer);
            let updates = writer_updates.clone();
            let policy = policy.clone();
            async move {
                let class = match &*input {
                    crate::ConversationInput::Cancel
                    | crate::ConversationInput::PermissionAnswer { .. } => IngressClass::Control,
                    _ => IngressClass::Data,
                };
                input
                    .map(|input| async move { writer.drive(input, &updates).await })
                    .resolve_admitted_result(&policy, class)
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
