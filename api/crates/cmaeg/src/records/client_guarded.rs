use super::{AuthorityGuard, RecordClient, RecordError};

impl RecordClient {
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn put_interaction_guarded(
        &self,
        id: &str,
        owner: &str,
        blob: &[u8],
        expires_at: i64,
        channel_record_id: &str,
        channel_version: u64,
        authority_guards: &[AuthorityGuard],
    ) -> Result<u64, RecordError> {
        let database = self.database.as_ref().ok_or_else(|| {
            RecordError::Unavailable("guarded admission requires local records".into())
        })?;
        database
            .guarded_insert_interaction(
                id,
                owner,
                blob,
                expires_at,
                channel_record_id,
                channel_version,
                authority_guards,
            )
            .await
    }
}
