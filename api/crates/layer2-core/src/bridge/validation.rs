use std::time::Duration;

use crate::resource_access::{ResourceAccessSnapshot, ResourceOwner, ResourceTarget};

use super::{
    BridgeConfig, BridgeConnector, BridgeError, BridgePlacementPolicy, BridgeSecretRef,
    BridgeSetupBlock,
};

/// Internal request for current custody verification of one already-created
/// channel Agent. Only invocation authorization can prepare this request; it
/// cannot authorize provisioning, setup, or a different input/target.
#[derive(Clone)]
pub struct RetainedChannelConnection {
    grant: super::invoke::BridgeInvokeGrant,
    effect: super::invoke::BridgeInvokeEffect,
}

impl RetainedChannelConnection {
    pub(super) fn new(
        grant: &super::invoke::BridgeInvokeGrant,
        effect: &super::invoke::BridgeInvokeEffect,
        resources: &ResourceAccessSnapshot,
    ) -> Result<Self, super::invoke::BridgeInvokeError> {
        require_retained_channel(grant, effect)?;
        grant.target.require(&grant.owner, resources)?;
        grant
            .channel
            .as_ref()
            .expect("checked channel basis")
            .require_resources(resources)?;
        Ok(Self {
            grant: grant.clone(),
            effect: effect.clone(),
        })
    }

    pub fn owner(&self) -> &ResourceOwner {
        &self.grant.owner
    }
    pub fn config(&self) -> &BridgeConfig {
        &self.grant.config
    }
}

fn require_retained_channel(
    grant: &super::invoke::BridgeInvokeGrant,
    effect: &super::invoke::BridgeInvokeEffect,
) -> Result<(), super::invoke::BridgeInvokeError> {
    use super::invoke::{BridgeInvokeEffect, BridgeInvokeError};
    grant.require_active()?;
    effect.validate()?;
    if grant.channel.is_none()
        || !matches!(
            effect,
            BridgeInvokeEffect::AdmitMessage { .. } | BridgeInvokeEffect::ExecuteCommand { .. }
        )
    {
        return Err(BridgeInvokeError::TargetMismatch);
    }
    Ok(())
}

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
    /// None is the full setup/provisioning proof. A retained proof is an exact
    /// grant-and-effect digest and is rejected by every generic consumer.
    retained_channel: Option<String>,
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
        Self::observe(owner, config, observation, None)
    }

    pub fn from_retained_channel_observation(
        request: &RetainedChannelConnection,
        observation: BridgeConnectionObservation,
    ) -> Result<Self, BridgeError> {
        require_retained_channel(&request.grant, &request.effect)
            .map_err(|_| BridgeError::ConnectionRequired(BridgeSetupBlock::TargetMismatch))?;
        let scope = super::store::digest(&(&request.grant, &request.effect))?;
        Self::observe(request.owner(), request.config(), observation, Some(scope))
    }

    fn observe(
        owner: &ResourceOwner,
        config: &BridgeConfig,
        observation: BridgeConnectionObservation,
        retained_channel: Option<String>,
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
        if retained_channel.is_none() {
            match &config.placement {
                BridgePlacementPolicy::WorkspacePerPartition { sandbox_id, .. }
                | BridgePlacementPolicy::WorktreeAgentPerPartition { sandbox_id, .. } => {
                    let target = config
                        .placement
                        .reused_parent()
                        .ok_or_else(|| blocked(BridgeSetupBlock::IncompatibleTarget))?;
                    let resources = observation
                        .existing_target
                        .as_ref()
                        .ok_or_else(|| blocked(BridgeSetupBlock::IncompatibleTarget))?;
                    let scope = resources
                        .scope(&target)
                        .map_err(|_| blocked(BridgeSetupBlock::IncompatibleTarget))?;
                    if &scope.owner != owner
                        || &resources.lineage.sandbox.id != sandbox_id
                        || !observation.exact_profiles_available
                        || !observation.renewable_sandbox_authority
                    {
                        return Err(blocked(BridgeSetupBlock::IncompatibleTarget));
                    }
                }
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
            retained_channel,
        })
    }

    pub(super) fn require(
        &self,
        owner: &ResourceOwner,
        config: &BridgeConfig,
    ) -> Result<(), BridgeError> {
        if self.retained_channel.is_some() {
            return Err(BridgeError::ConnectionRequired(
                BridgeSetupBlock::TargetMismatch,
            ));
        }
        self.require_binding(owner, config)
    }

    pub(super) fn require_invocation(
        &self,
        grant: &super::invoke::BridgeInvokeGrant,
        effect: &super::invoke::BridgeInvokeEffect,
    ) -> Result<(), super::invoke::BridgeInvokeError> {
        if let Some(scope) = &self.retained_channel {
            require_retained_channel(grant, effect)?;
            if scope != &super::store::digest(&(grant, effect))? {
                return Err(super::invoke::BridgeInvokeError::TargetMismatch);
            }
        }
        self.require_binding(&grant.owner, &grant.config)?;
        Ok(())
    }

    fn require_binding(
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

/// Independently switchable boundaries for new Bridge effects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BridgeFeature {
    Management,
    Ingress,
    Allocation,
    Execution,
    Outbound,
}

/// Read-only provider/custody verification. Implementations must resolve the
/// selected existing bot/key/routes, verify ownership and shared-key callback
/// use, and resolve exact target ancestry or pinned profiles. This port never
/// authorizes creating a bot or rewriting a callback/route.
#[async_trait::async_trait]
pub trait BridgeConnectionValidator: Send + Sync {
    /// Additional deployment availability, never a substitute for authority.
    /// Call only for new effects; retained receipt/outcome recovery stays open.
    fn check_feature(
        &self,
        _feature: BridgeFeature,
        _platform: &super::BridgePlatform,
    ) -> Result<(), BridgeError> {
        Ok(())
    }

    /// Additional rollout policy for a fresh placement selection or allocation.
    /// Retained validation, execution and exact receipt recovery must bypass it.
    fn check_new_placement(&self, _config: &BridgeConfig) -> Result<(), BridgeError> {
        Ok(())
    }

    /// Unknown implementations expose no capabilities by default.
    fn capabilities(&self) -> super::BridgeCapabilityCatalog {
        super::BridgeCapabilityCatalog::default()
    }

    async fn validate_connection(
        &self,
        owner: &ResourceOwner,
        config: &BridgeConfig,
    ) -> Result<BridgeValidation, BridgeError>;

    /// Implementations may omit provisioning-only work for this exact retained
    /// input. The default preserves the full validation contract.
    async fn validate_retained_channel(
        &self,
        request: &RetainedChannelConnection,
    ) -> Result<BridgeValidation, BridgeError> {
        self.validate_connection(request.owner(), request.config())
            .await
    }
}
