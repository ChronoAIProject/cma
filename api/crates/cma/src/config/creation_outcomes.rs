use layer1_public::rest::creation_outcomes::lower_hex;

#[derive(Clone)]
pub(crate) struct CreationOutcomesConfig {
    pub(crate) deployment: String,
    pub(crate) epoch: String,
    pub(crate) first_day: i64,
    pub(crate) legacy_custody: bool,
    pub(crate) terminal_settlement: bool,
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
        let terminal_settlement = super::features::flag(
            lookup,
            "CHRONO_SANDBOX_CREATION_TERMINAL_SETTLEMENT_ENABLED",
        )?;
        Ok(Some(Self {
            deployment,
            epoch,
            first_day,
            legacy_custody,
            terminal_settlement,
        }))
    }
}
