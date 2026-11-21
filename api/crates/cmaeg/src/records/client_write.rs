//! Record writes keep the legacy transport contract separate from local retention classes.
use super::*;

impl RecordClient {
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
