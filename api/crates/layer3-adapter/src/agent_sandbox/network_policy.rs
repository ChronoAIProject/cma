//! Operator-owned Template policy. The controller owns selectors and NetworkPolicies.
use k8s_openapi::api::networking::v1::{NetworkPolicyEgressRule, NetworkPolicyIngressRule};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::rejected;
use crate::Result;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct NativeTemplateNetworkPolicy {
    ingress: Vec<NetworkPolicyIngressRule>,
    egress: Vec<NetworkPolicyEgressRule>,
}
// Kubernetes rules contain only strings, integer ports and structural selectors.
impl Eq for NativeTemplateNetworkPolicy {}

impl<'de> Deserialize<'de> for NativeTemplateNetworkPolicy {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Rules {
            ingress: Vec<NetworkPolicyIngressRule>,
            egress: Vec<NetworkPolicyEgressRule>,
        }
        let raw = Value::deserialize(deserializer)?;
        let rules: Rules = serde_json::from_value(raw.clone()).map_err(serde::de::Error::custom)?;
        let policy = Self {
            ingress: rules.ingress,
            egress: rules.egress,
        };
        // Generated Kubernetes decoders otherwise silently discard unknown fields.
        if serde_json::to_value(&policy).map_err(serde::de::Error::custom)? != raw {
            return Err(serde::de::Error::custom(
                "unknown or noncanonical network policy fields",
            ));
        }
        policy.validate().map_err(serde::de::Error::custom)?;
        Ok(policy)
    }
}

impl NativeTemplateNetworkPolicy {
    pub fn parse(raw: &str) -> Result<Self> {
        if raw.len() > 64 * 1024 {
            return Err(rejected("native Template network policy exceeds its bound"));
        }
        serde_json::from_str(raw).map_err(|_| rejected("invalid native Template network policy"))
    }

    pub(crate) fn validate(&self) -> Result<()> {
        let invalid = || rejected("invalid or oversized native Template network policy");
        if self.ingress.len() > 64
            || self.egress.len() > 64
            || serde_json::to_vec(self).map_err(|_| invalid())?.len() > 64 * 1024
        {
            return Err(invalid());
        }
        for (peers, ports) in self
            .ingress
            .iter()
            .map(|r| (&r.from, &r.ports))
            .chain(self.egress.iter().map(|r| (&r.to, &r.ports)))
        {
            if peers.as_ref().is_some_and(|v| v.len() > 64)
                || ports.as_ref().is_some_and(|v| v.len() > 64)
            {
                return Err(invalid());
            }
            for peer in peers.iter().flatten() {
                if let Some(block) = &peer.ip_block {
                    if peer.namespace_selector.is_some() || peer.pod_selector.is_some() {
                        return Err(invalid());
                    }
                    let network = block.cidr.parse::<ipnet::IpNet>().map_err(|_| invalid())?;
                    if network.to_string() != block.cidr
                        || network.trunc() != network
                        || block.except.as_ref().is_some_and(|v| v.len() > 64)
                    {
                        return Err(invalid());
                    }
                    for raw in block.except.iter().flatten() {
                        let excluded = raw.parse::<ipnet::IpNet>().map_err(|_| invalid())?;
                        if excluded.to_string() != *raw
                            || excluded.trunc() != excluded
                            || !network.contains(&excluded)
                            || excluded == network
                        {
                            return Err(invalid());
                        }
                    }
                }
            }
            for port in ports.iter().flatten() {
                if port
                    .protocol
                    .as_deref()
                    .is_some_and(|p| !matches!(p, "TCP" | "UDP" | "SCTP"))
                {
                    return Err(invalid());
                }
                let number = match &port.port {
                    Some(k8s_openapi::apimachinery::pkg::util::intstr::IntOrString::Int(n))
                        if (1..=65535).contains(n) =>
                    {
                        Some(*n)
                    }
                    None => None,
                    _ => return Err(invalid()),
                };
                if port
                    .end_port
                    .is_some_and(|end| number.is_none_or(|begin| end < begin || end > 65535))
                {
                    return Err(invalid());
                }
            }
        }
        Ok(())
    }
}
