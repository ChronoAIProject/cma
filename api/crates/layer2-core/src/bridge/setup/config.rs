use serde::{Deserialize, Serialize};

use super::super::*;

/// Owner-selected existing identities. Deployment endpoints and local custody
/// locators are supplied by CMA and never accepted from management JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeSelectedBinding {
    pub bot_id: String,
    pub platform: BridgePlatform,
    pub route_ids: Vec<String>,
    pub key_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeSetupConfig {
    pub display_name: String,
    pub binding: BridgeSelectedBinding,
    pub partition: BridgePartitionPolicy,
    pub placement: BridgePlacementPolicy,
    pub admission: BridgeAdmissionPolicy,
    pub reply: BridgeReplyPolicy,
    pub limits: BridgeLimits,
}

impl From<&BridgeConfig> for BridgeSetupConfig {
    fn from(value: &BridgeConfig) -> Self {
        Self {
            display_name: value.display_name.clone(),
            binding: BridgeSelectedBinding {
                bot_id: value.connector.bot_id.clone(),
                platform: value.connector.platform.clone(),
                route_ids: value.connector.route_ids.clone(),
                key_id: value.connector.key_id.clone(),
            },
            partition: value.partition,
            placement: value.placement.clone(),
            admission: value.admission.clone(),
            reply: value.reply.clone(),
            limits: value.limits.clone(),
        }
    }
}

#[derive(Clone)]
pub struct BridgeSetupPolicy {
    issuer: String,
    base_url: String,
    callback_root: String,
}
impl BridgeSetupPolicy {
    pub fn new(
        issuer: String,
        base_url: String,
        callback_root: String,
    ) -> Result<Self, BridgeError> {
        for (value, callback) in [(&issuer, false), (&base_url, false), (&callback_root, true)] {
            let url = url::Url::parse(value)
                .map_err(|_| BridgeError::InvalidConfiguration("deployment_url"))?;
            if !(url.scheme() == "https" || (!callback && url.scheme() == "http"))
                || url.host_str().is_none()
                || !url.username().is_empty()
                || url.password().is_some()
                || url.query().is_some()
                || url.fragment().is_some()
                || value.ends_with('/')
                || (!callback && url.path() != "/")
            {
                return Err(BridgeError::InvalidConfiguration("deployment_url"));
            }
        }
        Ok(Self {
            issuer,
            base_url,
            callback_root,
        })
    }
    pub fn callback_url(&self, id: &BridgeId) -> String {
        format!("{}/{}", self.callback_root, id.as_str())
    }
    pub(super) fn configure(
        &self,
        id: &BridgeId,
        value: BridgeSetupConfig,
    ) -> Result<BridgeConfig, BridgeError> {
        let mut routes = value.binding.route_ids.clone();
        routes.sort();
        let digest = super::super::store::digest(&(
            "cma-bridge-selected-custody-v1",
            id,
            &self.issuer,
            &value.binding.bot_id,
            &value.binding.key_id,
            &routes,
        ))?;
        let config = BridgeConfig {
            display_name: value.display_name,
            connector: BridgeConnector {
                issuer: self.issuer.clone(),
                base_url: self.base_url.clone(),
                bot_id: value.binding.bot_id,
                platform: value.binding.platform,
                route_ids: routes,
                key_id: value.binding.key_id,
                custody: BridgeSecretRef::new(format!("brsec_{}", &digest[..32]))?,
                callback_url: self.callback_url(id),
            },
            partition: value.partition,
            placement: value.placement,
            admission: value.admission,
            reply: value.reply,
            limits: value.limits,
        };
        config.validate()?;
        config.validate_partition_admission()?;
        Ok(config)
    }
}
