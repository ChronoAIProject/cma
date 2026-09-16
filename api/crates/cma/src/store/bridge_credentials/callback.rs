//! Preserve driver labels while fencing current signed callback custody.
use super::*;

#[derive(Debug, thiserror::Error)]
pub(crate) enum CallbackBindingError {
    #[error(transparent)]
    Core(#[from] BridgeError),
    #[error(transparent)]
    Driver(#[from] mongodb::error::Error),
}
impl CallbackBindingError {
    pub(super) fn into_core(self) -> BridgeError {
        match self {
            Self::Core(error) => error,
            Self::Driver(_) => BridgeError::Unavailable,
        }
    }
}

impl MongoBridgeCredentialStore {
    pub(crate) async fn require_callback_binding(
        &self,
        bridge: &BridgeRecord,
        pin: &BridgeCustodyPin,
        session: &mut ClientSession,
    ) -> Result<(), CallbackBindingError> {
        self.require_retained_callback_binding(bridge, &bridge.current()?.config, pin, session)
            .await
    }

    pub(super) async fn require_retained_callback_binding(
        &self,
        bridge: &BridgeRecord,
        config: &layer2_core::bridge::BridgeConfig,
        pin: &BridgeCustodyPin,
        session: &mut ClientSession,
    ) -> Result<(), CallbackBindingError> {
        let unavailable = || BridgeError::ConnectionRequired(BridgeSetupBlock::CustodyUnavailable);
        if pin.reference() != &config.connector.custody
            || !bridge
                .generations
                .iter()
                .any(|generation| generation.config == *config)
        {
            return Err(unavailable().into());
        }
        let document = self
            .credentials
            .find_one(doc! { "_id": pin.reference().as_str() })
            .session(session)
            .await?
            .ok_or_else(unavailable)?;
        let material = self.decode(document)?;
        let metadata = material.metadata;
        if metadata.bridge_id != bridge.id
            || metadata.owner != bridge.owner
            || metadata.connector != config.connector
            || metadata.revision != pin.revision()
            || metadata.key_state_version != pin.key_state_version()
            || metadata.revoked_at.is_some()
        {
            return Err(unavailable().into());
        }
        Ok(())
    }
}
