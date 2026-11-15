//! Durable single-use admission. A successful create is the only fetch authority.
use crate::records::{RecordClient, RecordError};
use cmaeg_core::{
    ids::RecordId,
    seal::{seal_interaction, MasterKeyRing, MediaLinkPayload},
};
use sha2::{Digest, Sha256};

/// Shared record/CAS adapter; no process-local state can grant redemption.
pub(super) struct Redemptions {
    records: RecordClient,
}

impl Redemptions {
    pub(super) fn new(records: RecordClient) -> Self {
        Self {
            records: records.interactions(),
        }
    }

    /// Consume through the sealed expiry, including requests subsequently refused.
    /// A lost acknowledgement is an error even if the record may have been saved.
    pub(super) async fn consume(
        &self,
        keys: &MasterKeyRing,
        link: &MediaLinkPayload,
    ) -> Result<bool, RecordError> {
        let id = record_id(&link.jti);
        let payload = serde_json::to_vec(&serde_json::json!({
            "kind": "media_redemption_v1",
            "record": link.event.record_id,
            "expires_at": link.expires_at,
        }))
        .map_err(|error| RecordError::Malformed(error.to_string()))?;
        let blob = seal_interaction(keys, id.as_str(), &payload)
            .map_err(|error| RecordError::Malformed(error.to_string()))?;
        // No live owner lookup may precede consumption: deleted channels and
        // revoked sources also burn the credential. This internal row belongs
        // to its sealed channel record, never to a bearer-supplied identity.
        let owner = format!("media-download:{}", link.event.record_id);
        match self
            .records
            .put_expiring(&id, &owner, &blob, None, Some(link.expires_at))
            .await
        {
            Ok(_) => Ok(true),
            Err(RecordError::Conflict { .. }) => Ok(false),
            Err(error) => Err(error),
        }
    }
}

fn record_id(jti: &str) -> RecordId {
    let digest = Sha256::digest(format!("cmaeg.media-redemption.v1\0{jti}"));
    let mut bytes: [u8; 16] = digest[..16].try_into().unwrap();
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    uuid::Uuid::from_bytes(bytes).to_string().parse().unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingress::{routing_plan::fixture, routing_store::test_server::Server};
    use cmaeg_core::seal::EventRefPayload;

    fn link() -> MediaLinkPayload {
        let event: EventRefPayload = serde_json::from_value(serde_json::json!({
            "v":1,"channel_id":"cmaeg1.ch.fixture", "record_id":uuid::Uuid::new_v4().to_string(),
            "event_id":"event", "conversation_id":"conversation", "partition_key":"partition",
            "expires_at":fixture::NOW+600,
            "reply":{"type":"discord","bot_id":"77","channel_id":"99","message_id":"700"}
        }))
        .unwrap();
        MediaLinkPayload::new(
            "nyxbot".parse().unwrap(),
            "binding-1".parse().unwrap(),
            &[7; 32],
            event,
            0,
            fixture::NOW,
            120,
        )
        .unwrap()
    }

    #[tokio::test]
    async fn media_redemption_storage_uncertainty_never_grants_fetch_authority() {
        for persisted in [false, true] {
            let server = Server::start().await;
            let link = link();
            server.fault(record_id(&link.jti).to_string(), persisted, false);
            let store = Redemptions::new(server.records.clone());
            assert!(store.consume(&fixture::keys(), &link).await.is_err());
            let restarted = Redemptions::new(server.records.clone());
            assert_eq!(
                restarted.consume(&fixture::keys(), &link).await.unwrap(),
                !persisted
            );
            assert!(!restarted.consume(&fixture::keys(), &link).await.unwrap());
            assert_eq!(
                server.memory.rows.lock().unwrap()[record_id(&link.jti).as_str()].expiry,
                link.expires_at
            );
        }
    }
}
