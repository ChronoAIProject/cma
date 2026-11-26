//! Canonical public and internal origins used by startup configuration.
use super::*;
pub(super) fn parse_api_advertisement(value: Option<&str>) -> Result<bool, ConfigError> {
    match value {
        None | Some("true") => Ok(true),
        Some("false") => Ok(false),
        _ => Err(ConfigError::InvalidManagement {
            detail: format!("{CMAEG_ADVERTISE_PUBLIC_API_URL} must be true or false"),
        }),
    }
}

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

pub(super) fn required_origin(name: &'static str) -> Result<String, ConfigError> {
    let value = env::var(name).map_err(|_| ConfigError::InvalidManagement {
        detail: format!("{name} must be set"),
    })?;
    validate_origin(name, &value)
}

pub(crate) fn validate_origin(name: &'static str, value: &str) -> Result<String, ConfigError> {
    let url = Url::parse(value).map_err(|error| ConfigError::InvalidManagement {
        detail: format!("{name} is not an origin URL: {error}"),
    })?;
    if url.host_str().is_none()
        || url.username() != ""
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || (url.path() != "" && url.path() != "/")
        || value.ends_with('/')
    {
        return Err(ConfigError::InvalidManagement {
            detail: format!(
                "{name} must be an origin without credentials, path, query, or trailing slash"
            ),
        });
    }
    if url.scheme() != "https"
        && !(url.scheme() == "http"
            && url.host().is_some_and(|host| match host {
                url::Host::Domain(domain) => {
                    domain.eq_ignore_ascii_case("localhost")
                        || (!matches!(
                            name,
                            CMAEG_PUBLIC_URL
                                | CMAEG_CONSOLE_PUBLIC_URL
                                | CMAEG_CONSOLE_NYXID_PUBLIC_URL
                        ) && domain.ends_with(".svc"))
                }
                url::Host::Ipv4(address) => address.is_loopback(),
                url::Host::Ipv6(address) => address.is_loopback(),
            }))
    {
        return Err(ConfigError::InvalidManagement {
            detail: format!("{name} must use https (or loopback http for local tests)"),
        });
    }
    Ok(url.as_str().trim_end_matches('/').to_owned())
}

pub(super) fn parse_public_api_aliases(value: &str) -> Result<Vec<String>, ConfigError> {
    if value.is_empty() {
        return Ok(vec![]);
    }
    let values = value.split(',').collect::<Vec<_>>();
    if values.len() > 8 {
        return Err(ConfigError::InvalidManagement {
            detail: format!("{CMAEG_PUBLIC_API_ALIASES} allows at most eight exact bases"),
        });
    }
    values.into_iter().map(validate_public_api_url).collect()
}

/// Earlier CMA origins whose record URLs are sealed into retained channel IDs.
pub(super) fn parse_cma_record_origin_aliases(value: &str) -> Result<Vec<String>, ConfigError> {
    if value.is_empty() {
        return Ok(vec![]);
    }
    let values = value.split(',').collect::<Vec<_>>();
    if values.len() > 8 {
        return Err(ConfigError::InvalidManagement {
            detail: format!("{CMAEG_CMA_RECORD_ORIGIN_ALIASES} allows at most eight origins"),
        });
    }
    values
        .into_iter()
        .map(|value| validate_origin(CMAEG_CMA_RECORD_ORIGIN_ALIASES, value))
        .collect()
}
