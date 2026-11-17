#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RelayCapacityConfig {
    pub limit: usize,
    pub control_reserve: usize,
}
impl Default for RelayCapacityConfig {
    fn default() -> Self {
        Self {
            limit: 256,
            control_reserve: 0,
        }
    }
}
impl RelayCapacityConfig {
    pub(super) fn from_lookup(
        lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
    ) -> Result<Self, String> {
        let limit = super::runtime_capacity::number(
            lookup,
            "CHRONO_AGENT_MANAGER_RELAY_STREAM_LIMIT",
            256,
            1,
            4096,
        )?;
        let control_reserve = super::runtime_capacity::number(
            lookup,
            "CHRONO_AGENT_MANAGER_RELAY_CONTROL_RESERVE",
            0,
            0,
            4095,
        )?;
        if control_reserve >= limit {
            return Err("CHRONO_AGENT_MANAGER_RELAY_CONTROL_RESERVE must be less than CHRONO_AGENT_MANAGER_RELAY_STREAM_LIMIT".into());
        }
        Ok(Self {
            limit,
            control_reserve,
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn defaults_preserve_shared_256_and_activation_is_strict() {
        assert_eq!(
            RelayCapacityConfig::from_lookup(&mut |_| Ok(None)).unwrap(),
            RelayCapacityConfig::default()
        );
        let parse = |limit: &str, reserve: &str| {
            RelayCapacityConfig::from_lookup(&mut |key| {
                Ok(Some(
                    if key.ends_with("LIMIT") {
                        limit
                    } else {
                        reserve
                    }
                    .into(),
                ))
            })
        };
        assert_eq!(
            parse("384", "64").unwrap(),
            RelayCapacityConfig {
                limit: 384,
                control_reserve: 64
            }
        );
        for (limit, reserve) in [
            ("0", "0"),
            ("4097", "0"),
            ("256", "256"),
            (" 256", "0"),
            ("256", "-1"),
            ("256", "+1"),
        ] {
            assert!(parse(limit, reserve).is_err());
        }
    }
}
