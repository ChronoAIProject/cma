//! Replica participation is an explicit protocol opt-in, never an inferred pod count.
use std::{env, time::Duration};

pub const CMAEG_REPLICAS_ENABLED: &str = "CMAEG_REPLICAS_ENABLED";
pub const CMAEG_REPLICA_ADDRESS: &str = "CMAEG_REPLICA_ADDRESS";
pub const LEASE: Duration = Duration::from_secs(15);
pub const RENEW: Duration = Duration::from_secs(3);
pub const STORE_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Config {
    pub enabled: bool,
}

impl Config {
    pub fn from_env() -> Result<Self, &'static str> {
        match env::var(CMAEG_REPLICAS_ENABLED) {
            Ok(value) => Self::parse(Some(&value)),
            Err(env::VarError::NotPresent) => Self::parse(None),
            Err(_) => Err("CMAEG_REPLICAS_ENABLED must be true or false"),
        }
    }

    fn parse(value: Option<&str>) -> Result<Self, &'static str> {
        match value {
            None | Some("false") => Ok(Self { enabled: false }),
            Some("true") => Ok(Self { enabled: true }),
            Some(_) => Err("CMAEG_REPLICAS_ENABLED must be true or false"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn participation_is_default_off_and_strict() {
        assert!(!Config::parse(None).unwrap().enabled);
        assert!(!Config::parse(Some("false")).unwrap().enabled);
        assert!(Config::parse(Some("true")).unwrap().enabled);
        for invalid in ["", "1", "0", "TRUE", "False", " true", "true\n"] {
            assert!(Config::parse(Some(invalid)).is_err());
        }
    }
}

/// The membership origin is a configured direct pod address, never a request URL.
pub(crate) fn direct_address() -> Result<String, &'static str> {
    let value = env::var(CMAEG_REPLICA_ADDRESS)
        .map_err(|_| "CMAEG_REPLICA_ADDRESS is required in replica mode")?;
    validate_address(&value)
}

pub(crate) fn validate_address(value: &str) -> Result<String, &'static str> {
    let invalid = "CMAEG_REPLICA_ADDRESS must be a direct HTTP IP origin with an explicit port";
    let url = reqwest::Url::parse(value).map_err(|_| invalid)?;
    let host = url.host_str().ok_or(invalid)?;
    let ip = host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .parse::<std::net::IpAddr>()
        .map_err(|_| invalid)?;
    if url.scheme() != "http"
        || url.port().is_none()
        || url.port() == Some(0)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
        || ip.is_unspecified()
        || ip.is_multicast()
    {
        return Err(invalid);
    }
    Ok(url.as_str().trim_end_matches('/').to_owned())
}

#[test]
fn direct_origin_rejects_untrusted_urls() {
    for origin in [
        "http://127.0.0.1:9090",
        "http://[::1]:9090",
        "http://10.2.3.4:8080",
    ] {
        assert_eq!(validate_address(origin).unwrap(), origin);
    }
    for origin in [
        "https://10.2.3.4:8080",
        "http://service:8080",
        "http://user:pass@10.2.3.4:8080",
        "http://10.2.3.4:8080/path",
        "http://10.2.3.4:8080/?x=1",
        "http://10.2.3.4:8080/#x",
        "http://0.0.0.0:8080",
        "http://224.0.0.1:8080",
        "http://127.0.0.1:0",
        "http://127.0.0.1",
    ] {
        assert!(validate_address(origin).is_err());
    }
}
