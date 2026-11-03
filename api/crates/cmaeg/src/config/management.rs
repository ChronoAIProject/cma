//! Secrets, origins and startup validation for the channel management plane.
use super::*;
use cmaeg_core::seal::MasterKeyRing;
use std::sync::Arc;

/// Secrets and origins used by the channel management plane.
#[derive(Debug, Clone)]
pub struct ManagementConfig {
    /// Legacy browser console authority; remains an origin.
    pub public_url: String,
    /// Canonical externally reachable API base, optionally with a path prefix.
    pub public_api_url: String,
    /// Retained exact API bases accepted during domain migration.
    pub public_api_aliases: Vec<String>,
    /// Disable only while old replicas still lack the canonical browser routes.
    pub advertise_public_api_url: bool,
    pub master_keys: Arc<MasterKeyRing>,
    pub cma_base_url: String,
    pub cma_service_token: Secret,
    pub nyxid_base_url: String,
    pub tools_nyxid_slug: String,
}

impl ManagementConfig {
    pub(super) fn from_env() -> Result<Self, ConfigError> {
        let public_url = required_origin(CMAEG_PUBLIC_URL)?;
        let public_api_url = env::var(CMAEG_PUBLIC_API_URL)
            .map(|value| validate_public_api_url(&value))
            .unwrap_or_else(|_| Ok(public_url.clone()))?;
        let public_api_aliases = super::origins::parse_public_api_aliases(
            &env::var(CMAEG_PUBLIC_API_ALIASES).unwrap_or_default(),
        )?;
        let advertisement = env::var(CMAEG_ADVERTISE_PUBLIC_API_URL);
        let advertise_public_api_url = parse_api_advertisement(match &advertisement {
            Ok(value) => Some(value.as_str()),
            Err(env::VarError::NotPresent) => None,
            Err(_) => Some("invalid"),
        })?;
        let cma_base_url = required_origin(CMAEG_CMA_BASE_URL)?;
        let nyxid_base_url = required_origin(CMAEG_NYXID_BASE_URL)?;
        let key_spec = secrets::read(CMAEG_MASTER_KEYS, CMAEG_MASTER_KEYS_FILE)?;
        let master_keys =
            MasterKeyRing::parse(&key_spec).map_err(|error| ConfigError::InvalidManagement {
                detail: format!("{CMAEG_MASTER_KEYS} is not a valid key ring: {error}"),
            })?;
        let cma_service_token =
            secrets::read(CMAEG_CMA_SERVICE_TOKEN, CMAEG_CMA_SERVICE_TOKEN_FILE)?;
        if cma_service_token.len() > 16_384
            || !cma_service_token
                .bytes()
                .all(|byte| (33..=126).contains(&byte))
        {
            return Err(ConfigError::InvalidManagement {
                detail: format!("{CMAEG_CMA_SERVICE_TOKEN} must contain a printable bearer token"),
            });
        }
        let tools_nyxid_slug =
            env::var(CMAEG_TOOLS_NYXID_SLUG).unwrap_or_else(|_| "cmaeg".to_owned());
        if !(1..=32).contains(&tools_nyxid_slug.len())
            || !tools_nyxid_slug
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        {
            return Err(ConfigError::InvalidManagement {
                detail: format!("{CMAEG_TOOLS_NYXID_SLUG} must match [a-z0-9_]{{1,32}}"),
            });
        }
        Ok(Self {
            public_url,
            public_api_url,
            public_api_aliases,
            advertise_public_api_url,
            master_keys: Arc::new(master_keys),
            cma_base_url,
            cma_service_token: Secret::new(cma_service_token),
            nyxid_base_url,
            tools_nyxid_slug,
        })
    }
}
