use crate::config::{ConfigError, CMAEG_NYXID_TURN_TRACE_ENABLED as ENABLED};

pub(crate) fn enabled() -> Result<bool, ConfigError> {
    match std::env::var(ENABLED) {
        Ok(value) => parse(Some(&value)),
        Err(std::env::VarError::NotPresent) => parse(None),
        Err(std::env::VarError::NotUnicode(_)) => parse(Some("invalid")),
    }
}

fn parse(value: Option<&str>) -> Result<bool, ConfigError> {
    match value {
        None | Some("false") => Ok(false),
        Some("true") => Ok(true),
        Some(_) => Err(ConfigError::InvalidManagement {
            detail: format!("{ENABLED} must be true or false"),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gate_defaults_off_and_requires_explicit_boolean() {
        assert!(!parse(None).unwrap());
        assert!(!parse(Some("false")).unwrap());
        assert!(parse(Some("true")).unwrap());
        for value in ["", "1", "TRUE", " true ", "private-canary"] {
            let error = parse(Some(value)).unwrap_err().to_string();
            assert!(error.contains(ENABLED));
            assert!(!error.contains("private-canary"));
        }
    }
}
