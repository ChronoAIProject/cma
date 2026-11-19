use crate::providers::ProviderError;

const KEY: &str = "CMAEG_CONTEXT_DIAGNOSTICS_ENABLED";

pub(in crate::providers) fn enabled() -> Result<bool, ProviderError> {
    let value = match std::env::var(KEY) {
        Ok(value) => Some(value),
        Err(std::env::VarError::NotPresent) => None,
        Err(std::env::VarError::NotUnicode(_)) => return Err(invalid()),
    };
    parse(value.as_deref())
}

fn parse(value: Option<&str>) -> Result<bool, ProviderError> {
    match value {
        None | Some("false") => Ok(false),
        Some("true") => Ok(true),
        _ => Err(invalid()),
    }
}

fn invalid() -> ProviderError {
    ProviderError::InvalidRequest(format!("{KEY} must be true or false"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostics_require_explicit_true_without_echoing_invalid_input() {
        for (value, expected) in [(None, false), (Some("false"), false), (Some("true"), true)] {
            assert_eq!(parse(value).unwrap(), expected);
        }
        for value in ["", "TRUE", "1", "private-invalid-option"] {
            let error = parse(Some(value)).unwrap_err();
            assert_eq!(error, invalid());
        }
    }
}
