//! Record writes keep the legacy transport contract separate from local retention classes.
use super::*;

impl RecordClient {
    pub(crate) async fn channel_create_capability(
        &self,
        issuer: &str,
    ) -> Result<bool, RecordError> {
        if !self.channel_cancellation_enabled()
            || !matches!(&self.channel_create_authority, ChannelCreateAuthority::Fenced(expected) if expected.as_ref() == issuer)
        {
            return Ok(false);
        }
        let database = self.database().ok_or_else(|| {
            RecordError::Unavailable("Channel create fences need Gateway Mongo".into())
        })?;
        super::super::mongo::check_create_fence_floor(&database, issuer).await
    }

    pub(crate) async fn cancel_channel_create(
        &self,
        record_id: &RecordId,
        owner: &crate::auth::CancelCreateOwner,
        keys: &cmaeg_core::seal::MasterKeyRing,
    ) -> Result<super::super::CancelCreate, RecordError> {
        if !self.channel_cancellation_enabled()
            || !matches!(&self.channel_create_authority, ChannelCreateAuthority::Fenced(expected) if expected.as_ref() == owner.issuer)
        {
            return Err(RecordError::Unavailable(
                "Channel create cancellation unavailable".into(),
            ));
        }
        let database = self.database.as_ref().ok_or_else(|| {
            RecordError::Unavailable("Channel create fences need Gateway Mongo".into())
        })?;
        let identity = super::super::mongo::CreateFenceIdentity {
            issuer: &owner.issuer,
            kind: "human",
            subject: &owner.subject,
            record: record_id.as_str(),
        };
        database.cancel_channel_create(&identity, keys).await
    }

    pub(crate) async fn reserve_channel(
        &self,
        record_id: &RecordId,
        creator: &crate::auth::Creator,
        blob: &[u8],
        claim: Option<&super::super::CreateConnectionClaim>,
    ) -> Result<u64, RecordError> {
        self.require_channel_create_authority()?;
        let ChannelCreateAuthority::Fenced(issuer) = &self.channel_create_authority else {
            return self.put(record_id, &creator.subject, blob, None).await;
        };
        let database = self.database.as_ref().ok_or_else(|| {
            RecordError::Unavailable("Channel create fences need Gateway Mongo".into())
        })?;
        let identity = super::super::mongo::CreateFenceIdentity {
            issuer,
            kind: if creator.organization {
                "organization"
            } else {
                "human"
            },
            subject: &creator.subject,
            record: record_id.as_str(),
        };
        database
            .reserve_channel_create(&identity, blob, claim)
            .await
    }

    pub async fn put_expiring(
        &self,
        record_id: &RecordId,
        owner: &str,
        blob: &[u8],
        expected_version: Option<u64>,
        expires_at: Option<i64>,
    ) -> Result<u64, RecordError> {
        if blob.is_empty() || blob.len() > 65_536 {
            return Err(RecordError::Rejected(
                "record size is outside 1..=65536 bytes".into(),
            ));
        }
        let url = self.record_url(record_id.as_str())?;
        #[cfg(test)]
        let mut url = url;
        if let Some(db) = &self.database {
            return db
                .for_kind(self.record_kind)
                .put_with_retention(
                    record_id.as_str(),
                    owner,
                    blob,
                    expected_version,
                    expires_at,
                    self.retention,
                )
                .await;
        }
        if self.retention == super::super::retention::InteractionRetention::WhatsappReaction {
            if !self.whatsapp_reaction_storage_available() {
                return Err(RecordError::Unavailable(
                    "WhatsApp reaction retention requires Gateway Mongo storage".into(),
                ));
            }
            // Explicit test-only typed-storage adapter. Production never sends a
            // retention override to the legacy seven-day HTTP transport.
            #[cfg(test)]
            url.set_path(&format!("/test/whatsapp-reaction-records/{record_id}"));
        }
        let response = self
            .client
            .put(url)
            .bearer_auth(
                self.service_token
                    .bearer()
                    .await
                    .map_err(|error| RecordError::Unavailable(error.to_string()))?
                    .as_str(),
            )
            .json(&PutRequest {
                owner,
                blob: STANDARD.encode(blob),
                expected_version,
                expires_at,
            })
            .timeout(Duration::from_secs(5))
            .send()
            .await
            .map_err(transport)?;
        if response.status() == StatusCode::CONFLICT {
            return Err(conflict(response, expected_version.unwrap_or(0)).await);
        }
        let response = success(response).await?;
        read_json::<PutResponse>(response)
            .await
            .map(|body| body.version)
    }
}
