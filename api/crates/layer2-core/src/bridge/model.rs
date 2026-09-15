use serde::{Deserialize, Serialize};

use crate::resource_access::{ResourceActor, ResourceOwner};

use super::{BridgeConfig, BridgePlatform};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BridgeId(pub String);

impl BridgeId {
    pub fn generate() -> Self {
        Self(format!("brg_{}", uuid::Uuid::new_v4().simple()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BridgeError {
    #[error("bridge not found")]
    NotFound,
    #[error("this bridge does not permit the requested operation")]
    Forbidden,
    #[error("current bridge authority is unavailable")]
    AuthorityUnavailable,
    #[error("bridge configuration field is invalid: {0}")]
    InvalidConfiguration(&'static str),
    #[error("bridge revision changed")]
    Conflict,
    #[error("idempotency key was already used for a different bridge command")]
    IdempotencyConflict,
    #[error("bridge identity is immutable; create a new bridge")]
    ImmutableIdentity,
    #[error("fixed Agent rebinding requires an explicit accepted-work policy")]
    RebindingPolicyRequired,
    #[error("bridge is retired")]
    Retired,
    #[error("bridge setup requires a verified connection: {0:?}")]
    ConnectionRequired(BridgeSetupBlock),
    #[error("bridge is not accepting new work")]
    AdmissionStopped,
    #[error("bridge storage is unavailable")]
    Unavailable,
    #[error("stored bridge data is invalid")]
    Corrupt,
    #[error("bridge configuration history has reached its supported bound")]
    HistoryLimit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BridgeSetupBlock {
    BindingMismatch,
    CallbackConflict,
    CustodyUnavailable,
    RenewableAuthorityUnavailable,
    TargetMismatch,
    InvokeConsentRequired,
    ProfileUnavailable,
    AuthorityExpired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcceptedWorkPolicy {
    DrainOriginal,
    SuspendOriginal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BridgeState {
    Disabled { accepted_work: AcceptedWorkPolicy },
    Enabled,
    Retired { accepted_work: AcceptedWorkPolicy },
}

/// Independent reversible controls. These never grant an effect permission;
/// each worker must still check current owner/key authority and pinned routing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeEmergencyControls {
    pub ingress_stopped: bool,
    pub execution_stopped: bool,
    pub outbound_stopped: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeGeneration {
    pub config_revision: u64,
    pub config: BridgeConfig,
    pub created_at: String,
    pub actor: ResourceActor,
    /// Applies only to accepted work from earlier generations. Routing remains
    /// bound to each original generation under both policies.
    pub previous_work: Option<AcceptedWorkPolicy>,
}

/// Internal durable aggregate. Public management must project `BridgeView`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeRecord {
    pub id: BridgeId,
    pub owner: ResourceOwner,
    pub revision: u64,
    pub config_revision: u64,
    pub state: BridgeState,
    pub emergency: BridgeEmergencyControls,
    pub generations: Vec<BridgeGeneration>,
    pub created_at: String,
    pub created_by: ResourceActor,
    pub updated_at: String,
}

impl BridgeRecord {
    pub fn current(&self) -> Result<&BridgeGeneration, BridgeError> {
        self.generations
            .last()
            .filter(|g| g.config_revision == self.config_revision)
            .ok_or(BridgeError::Corrupt)
    }

    pub fn validate(&self) -> Result<(), BridgeError> {
        let owner = match &self.owner {
            ResourceOwner::Person { subject } | ResourceOwner::Organization { subject } => subject,
        };
        if !super::config::valid_id(self.id.as_str())
            || !super::config::valid_id(owner.as_str())
            || self.revision == 0
            || self.revision > i64::MAX as u64
            || self.generations.is_empty()
            || self.generations.len() > 256
            || self.config_revision != self.generations.len() as u64
            || self.config_revision > self.revision
            || crate::seconds_from_rfc3339(&self.created_at).is_none()
            || crate::seconds_from_rfc3339(&self.updated_at).is_none()
            || !management_actor(&self.created_by)
        {
            return Err(BridgeError::Corrupt);
        }
        let identity = &self.generations[0].config.connector;
        if self.generations[0].actor != self.created_by
            || self.generations[0].created_at != self.created_at
        {
            return Err(BridgeError::Corrupt);
        }
        for (index, generation) in self.generations.iter().enumerate() {
            generation
                .config
                .validate()
                .map_err(|_| BridgeError::Corrupt)?;
            let connector = &generation.config.connector;
            if generation.config_revision != index as u64 + 1
                || connector.bot_id != identity.bot_id
                || connector.platform != identity.platform
                || connector.issuer != identity.issuer
                || connector.base_url != identity.base_url
                || crate::seconds_from_rfc3339(&generation.created_at).is_none()
                || !management_actor(&generation.actor)
            {
                return Err(BridgeError::Corrupt);
            }
        }
        self.current().map(|_| ())
    }

    /// Capture this at admission and retain it with the accepted inbox record.
    /// It is an identity pin, never authorization for deferred execution/reply.
    pub fn admission_pin(&self) -> Result<BridgeWorkPin, BridgeError> {
        self.validate()?;
        if self.state != BridgeState::Enabled || self.emergency.ingress_stopped {
            return Err(BridgeError::AdmissionStopped);
        }
        Ok(BridgeWorkPin {
            bridge_id: self.id.clone(),
            config_revision: self.config_revision,
        })
    }

    pub fn pinned(&self, pin: &BridgeWorkPin) -> Result<&BridgeGeneration, BridgeError> {
        self.validate()?;
        if pin.bridge_id != self.id {
            return Err(BridgeError::NotFound);
        }
        self.generations
            .iter()
            .find(|g| g.config_revision == pin.config_revision)
            .ok_or(BridgeError::NotFound)
    }
}

fn management_actor(actor: &ResourceActor) -> bool {
    matches!(actor, ResourceActor::Human { subject } | ResourceActor::Api { subject, delegation_id: None }
        if super::config::valid_id(subject.as_str()))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeWorkPin {
    pub bridge_id: BridgeId,
    pub config_revision: u64,
}

/// The complete nonsecret visibility projection for an organization member.
/// Selected key, routes, callback and custody references never enter this DTO.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BridgeView {
    pub id: BridgeId,
    pub owner: ResourceOwner,
    pub display_name: String,
    pub revision: u64,
    pub config_revision: u64,
    pub state: BridgeState,
    pub emergency: BridgeEmergencyControls,
    pub bot_id: String,
    pub platform: BridgePlatform,
    pub can_manage: bool,
    pub created_at: String,
    pub updated_at: String,
}

impl BridgeView {
    pub(super) fn from_record(
        record: &BridgeRecord,
        can_manage: bool,
    ) -> Result<Self, BridgeError> {
        record.validate()?;
        let config = &record.current()?.config;
        Ok(Self {
            id: record.id.clone(),
            owner: record.owner.clone(),
            display_name: config.display_name.clone(),
            revision: record.revision,
            config_revision: record.config_revision,
            state: record.state.clone(),
            emergency: record.emergency,
            bot_id: config.connector.bot_id.clone(),
            platform: config.connector.platform.clone(),
            can_manage,
            created_at: record.created_at.clone(),
            updated_at: record.updated_at.clone(),
        })
    }
}
