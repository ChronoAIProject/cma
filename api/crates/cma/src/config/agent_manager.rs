use layer2_core::agent_ownership::DEFAULT_MAX_OWNED_AGENTS;

const SKIP_INELIGIBLE_RECONCILIATION: &str =
    "CHRONO_AGENT_RECONCILIATION_SKIP_INELIGIBLE_OWNERSHIP_ENABLED";
const DEFER_FOREIGN_RECONCILIATION: &str =
    "CHRONO_AGENT_RECONCILIATION_DEFER_FOREIGN_OWNERSHIP_ENABLED";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentManagerReplicas {
    pub relay_address: String,
    pub max_owned_agents: u32,
    pub skip_ineligible_reconciliation: bool,
    pub defer_foreign_reconciliation: bool,
}

impl AgentManagerReplicas {
    pub(super) fn from_lookup(
        lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
    ) -> Result<Option<Self>, String> {
        match lookup("CHRONO_AGENT_MANAGER_REPLICAS_ENABLED")?.as_deref() {
            None | Some("false") => return Ok(None),
            Some("true") => (),
            _ => return Err("CHRONO_AGENT_MANAGER_REPLICAS_ENABLED must be true or false".into()),
        }
        let relay_address = super::required(lookup, "CHRONO_AGENT_MANAGER_RELAY_ADDRESS")?;
        let parsed = url::Url::parse(&relay_address)
            .map_err(|_| "CHRONO_AGENT_MANAGER_RELAY_ADDRESS must be an absolute HTTP origin")?;
        if relay_address.len() > 2_048
            || !matches!(parsed.scheme(), "http" | "https")
            || parsed.host().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
            || parsed.path() != "/"
        {
            return Err("CHRONO_AGENT_MANAGER_RELAY_ADDRESS must be a bounded HTTP origin without credentials or a path".into());
        }
        let max_owned_agents = super::positive_u32(
            lookup,
            "CHRONO_AGENT_MANAGER_MAX_OWNED_AGENTS",
            DEFAULT_MAX_OWNED_AGENTS,
            1,
        )?
        .get();
        Ok(Some(Self {
            relay_address: parsed.to_string(),
            max_owned_agents,
            skip_ineligible_reconciliation: super::features::flag(
                lookup,
                SKIP_INELIGIBLE_RECONCILIATION,
            )?,
            defer_foreign_reconciliation: super::features::flag(
                lookup,
                DEFER_FOREIGN_RECONCILIATION,
            )?,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switch_off_does_not_read_or_require_any_replica_configuration() {
        for flag in [None, Some("false")] {
            let mut keys = Vec::new();
            let result = AgentManagerReplicas::from_lookup(&mut |key| {
                keys.push(key.to_owned());
                assert_eq!(key, "CHRONO_AGENT_MANAGER_REPLICAS_ENABLED");
                Ok(flag.map(str::to_owned))
            })
            .unwrap();
            assert_eq!(result, None);
            assert_eq!(keys.len(), 1);
        }
    }

    fn config(
        flag: &str,
        address: &str,
        limit: &str,
    ) -> Result<Option<AgentManagerReplicas>, String> {
        AgentManagerReplicas::from_lookup(&mut |key| {
            if matches!(
                key,
                SKIP_INELIGIBLE_RECONCILIATION | DEFER_FOREIGN_RECONCILIATION
            ) {
                return Ok(None);
            }
            Ok(Some(
                match key {
                    "CHRONO_AGENT_MANAGER_REPLICAS_ENABLED" => flag,
                    "CHRONO_AGENT_MANAGER_RELAY_ADDRESS" => address,
                    "CHRONO_AGENT_MANAGER_MAX_OWNED_AGENTS" => limit,
                    _ => panic!("unexpected key"),
                }
                .into(),
            ))
        })
    }

    #[test]
    fn switch_on_validates_peer_address_and_positive_capacity() {
        assert_eq!(
            config("true", "http://10.0.0.1:8080", "256").unwrap(),
            Some(AgentManagerReplicas {
                relay_address: "http://10.0.0.1:8080/".into(),
                max_owned_agents: 256,
                skip_ineligible_reconciliation: false,
                defer_foreign_reconciliation: false,
            })
        );
        for flag in ["1", "TRUE", "true ", "", "yes"] {
            assert!(config(flag, "http://peer:8080", "256").is_err());
        }
        for address in [
            "peer:8080",
            "file:///tmp/peer",
            "http://user:pass@peer",
            "http://peer/path",
            "http://peer?token=synthetic",
            "http://peer#fragment",
        ] {
            assert!(config("true", address, "256").is_err());
        }
        assert!(config("true", "http://peer", "0").is_err());
    }

    #[test]
    fn reconciliation_ownership_optimizations_require_independent_strict_opt_ins() {
        for option in [SKIP_INELIGIBLE_RECONCILIATION, DEFER_FOREIGN_RECONCILIATION] {
            for (value, expected) in [
                (None, Ok(false)),
                (Some("false"), Ok(false)),
                (Some("true"), Ok(true)),
                (Some("1"), Err(())),
            ] {
                let config = AgentManagerReplicas::from_lookup(&mut |key| {
                    Ok(match key {
                        "CHRONO_AGENT_MANAGER_REPLICAS_ENABLED" => Some("true".into()),
                        "CHRONO_AGENT_MANAGER_RELAY_ADDRESS" => {
                            Some("http://fixture.example".into())
                        }
                        key if key == option => value.map(str::to_owned),
                        _ => None,
                    })
                });
                let actual = config
                    .map(|replicas| {
                        let replicas = replicas.unwrap();
                        (
                            replicas.skip_ineligible_reconciliation,
                            replicas.defer_foreign_reconciliation,
                        )
                    })
                    .map_err(|_| ());
                assert_eq!(
                    actual,
                    expected.map(|enabled| (
                        enabled && option == SKIP_INELIGIBLE_RECONCILIATION,
                        enabled && option == DEFER_FOREIGN_RECONCILIATION,
                    ))
                );
            }
        }
    }
}
