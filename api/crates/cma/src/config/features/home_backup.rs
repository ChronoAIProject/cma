//! Capture configuration is parsed even while its writer is disabled.
#[derive(Clone, Debug)]
pub(crate) struct HomeBackupConfig {
    pub snapshot_class: Option<String>,
    pub interval_seconds: u64,
    pub max_in_flight: u16,
}

impl HomeBackupConfig {
    pub(crate) fn from_lookup(
        lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
        enabled: bool,
    ) -> Result<Self, String> {
        let configured = Self {
            snapshot_class: lookup("CHRONO_HOME_BACKUP_SNAPSHOT_CLASS")?
                .filter(|value| !value.is_empty()),
            interval_seconds: number(lookup, "CHRONO_HOME_BACKUP_INTERVAL_SECONDS", 1800)?,
            max_in_flight: u16::try_from(number(lookup, "CHRONO_HOME_BACKUP_MAX_IN_FLIGHT", 2)?)
                .map_err(|_| "CHRONO_HOME_BACKUP_MAX_IN_FLIGHT must be in 1..=65535".to_string())?,
        };
        configured.validate(enabled)?;
        Ok(configured)
    }

    fn validate(&self, enabled: bool) -> Result<(), String> {
        const CLASS: &str = "CHRONO_HOME_BACKUP_SNAPSHOT_CLASS";
        if self
            .snapshot_class
            .as_ref()
            .is_some_and(|value| !dns_subdomain(value))
        {
            return Err(format!("{CLASS} must be a DNS subdomain"));
        }
        if enabled && self.snapshot_class.is_none() {
            return Err(format!("{CLASS} is required when home backup is enabled"));
        }
        if !(1..=1800).contains(&self.interval_seconds) {
            return Err("CHRONO_HOME_BACKUP_INTERVAL_SECONDS must be in 1..=1800".into());
        }
        if self.max_in_flight == 0 {
            return Err("CHRONO_HOME_BACKUP_MAX_IN_FLIGHT must be in 1..=65535".into());
        }
        Ok(())
    }
}

fn number(
    lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
    key: &str,
    default: u64,
) -> Result<u64, String> {
    let Some(value) = lookup(key)? else {
        return Ok(default);
    };
    let invalid = || format!("{key} must be an unsigned integer");
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(invalid());
    }
    value.parse().map_err(|_| invalid())
}

fn dns_subdomain(value: &str) -> bool {
    value.len() <= 253
        && value.split('.').all(|label| {
            (1..=63).contains(&label.len())
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
                && label
                    .as_bytes()
                    .first()
                    .is_some_and(u8::is_ascii_alphanumeric)
                && label
                    .as_bytes()
                    .last()
                    .is_some_and(u8::is_ascii_alphanumeric)
        })
}
