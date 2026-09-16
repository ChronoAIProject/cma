//! Existing NyxID bridge verification. Read-only metadata is kept separate from
//! encrypted selected-key custody and distinct Sandbox provisioning authority.

use std::{sync::Arc, time::Duration};

use layer2_core::bridge::{
    BridgeConfig, BridgeConnectionObservation, BridgeConnectionValidator, BridgeError,
    BridgePlacementPolicy, BridgeService, BridgeSetupBlock, BridgeValidation,
};
use layer2_core::resource_access::{ResourceOwner, ResourceShareStore, ResourceTarget};

use crate::auth::AuthMode;
use crate::store::MongoBridgeStore;

#[allow(dead_code)] // No connector fetch grants, expiring custody or Agent media consumer yet.
mod attachments;
mod capability_catalog;
pub(crate) mod channel_renewal_work;
pub(crate) mod channel_scheduler;
pub(crate) mod channel_tools;
pub(crate) mod channel_work;
pub(crate) mod cleanup;
pub(crate) mod cleanup_work;
pub(crate) mod composition;
pub(crate) mod control_plane;
pub(crate) mod custody;
mod feature_policy;
#[allow(dead_code)] // #861 awaits a composed route with live selected-key custody.
pub(crate) mod ingress;
mod managed_credentials;
pub(crate) mod outbox_delivery;
pub(crate) mod outbox_rate;
pub(crate) mod outbox_scheduler;
mod provider;
pub(crate) mod provisioning;
pub(crate) mod setup;
mod types;
pub(crate) mod workspace_cleanup_work;

pub(crate) use control_plane::BridgeControlPlaneLease;
use custody::BridgeCredentialResolver;
pub(crate) use managed_credentials::ManagedBridgeCredentials;
use provider::NyxBridgeProvider;

pub(crate) fn service(
    mode: &AuthMode,
    store: Arc<MongoBridgeStore>,
    organizations: Arc<crate::auth::AuthState>,
    resources: Arc<dyn ResourceShareStore>,
    credential_store: Arc<dyn custody::BridgeCredentialStore>,
    provisioning: Option<Arc<provisioning::BridgeProvisioningDependencies>>,
    flags: crate::config::BridgeFeatureFlags,
) -> Result<(BridgeService, Option<Arc<ManagedBridgeCredentials>>), BridgeError> {
    if let AuthMode::Provider { base_url, .. } = mode {
        let provider = Arc::new(NyxBridgeProvider::new(base_url)?);
        let credentials = Arc::new(ManagedBridgeCredentials {
            store: credential_store,
            provider: provider.clone(),
            bridges: store.clone(),
            auth: organizations.clone(),
            provisioning: provisioning.clone(),
        });
        let connections = Arc::new(NyxBridgeValidator {
            custody: credentials.clone(),
            resources,
            provisioning,
        });
        Ok((
            BridgeService::new(
                store,
                organizations,
                feature_policy::configured(connections, flags),
            ),
            Some(credentials),
        ))
    } else {
        Ok((
            BridgeService::new(
                store,
                organizations,
                feature_policy::configured(Arc::new(UnavailableConnections), flags),
            ),
            None,
        ))
    }
}

struct UnavailableConnections;
#[async_trait::async_trait]
impl BridgeConnectionValidator for UnavailableConnections {
    fn capabilities(&self) -> layer2_core::bridge::BridgeCapabilityCatalog {
        capability_catalog::catalog(false)
    }
    async fn validate_connection(
        &self,
        _: &ResourceOwner,
        _: &BridgeConfig,
    ) -> Result<BridgeValidation, BridgeError> {
        Err(BridgeError::ConnectionRequired(
            BridgeSetupBlock::CustodyUnavailable,
        ))
    }
}

struct NyxBridgeValidator {
    custody: Arc<dyn BridgeCredentialResolver>,
    resources: Arc<dyn ResourceShareStore>,
    provisioning: Option<Arc<provisioning::BridgeProvisioningDependencies>>,
}

#[async_trait::async_trait]
impl BridgeConnectionValidator for NyxBridgeValidator {
    fn capabilities(&self) -> layer2_core::bridge::BridgeCapabilityCatalog {
        capability_catalog::catalog(true)
    }
    async fn validate_connection(
        &self,
        owner: &ResourceOwner,
        config: &BridgeConfig,
    ) -> Result<BridgeValidation, BridgeError> {
        config.validate()?;
        let started = tokio::time::Instant::now();
        let lease = self.custody.resolve(owner, config).await?;
        if &lease.owner != owner
            || lease.credential_subject != *types::owner_subject(owner)
            || lease.issuer != config.connector.issuer
            || lease.key_id != config.connector.key_id
            || lease.custody != config.connector.custody
            || lease.custody_revision == 0
            || lease.key_state_version <= 0
            || lease.deadline <= tokio::time::Instant::now()
            || lease.value.expose().is_empty()
            || lease.value.expose().len() > 4096
        {
            return Err(BridgeError::ConnectionRequired(
                BridgeSetupBlock::CustodyUnavailable,
            ));
        }
        let mut reused_snapshot = None;
        let existing_target = match &config.placement {
            BridgePlacementPolicy::WorkspacePerPartition { .. }
            | BridgePlacementPolicy::WorktreeAgentPerPartition { .. } => {
                let (_, authorized, _) = self
                    .provisioning
                    .as_ref()
                    .ok_or_else(control_plane::reconnect)?
                    .reuse_preflight(&lease.control_plane, config, None)
                    .await?;
                reused_snapshot = Some(authorized.snapshot);
                config.placement.reused_parent()
            }
            BridgePlacementPolicy::ExistingAgent { target } => Some(ResourceTarget::Agent {
                agent_id: target.agent_id.clone(),
            }),
            BridgePlacementPolicy::NewSandbox { .. } => {
                self.provisioning
                    .as_ref()
                    .ok_or_else(control_plane::reconnect)?
                    .preflight(
                        &lease.control_plane,
                        config,
                        lease.control_plane.bridge_id().as_str(),
                        None,
                    )
                    .await?;
                None
            }
        };
        let current = if let Some(target) = existing_target {
            let snapshot = if let Some(snapshot) = reused_snapshot {
                snapshot
            } else {
                self.resources
                    .resource_access_snapshot(&target)
                    .await
                    .map_err(target_error)?
            };
            self.custody.revalidate(&lease).await?;
            let current = self
                .resources
                .resource_access_snapshot(&target)
                .await
                .map_err(target_error)?;
            if current.fence() != snapshot.fence() || current.lineage != snapshot.lineage {
                return Err(BridgeError::ConnectionRequired(
                    BridgeSetupBlock::TargetMismatch,
                ));
            }
            Some(current)
        } else {
            None
        };
        self.custody.revalidate(&lease).await?;
        let creates_children = config.placement.creates_partition_agent();
        BridgeValidation::from_observation(
            owner,
            config,
            BridgeConnectionObservation {
                connector: config.connector.clone(),
                bot_owner: owner.clone(),
                key_owner: owner.clone(),
                routes: lease.routes,
                custody_owner: lease.owner,
                custody_revision: lease.custody_revision,
                key_state_version: lease.key_state_version,
                key_active: true,
                custody_available: true,
                callback_conflict: false,
                renewable_sandbox_authority: creates_children,
                exact_profiles_available: creates_children,
                existing_target: current,
                deadline: lease.deadline.min(started + Duration::from_secs(60)),
            },
        )
    }
}

fn target_error(error: layer2_core::resource_access::ResourceAccessStoreError) -> BridgeError {
    match error {
        layer2_core::resource_access::ResourceAccessStoreError::Unavailable => {
            BridgeError::AuthorityUnavailable
        }
        _ => BridgeError::ConnectionRequired(BridgeSetupBlock::TargetMismatch),
    }
}
