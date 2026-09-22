use super::{ConfigError, Secret};
use std::env;

pub const CMAEG_CMA_CLIENT_ID: &str = "CMAEG_CMA_CLIENT_ID";
pub const CMAEG_CMA_CLIENT_SECRET: &str = "CMAEG_CMA_CLIENT_SECRET";

/// Optional renewable outbound CMA authentication. The separate configured
/// service bearer continues to authenticate inbound gateway rekey requests.
#[derive(Debug, Clone)]
pub struct ServiceAccountConfig {
    pub client_id: String,
    pub client_secret: Secret,
}

impl ServiceAccountConfig {
    pub(super) fn from_env() -> Result<Option<Self>, ConfigError> {
        let id = env::var_os(CMAEG_CMA_CLIENT_ID);
        let secret = env::var_os(CMAEG_CMA_CLIENT_SECRET);
        let invalid = || {
            ConfigError::InvalidManagement {
            detail: "CMA service-account client ID and secret must both be printable, nonempty environment values".into(),
        }
        };
        let (id, secret) = match (id, secret) {
            (None, None) => return Ok(None),
            (Some(id), Some(secret)) => (id, secret),
            _ => return Err(invalid()),
        };
        let client_id = id.into_string().map_err(|_| invalid())?;
        let client_secret = secret.into_string().map_err(|_| invalid())?;
        for value in [&client_id, &client_secret] {
            if value.is_empty()
                || value.len() > 4096
                || !value.bytes().all(|byte| (33..=126).contains(&byte))
            {
                return Err(invalid());
            }
        }
        Ok(Some(Self {
            client_id,
            client_secret: Secret::new(client_secret),
        }))
    }
}
