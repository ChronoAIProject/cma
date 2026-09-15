use std::time::Duration;

use crate::resource_access::{ResourceAccessSnapshot, ResourceOwner, ResourceTarget};

use super::{
    BridgeConfig, BridgeConnector, BridgeError, BridgePlacementPolicy, BridgeSecretRef,
    BridgeSetupBlock,
};

/// Facts from existing NyxID interfaces and the selected encrypted custody
/// entry. This type cannot be deserialized from browser or callback payloads.
#[derive(Debug, Clone)]
pub struct BridgeRouteObservation {
    pub route_id: String,
    pub owner: ResourceOwner,
    pub bot_id: String,
    pub key_id: String,
    pub enabled: bool,
}

#[derive(Debug, Clone)]
pub struct BridgeConnectionObservation {
    pub connector: BridgeConnector,
    pub bot_owner: ResourceOwner,
    pub key_owner: ResourceOwner,
    pub routes: Vec<BridgeRouteObservation>,
    pub custody_owner: ResourceOwner,
    pub custody_revision: u64,
    pub key_state_version: i64,
    pub key_active: bool,
    pub custody_available: bool,
    pub callback_conflict: bool,
    pub renewable_sandbox_authority: bool,
    pub exact_profiles_available: bool,
    pub existing_target: Option<ResourceAccessSnapshot>,
    /// Bound by the start of provider IO, key expiry and custody verification.
    pub deadline: tokio::time::Instant,
}

/// Request-local connection proof. Prior health or persisted configuration is
/// never sufficient to enable or admit execution. No Serialize/Deserialize.
#[derive(Debug, Clone)]
pub struct BridgeValidation {
    owner: ResourceOwner,
    digest: String,
    custody: BridgeCustodyPin,
    deadline: tokio::time::Instant,
}

/// Exact local custody observation retained through storage admission. Stores
/// must compare this with current encrypted custody in their mutation snapshot;
/// a deadline alone cannot detect a revocation after provider verification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BridgeCustodyPin {
    reference: BridgeSecretRef,
    revision: u64,
    key_state_version: i64,
}

impl BridgeCustodyPin {
    pub fn reference(&self) -> &BridgeSecretRef {
        &self.reference
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn key_state_version(&self) -> i64 {
        self.key_state_version
    }
}

impl BridgeValidation {
    pub fn from_observation(
        owner: &ResourceOwner,
        config: &BridgeConfig,
        observation: BridgeConnectionObservation,
    ) -> Result<Self, BridgeError> {
        config.validate()?;
        let blocked = |reason| BridgeError::ConnectionRequired(reason);
        let now = tokio::time::Instant::now();
        if observation.deadline <= now || observation.deadline > now + Duration::from_secs(60) {
            return Err(blocked(BridgeSetupBlock::AuthorityExpired));
        }
        let routes: std::collections::BTreeSet<_> = observation
            .routes
            .iter()
            .map(|route| &route.route_id)
            .collect();
        let selected: std::collections::BTreeSet<_> = config.connector.route_ids.iter().collect();
        if observation.connector != config.connector
            || &observation.bot_owner != owner
            || &observation.key_owner != owner
            || !observation.key_active
            || routes != selected
            || observation.routes.len() != selected.len()
            || observation.routes.iter().any(|route| {
                &route.owner != owner
                    || route.bot_id != config.connector.bot_id
                    || route.key_id != config.connector.key_id
                    || !route.enabled
            })
        {
            return Err(blocked(BridgeSetupBlock::BindingMismatch));
        }
        if observation.callback_conflict {
            return Err(blocked(BridgeSetupBlock::CallbackConflict));
        }
        if &observation.custody_owner != owner
            || !observation.custody_available
            || observation.custody_revision == 0
            || observation.custody_revision > i64::MAX as u64
            || observation.key_state_version <= 0
        {
            return Err(blocked(BridgeSetupBlock::CustodyUnavailable));
        }
        match &config.placement {
            BridgePlacementPolicy::ExistingAgent { target } => {
                let snapshot = observation
                    .existing_target
                    .as_ref()
                    .ok_or_else(|| blocked(BridgeSetupBlock::TargetMismatch))?;
                let resource = ResourceTarget::Agent {
                    agent_id: target.agent_id.clone(),
                };
                let scope = snapshot
                    .scope(&resource)
                    .map_err(|_| blocked(BridgeSetupBlock::TargetMismatch))?;
                if &scope.owner != owner
                    || snapshot.lineage.sandbox.id != target.sandbox_id
                    || snapshot
                        .lineage
                        .workspace
                        .as_ref()
                        .is_none_or(|w| w.id != target.workspace_id)
                    || snapshot
                        .lineage
                        .worktree
                        .as_ref()
                        .is_none_or(|w| w.id != target.worktree_id)
                {
                    return Err(blocked(BridgeSetupBlock::TargetMismatch));
                }
            }
            BridgePlacementPolicy::NewSandbox { .. } => {
                if !observation.renewable_sandbox_authority {
                    return Err(blocked(BridgeSetupBlock::RenewableAuthorityUnavailable));
                }
                if !observation.exact_profiles_available {
                    return Err(blocked(BridgeSetupBlock::ProfileUnavailable));
                }
            }
        }
        Ok(Self {
            owner: owner.clone(),
            digest: super::store::digest(config)?,
            custody: BridgeCustodyPin {
                reference: config.connector.custody.clone(),
                revision: observation.custody_revision,
                key_state_version: observation.key_state_version,
            },
            deadline: observation.deadline,
        })
    }

    pub(super) fn require(
        &self,
        owner: &ResourceOwner,
        config: &BridgeConfig,
    ) -> Result<(), BridgeError> {
        if &self.owner != owner || self.digest != super::store::digest(config)? {
            return Err(BridgeError::ConnectionRequired(
                BridgeSetupBlock::BindingMismatch,
            ));
        }
        if tokio::time::Instant::now() >= self.deadline {
            return Err(BridgeError::ConnectionRequired(
                BridgeSetupBlock::AuthorityExpired,
            ));
        }
        Ok(())
    }

    pub(super) fn deadline(&self) -> tokio::time::Instant {
        self.deadline
    }

    pub fn custody_pin(&self) -> &BridgeCustodyPin {
        &self.custody
    }
}

/// Read-only provider/custody verification. Implementations must resolve the
/// selected existing bot/key/routes, verify ownership and shared-key callback
/// use, and resolve exact target ancestry or pinned profiles. This port never
/// authorizes creating a bot or rewriting a callback/route.
#[async_trait::async_trait]
pub trait BridgeConnectionValidator: Send + Sync {
    async fn validate_connection(
        &self,
        owner: &ResourceOwner,
        config: &BridgeConfig,
    ) -> Result<BridgeValidation, BridgeError>;
}
