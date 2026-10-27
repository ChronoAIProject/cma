//! Validation for the separately advertised public API path.
use super::{validate_origin, ConfigError, CMAEG_PUBLIC_API_URL, CMAEG_PUBLIC_URL};
use reqwest::Url;

/// A trusted base used for generated links and routing, never taken from Host.
pub(super) fn validate_public_api_url(value: &str) -> Result<String, ConfigError> {
    let url = Url::parse(value).map_err(|_| ConfigError::InvalidManagement {
        detail: format!("{CMAEG_PUBLIC_API_URL} must be a public API base URL"),
    })?;
    validate_origin(CMAEG_PUBLIC_URL, &url.origin().ascii_serialization())?;
    let path = url.path();
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || value.ends_with('/')
        || value.len() > 2048
        || value != url.as_str().trim_end_matches('/')
        || (path != "/"
            && path
                .strip_prefix('/')
                .unwrap_or_default()
                .split('/')
                .any(|part| {
                    part.is_empty()
                        || !part
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
                }))
    {
        return Err(ConfigError::InvalidManagement {
            detail: format!("{CMAEG_PUBLIC_API_URL} must be a canonical HTTPS base with clean path segments and no credentials, query, fragment or trailing slash"),
        });
    }
    Ok(value.to_owned())
}
