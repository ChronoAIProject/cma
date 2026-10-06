//! An explicit retry resends only the retained operation's platform button.
use super::{model::*, service::effective_status, store::index_id, ConnectionService, Error};
use sha2::{Digest, Sha256};
impl ConnectionService {
    pub async fn retry_keyboard(&self, record: &Connection) -> Result<(), Error> {
        if effective_status(record) != Status::AwaitingPlatform || record.material.is_some() {
            return Err(Error::Conflict);
        }
        let id = index_id(
            "manager-actor",
            &[record.origin.event.record_id.as_str(), &record.origin.actor],
        );
        let (mut pending, version) = match self.store.load(&id).await? {
            (Stored::Pending(pending), version)
                if pending.connection_id == record.id
                    && pending.owner == record.owner
                    && pending.receipt.is_none() =>
            {
                (pending, version)
            }
            _ => return Err(Error::Conflict),
        };
        let (token, _) = self.manager(record).await?;
        let request_id = i32::from_be_bytes(
            Sha256::digest(record.id.as_bytes())[..4]
                .try_into()
                .unwrap(),
        );
        let sent = self
            .telegram()?
            .send_managed_keyboard(
                &token,
                &record.origin.conversation.id,
                request_id,
                pending.suggested_name.as_deref(),
                pending.suggested_username.as_deref(),
            )
            .await
            .map_err(|_| Error::Unavailable)?;
        // Keep the earliest accepted button as the receipt ordering fence.
        if pending.prompt_message.is_none() {
            pending.prompt_message = sent.parse().ok();
            match self
                .store
                .save(
                    &id,
                    &pending.owner,
                    &Stored::Pending(pending.clone()),
                    Some(version),
                )
                .await
            {
                Ok(_) | Err(Error::Conflict) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }
}
