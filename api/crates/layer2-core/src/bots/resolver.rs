use super::{BotError, BotProfiles, CreateBot};
use crate::SubjectId;

#[async_trait::async_trait]
pub trait BotProfileResolver: Send + Sync {
    async fn resolve(
        &self,
        owner: &SubjectId,
        request: &CreateBot,
    ) -> Result<BotProfiles, BotError>;
    async fn resolve_for_owner(
        &self,
        owner: &crate::resource_access::ResourceOwner,
        request: &CreateBot,
    ) -> Result<BotProfiles, BotError> {
        self.resolve(super::authority::owner_subject(owner), request)
            .await
    }
}
/// Resolves the user-facing inline consumer configuration into immutable,
/// private profile revisions. The implementation lives in the CMA composition
/// because image authority and profile materialization are service concerns.
#[async_trait::async_trait]
pub trait BotInlineProfileResolver: Send + Sync {
    async fn resolve_inline(
        &self,
        owner: &SubjectId,
        request: &CreateBot,
        delegated_credential: Option<&str>,
        idempotency_key: &str,
    ) -> Result<BotProfiles, BotError>;
}
