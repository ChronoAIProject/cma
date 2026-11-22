//! History has an independent rolling retention policy; execution TTLs do not change.
use crate::activity::history::Error;

#[derive(Clone, Debug)]
pub struct Config {
    pub enabled: bool,
    pub capture: bool,
    pub retention_days: u16,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: false,
            capture: false,
            retention_days: 7,
        }
    }
}
impl Config {
    pub fn from_env() -> Result<Self, Error> {
        Self::read(|name| std::env::var(name).ok())
    }
    fn read(get: impl Fn(&str) -> Option<String>) -> Result<Self, Error> {
        let flag = |name| match get(name).as_deref() {
            None | Some("false") => Ok(false),
            Some("true") => Ok(true),
            _ => Err(Error::InvalidConfiguration),
        };
        let value = Self {
            enabled: flag("CMAEG_ACTIVITY_HISTORY_ENABLED")?,
            capture: flag("CMAEG_ACTIVITY_HISTORY_CAPTURE")?,
            retention_days: get("CMAEG_ACTIVITY_HISTORY_RETENTION_DAYS")
                .map(|v| v.parse().map_err(|_| Error::InvalidConfiguration))
                .transpose()?
                .unwrap_or(7),
        };
        if !(1..=90).contains(&value.retention_days) || (value.capture && !value.enabled) {
            return Err(Error::InvalidConfiguration);
        }
        Ok(value)
    }
    pub(crate) fn retention_ms(&self) -> i64 {
        i64::from(self.retention_days) * 86_400_000
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn default_is_seven_days_with_reader_and_capture_off() {
        let config = Config::read(|_| None).unwrap();
        assert_eq!(config.retention_days, 7);
        assert!(!config.enabled && !config.capture);
    }
    #[test]
    fn fourteen_days_is_configurable_without_changing_other_retention() {
        let config =
            Config::read(|k| (k == "CMAEG_ACTIVITY_HISTORY_RETENTION_DAYS").then(|| "14".into()))
                .unwrap();
        assert_eq!(config.retention_ms(), 14 * 86_400_000);
    }
    #[test]
    fn invalid_values_and_capture_without_readers_fail_closed() {
        for invalid in ["0", "91", "-1", "seven"] {
            assert!(Config::read(
                |k| (k == "CMAEG_ACTIVITY_HISTORY_RETENTION_DAYS").then(|| invalid.into())
            )
            .is_err());
        }
        assert!(
            Config::read(|k| (k == "CMAEG_ACTIVITY_HISTORY_CAPTURE").then(|| "true".into()))
                .is_err()
        );
        assert!(
            Config::read(|k| (k == "CMAEG_ACTIVITY_HISTORY_ENABLED").then(|| "yes".into()))
                .is_err()
        );
    }
}
