//! A lost storage acknowledgement can recover only the exact durable native binding.
use super::{Destination, Error, InteractionStore, PreparedCard};
use cmaeg_core::ids::MessageRef;

impl InteractionStore {
    pub(crate) async fn bind_native_whatsapp(
        &self,
        prepared: &mut PreparedCard,
        destination: Destination,
    ) -> Result<MessageRef, Error> {
        if prepared.card.native_whatsapp.is_none()
            || destination.outbound_id != destination.platform_id
            || !cmaeg_core::channel::whatsapp::valid_message_id(&destination.platform_id)
        {
            return Err(Error::Denied);
        }
        match self.bind(prepared, destination).await {
            Ok(reference) => Ok(reference),
            Err(error) => {
                let (stored, version) = self.load(&prepared.card.id).await?;
                // Never retry the platform send or guess from a matching ID alone.
                // A concurrent callback may advance the card; that remains unknown
                // to this sender while its own durable authority is still checked.
                if version != prepared.version.saturating_add(1)
                    || serde_json::to_value(&stored).map_err(|_| Error::Unavailable)?
                        != serde_json::to_value(&prepared.card).map_err(|_| Error::Unavailable)?
                {
                    return Err(error);
                }
                prepared.card = stored;
                prepared.version = version;
                self.reference(&prepared.card)
            }
        }
    }
}
