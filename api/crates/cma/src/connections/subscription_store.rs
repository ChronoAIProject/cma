use std::time::SystemTime;

use layer2_core::SubjectId;

use crate::auth::persistence::SecretValue;

use super::{native_codex::DeviceLogin, SubscriptionError};

#[derive(Debug, Clone)]
pub(crate) struct SubscriptionRecord {
    pub owner: SubjectId,
    pub incarnation: String,
    pub revision: u64,
    pub cache: Option<SecretValue>,
    pub pending: Option<PendingLogin>,
    pub refresh_lease: Option<RefreshLease>,
}

#[derive(Debug, Clone)]
pub(crate) struct PendingLogin {
    pub intent: String,
    pub device: DeviceLogin,
    pub expires_at: SystemTime,
    pub heartbeat_at: SystemTime,
    pub worker: String,
    pub state: layer2_core::trigger_domain::BrokerHandoffState,
}

#[derive(Debug, Clone)]
pub(crate) struct RefreshLease {
    pub holder: String,
    pub expires_at: SystemTime,
}

impl SubscriptionRecord {
    pub(crate) fn empty(owner: SubjectId) -> Self {
        Self {
            owner,
            incarnation: uuid::Uuid::new_v4().to_string(),
            revision: 1,
            cache: None,
            pending: None,
            refresh_lease: None,
        }
    }
}

#[async_trait::async_trait]
pub(crate) trait SubscriptionStore: Send + Sync {
    async fn get(&self, owner: &SubjectId)
        -> Result<Option<SubscriptionRecord>, SubscriptionError>;
    async fn insert(&self, record: SubscriptionRecord) -> Result<bool, SubscriptionError>;
    /// A login completion or stale refresher must never replace newer authority.
    async fn replace(
        &self,
        expected: &SubscriptionRecord,
        next: SubscriptionRecord,
    ) -> Result<bool, SubscriptionError>;
}
