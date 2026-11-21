//! Optional active-head scheduling hints; production requires Gateway-owned Mongo.
use super::{RecordClient, RecordError};
use crate::records::RecordKind;
use std::time::Duration;

impl RecordClient {
    /// The sealed payload's original retained-until value supplies record expiry.
    /// Presence in this index never proves that a provider request was admitted.
    pub fn media_continuations(mut self) -> Self {
        self.record_path = "/api/v1/gateway/media-continuation-records";
        self.record_kind = RecordKind::MediaContinuation;
        self.retention = Default::default();
        self
    }

    pub async fn media_continuations_ready(&self) -> bool {
        #[cfg(test)]
        if self.media_continuations_fixture && self.database.is_none() {
            return true;
        }
        let Some(database) = &self.database else {
            return false;
        };
        tokio::time::timeout(Duration::from_secs(2), database.media_continuations_ready())
            .await
            .is_ok_and(|result| result.is_ok())
    }

    #[cfg(test)]
    pub(crate) fn with_media_continuations_fixture(mut self) -> Self {
        self.media_continuations_fixture = true;
        self
    }

    pub(super) fn check_media_continuation_transport(&self) -> Result<(), RecordError> {
        if self.record_kind == RecordKind::MediaContinuation && self.database.is_none() {
            #[cfg(test)]
            let fixture = self.media_continuations_fixture;
            #[cfg(not(test))]
            let fixture = false;
            if !fixture {
                return Err(RecordError::Unavailable(
                    "media continuation index requires Gateway Mongo storage".into(),
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn media_continuations_refuse_legacy_http_except_explicit_test_fixtures() {
        let client = RecordClient::new("http://127.0.0.1:1", "fixture")
            .unwrap()
            .media_continuations();
        assert!(!client.media_continuations_ready().await);
        assert!(matches!(
            client.record_url(&uuid::Uuid::new_v4().to_string()),
            Err(RecordError::Unavailable(_))
        ));
        assert!(matches!(
            client.list(None, None, 32).await,
            Err(RecordError::Unavailable(_))
        ));
        let fixture = client.with_media_continuations_fixture();
        assert!(fixture.media_continuations_ready().await);
        assert!(matches!(
            fixture.list(None, None, 33).await,
            Err(RecordError::Rejected(_))
        ));
        assert!(fixture
            .record_url(&uuid::Uuid::new_v4().to_string())
            .unwrap()
            .path()
            .starts_with("/api/v1/gateway/media-continuation-records/"));
    }
}
