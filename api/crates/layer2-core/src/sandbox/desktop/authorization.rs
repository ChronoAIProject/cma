//! Current login/credential authority stays separate from resource lineage.
use super::*;

/// Per-connection proof supplied by trusted authentication, never by a header flag.
/// Revalidation must retain the original credential/custody identity, fail closed
/// on unavailability, and must not borrow a different login for the same subject.
#[async_trait::async_trait]
pub trait DesktopAuthorization: Send + Sync {
    async fn current(&self, caller: &Caller) -> Result<(), DesktopError>;
}

impl DesktopService {
    pub async fn open_authorized(
        &self,
        caller: &Caller,
        id: &AgentId,
        authority: Arc<dyn DesktopAuthorization>,
    ) -> Result<DesktopConnection, DesktopError> {
        if !self.replicas_enabled {
            return self.open(caller, id).await;
        }
        // These current reads have no runtime effects and neither depends on
        // the other's result. Both must succeed before acquiring any authority.
        let (_, (sandbox_id, runtime)) = tokio::try_join!(
            timing::observe("pre_open_credential", async {
                tokio::time::timeout(Duration::from_secs(5), authority.current(caller))
                    .await
                    .map_err(|_| DesktopError::AuthorityUnavailable)?
            }),
            timing::observe("pre_open_lineage", self.authorize(caller, id)),
        )?;
        self.open_owned(caller, id, sandbox_id, runtime, authority)
            .await
    }
}
