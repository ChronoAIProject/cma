use super::*;
use crate::bridge::BridgeManagementLease;

#[derive(Debug, Clone, serde::Serialize)]
pub struct BridgeInvokeReadiness {
    pub bridge_id: crate::bridge::BridgeId,
    pub revision: u64,
    pub config_revision: u64,
    pub status: BridgeInvokeConsentStatus,
    pub candidate: Option<BridgeInvokeGrantView>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BridgeInvokeConsentStatus {
    NotApplicable,
    ConsentRequired,
    ConsentPresent,
}

impl BridgeInvokeService {
    /// Only reports explicit grant consent. Connection, enablement, allocation
    /// and deferred execution each retain their separate current checks.
    pub async fn readiness(
        &self,
        context: &crate::resource_access::CallerContext,
        credential: Option<&str>,
        bridge_id: &crate::bridge::BridgeId,
    ) -> Result<BridgeInvokeReadiness, BridgeInvokeError> {
        let management = self
            .management(
                context,
                credential,
                bridge_id,
                tokio::time::Instant::now() + std::time::Duration::from_secs(15),
            )
            .await?;
        let bridge = management.record();
        let candidate = self.current_candidate(&management).await?;
        let status = if !matches!(
            bridge.current()?.config.placement,
            crate::bridge::BridgePlacementPolicy::ExistingAgent { .. }
        ) {
            BridgeInvokeConsentStatus::NotApplicable
        } else if candidate.is_some() {
            BridgeInvokeConsentStatus::ConsentPresent
        } else {
            BridgeInvokeConsentStatus::ConsentRequired
        };
        Ok(BridgeInvokeReadiness {
            bridge_id: bridge.id.clone(),
            revision: bridge.revision,
            config_revision: bridge.config_revision,
            status,
            candidate,
        })
    }
    /// Management-only readiness observation, including while disabled. This is
    /// not an effect proof or an atomic enable prerequisite. The consumer must
    /// obtain current execution authority for each allocation/command effect.
    pub async fn current_candidate(
        &self,
        management: &BridgeManagementLease,
    ) -> Result<Option<BridgeInvokeGrantView>, BridgeInvokeError> {
        management.require_current()?;
        self.definitions.validate_management(management).await?;
        let bridge = management.record();
        let config = &bridge.current()?.config;
        let crate::bridge::BridgePlacementPolicy::ExistingAgent { target } = &config.placement
        else {
            return Ok(None);
        };
        let mut cursor = None;
        loop {
            let grants = self.store.list(&bridge.id, cursor.as_ref(), 100).await?;
            management.require_current()?;
            if grants.len() > 101 {
                return Err(BridgeInvokeError::Corrupt);
            }
            let mut previous = cursor.as_ref();
            for grant in &grants {
                super::service::require_owner(grant, bridge)?;
                if previous.is_some_and(|id| id >= &grant.id) {
                    return Err(BridgeInvokeError::Corrupt);
                }
                previous = Some(&grant.id);
            }
            for grant in grants.iter().take(100) {
                if grant.state != BridgeInvokeGrantState::Active
                    || grant.pin.config_revision != bridge.config_revision
                    || grant.config != *config
                    || &grant.target.ids != target
                {
                    continue;
                }
                let resources = self
                    .resources
                    .resource_access_snapshot(&grant.target.resource())
                    .await
                    .map_err(super::service::resource_store_error)?;
                match super::service::require_resource_owner(grant, &resources) {
                    Ok(()) => {}
                    Err(BridgeInvokeError::TargetMismatch | BridgeInvokeError::NotFound) => {
                        continue;
                    }
                    Err(error) => return Err(error),
                }
                let current = self
                    .store
                    .grant(&grant.id)
                    .await?
                    .ok_or(BridgeInvokeError::NotFound)?;
                if &current != grant {
                    return Err(BridgeInvokeError::Conflict);
                }
                self.definitions.validate_management(management).await?;
                return Ok(Some(grant.view()));
            }
            if grants.len() <= 100 {
                self.definitions.validate_management(management).await?;
                return Ok(None);
            }
            cursor = grants.get(99).map(|grant| grant.id.clone());
        }
    }
}
