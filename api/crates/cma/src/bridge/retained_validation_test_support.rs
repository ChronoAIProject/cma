//! Exercise the production validator with current disposable custody and no
//! provisioning dependencies. Retained execution must not require provisioning.
use super::*;
use custody::{BridgeCredentialLease, BridgeCredentialResolver};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Mutex,
};

#[derive(Default)]
pub(crate) struct Checks {
    pub resolutions: AtomicUsize,
    pub revalidations: AtomicUsize,
    pub refuse_custody: AtomicBool,
    pub full: AtomicUsize,
    pub retained: AtomicUsize,
    pub proof: Mutex<Option<BridgeValidation>>,
}

struct Custody {
    checks: Arc<Checks>,
    bridge: layer2_core::bridge::BridgeId,
}

#[async_trait::async_trait]
impl BridgeCredentialResolver for Custody {
    async fn resolve(
        &self,
        owner: &ResourceOwner,
        config: &BridgeConfig,
    ) -> Result<BridgeCredentialLease, BridgeError> {
        self.checks.resolutions.fetch_add(1, Ordering::SeqCst);
        Ok(BridgeCredentialLease {
            owner: owner.clone(),
            credential_subject: types::owner_subject(owner).clone(),
            issuer: config.connector.issuer.clone(),
            key_id: config.connector.key_id.clone(),
            custody: config.connector.custody.clone(),
            custody_revision: 1,
            key_state_version: 3,
            value: crate::auth::persistence::SecretValue::new(
                "disposable-retained-channel-key".into(),
            ),
            deadline: tokio::time::Instant::now() + Duration::from_secs(30),
            routes: config
                .connector
                .route_ids
                .iter()
                .map(|id| layer2_core::bridge::BridgeRouteObservation {
                    route_id: id.clone(),
                    owner: owner.clone(),
                    bot_id: config.connector.bot_id.clone(),
                    key_id: config.connector.key_id.clone(),
                    enabled: true,
                })
                .collect(),
            control_plane: crate::auth::keys::bridge::test_fixture::lease(
                owner.clone(),
                self.bridge.clone(),
                config.connector.clone(),
                1,
            ),
        })
    }
    async fn revalidate(&self, _: &BridgeCredentialLease) -> Result<(), BridgeError> {
        self.checks.revalidations.fetch_add(1, Ordering::SeqCst);
        if self.checks.refuse_custody.load(Ordering::SeqCst) {
            return Err(BridgeError::ConnectionRequired(
                BridgeSetupBlock::CustodyUnavailable,
            ));
        }
        Ok(())
    }
}

struct Captured {
    inner: NyxBridgeValidator,
    checks: Arc<Checks>,
}
#[async_trait::async_trait]
impl BridgeConnectionValidator for Captured {
    async fn validate_connection(
        &self,
        owner: &ResourceOwner,
        config: &BridgeConfig,
    ) -> Result<BridgeValidation, BridgeError> {
        self.checks.full.fetch_add(1, Ordering::SeqCst);
        self.inner.validate_connection(owner, config).await
    }
    async fn validate_retained_channel(
        &self,
        request: &RetainedChannelConnection,
    ) -> Result<BridgeValidation, BridgeError> {
        self.checks.retained.fetch_add(1, Ordering::SeqCst);
        let result = self.inner.validate_retained_channel(request).await?;
        *self.checks.proof.lock().unwrap() = Some(result.clone());
        Ok(result)
    }
}

pub(crate) fn validator(
    resources: Arc<dyn ResourceShareStore>,
    bridge: layer2_core::bridge::BridgeId,
) -> (Arc<dyn BridgeConnectionValidator>, Arc<Checks>) {
    let checks = Arc::new(Checks::default());
    let flags = crate::config::BridgeFeatureFlags::default();
    let inner = NyxBridgeValidator {
        custody: Arc::new(Custody {
            checks: checks.clone(),
            bridge,
        }),
        resources,
        provisioning: None,
        flags,
    };
    // Include the actual rollout-policy wrapper so its retained forwarding is
    // exercised too, even when new-effect feature gates are disabled.
    (
        feature_policy::configured(
            Arc::new(Captured {
                inner,
                checks: checks.clone(),
            }),
            flags,
        ),
        checks,
    )
}
