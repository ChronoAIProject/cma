use layer1_public::rest::creation_outcomes::lower_hex;

const TERMINAL_SETTLEMENT: &str = "CHRONO_SANDBOX_CREATION_TERMINAL_SETTLEMENT_ENABLED";
const VALIDATION_SETTLEMENT: &str = "CHRONO_SANDBOX_CREATION_VALIDATION_SETTLEMENT_ENABLED";

#[derive(Clone)]
pub(crate) struct CreationOutcomesConfig {
    pub(crate) deployment: String,
    pub(crate) epoch: String,
    pub(crate) first_day: i64,
    pub(crate) legacy_custody: bool,
    pub(crate) terminal_settlement: bool,
    pub(crate) validation_settlement: bool,
}

impl CreationOutcomesConfig {
    pub(super) fn writer_epoch(
        lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
    ) -> Result<Option<String>, String> {
        let key = "CHRONO_SANDBOX_CREATION_OUTCOME_EPOCH";
        let value = lookup(key)?;
        if value.as_ref().is_some_and(|epoch| !lower_hex(epoch, 32)) {
            return Err(format!("{key} must contain 32 lowercase hex digits"));
        }
        Ok(value)
    }

    pub(super) fn from_lookup(
        lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
    ) -> Result<Option<Self>, String> {
        if !super::features::flag(lookup, "CHRONO_SANDBOX_CREATION_OUTCOME_LOOKUP_ENABLED")? {
            return Ok(None);
        }
        let mut hex_env = |key, length| {
            lookup(key)?
                .filter(|value| lower_hex(value, length))
                .ok_or_else(|| format!("{key} must contain {length} lowercase hex digits"))
        };
        let deployment = hex_env("CHRONO_SANDBOX_CREATION_OUTCOME_DEPLOYMENT", 64)?;
        let epoch = hex_env("CHRONO_SANDBOX_CREATION_OUTCOME_EPOCH", 32)?;
        let day_key = "CHRONO_SANDBOX_CREATION_OUTCOME_FIRST_UTC_DAY";
        let first_day = lookup(day_key)?
            .and_then(|value| {
                value
                    .parse::<u32>()
                    .ok()
                    .filter(|day| *day > 0 && day.to_string() == value)
            })
            .map(i64::from)
            .ok_or_else(|| format!("{day_key} must be a positive UTC Unix day"))?;
        let legacy_custody =
            super::features::flag(lookup, "CHRONO_SANDBOX_CREATION_OUTCOME_LEGACY_CUSTODY")?;
        // An enabled, configured coverage epoch must normally settle its
        // terminal proofs. Keep an explicit emergency off-switch for rollout.
        let terminal_settlement = match lookup(TERMINAL_SETTLEMENT)?.as_deref() {
            None | Some("true") => true,
            Some("false") => false,
            _ => return Err(format!("{TERMINAL_SETTLEMENT} must be true or false")),
        };
        let validation_settlement = super::features::flag(lookup, VALIDATION_SETTLEMENT)?;
        Ok(Some(Self {
            deployment,
            epoch,
            first_day,
            legacy_custody,
            terminal_settlement,
            validation_settlement,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_settlement_defaults_on_only_with_covered_lookup() {
        for (lookup_enabled, override_value, expected) in [
            (false, None, None),
            (false, Some("true"), None),
            (true, None, Some(true)),
            (true, Some("true"), Some(true)),
            (true, Some("false"), Some(false)),
        ] {
            let config = CreationOutcomesConfig::from_lookup(&mut |key| {
                Ok(match key {
                    "CHRONO_SANDBOX_CREATION_OUTCOME_LOOKUP_ENABLED" => {
                        Some(lookup_enabled.to_string())
                    }
                    "CHRONO_SANDBOX_CREATION_OUTCOME_DEPLOYMENT" => Some("d".repeat(64)),
                    "CHRONO_SANDBOX_CREATION_OUTCOME_EPOCH" => Some("e".repeat(32)),
                    "CHRONO_SANDBOX_CREATION_OUTCOME_FIRST_UTC_DAY" => Some("20000".into()),
                    TERMINAL_SETTLEMENT => override_value.map(str::to_owned),
                    _ => None,
                })
            })
            .unwrap();
            assert_eq!(
                config.map(|config| (config.terminal_settlement, config.validation_settlement)),
                expected.map(|enabled| (enabled, false))
            );
        }
    }

    #[test]
    fn terminal_settlement_rejects_invalid_configuration() {
        let config = CreationOutcomesConfig::from_lookup(&mut |key| {
            Ok(match key {
                "CHRONO_SANDBOX_CREATION_OUTCOME_LOOKUP_ENABLED" => Some("true".into()),
                "CHRONO_SANDBOX_CREATION_OUTCOME_DEPLOYMENT" => Some("d".repeat(64)),
                "CHRONO_SANDBOX_CREATION_OUTCOME_EPOCH" => Some("e".repeat(32)),
                "CHRONO_SANDBOX_CREATION_OUTCOME_FIRST_UTC_DAY" => Some("20000".into()),
                TERMINAL_SETTLEMENT => Some("invalid".into()),
                _ => None,
            })
        });
        assert!(matches!(config, Err(message) if message.contains(TERMINAL_SETTLEMENT)));
    }
}
