//! Configuration, read from the environment at startup.
//!
//! Every key here also appears in the deployment's ConfigMap or Secret. A key
//! read here but absent there is a divergence that will only show up in
//! production, so the two must be changed together.

use axum::http::HeaderValue;

use crate::auth::AuthMode;

/// Resolved configuration for the control plane.
#[derive(Debug, Clone)]
pub struct Config {
    /// Address to bind.
    pub bind: String,
    /// This service's public origin, used to build redirect URIs.
    pub public_url: String,
    /// Origins permitted to call this API from a browser.
    pub cors_origins: Vec<HeaderValue>,
    /// Base URL of the sandbox provider's lifecycle API.
    pub sandbox_base_url: String,
    /// API key for the sandbox provider.
    pub sandbox_api_key: String,
    /// Image each sandbox runs.
    pub sandbox_image: String,
    /// Where a sandbox forwards the agent's model calls, on the owner's behalf.
    pub upstream_base_url: String,
    /// Model the agent selects.
    pub upstream_model: String,
    /// Request shape the upstream speaks.
    pub upstream_wire_api: String,
    /// How callers are authenticated.
    pub auth_mode: AuthMode,
}

impl Config {
    /// Read configuration from the environment.
    pub fn from_env() -> Result<Self, String> {
        let bind = var_or("CHRONO_SERVER_BIND", "0.0.0.0:8080");
        let public_url = var_or("CHRONO_SERVER_PUBLIC_URL", "http://localhost:8080");
        let sandbox_base_url = var_or(
            "CHRONO_SANDBOX_BASE_URL",
            "http://opensandbox-server.opensandbox-system.svc.cluster.local",
        );
        let sandbox_image = var_or("CHRONO_SANDBOX_IMAGE", "chrono-bot-runtime:dev");
        let upstream_base_url = var_or("CHRONO_UPSTREAM_BASE_URL", "");
        let upstream_model = var_or("CHRONO_UPSTREAM_MODEL", "gpt-5.6-sol");
        let upstream_wire_api = var_or("CHRONO_UPSTREAM_WIRE_API", "responses");
        // The key is read from a file rather than an environment variable so it
        // can be delivered by a mounted secret and rotated without a restart of
        // the surrounding config, matching how neighbouring control planes do it.
        let sandbox_api_key = match std::env::var("CHRONO_SANDBOX_API_KEY_FILE")
            .ok()
            .filter(|v| !v.trim().is_empty())
        {
            Some(path) => std::fs::read_to_string(&path)
                .map_err(|e| format!("cannot read CHRONO_SANDBOX_API_KEY_FILE at {path}: {e}"))?
                .trim()
                .to_string(),
            None => require("CHRONO_SANDBOX_API_KEY")?,
        };

        let cors_origins = var_or("CHRONO_SERVER_CORS_ORIGIN", "http://localhost:5173")
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| {
                HeaderValue::from_str(s).map_err(|e| {
                    format!("CHRONO_SERVER_CORS_ORIGIN entry {s:?} is not usable: {e}")
                })
            })
            .collect::<Result<Vec<_>, _>>()?;

        // Trusted mode has to be asked for by name. Defaulting to it would mean
        // a missing provider configuration silently disables authentication.
        let auth_mode = match var_or("CHRONO_AUTH_MODE", "provider").as_str() {
            "trusted" => AuthMode::Trusted {
                subject: var_or("CHRONO_TRUSTED_SUBJECT", "local-developer"),
            },
            "provider" => {
                let base_url = require("CHRONO_NYXID_BASE_URL")?;
                let client_id = require("CHRONO_NYXID_CLIENT_ID")?;
                let client_secret = require("CHRONO_NYXID_CLIENT_SECRET")?;
                AuthMode::Provider {
                    base_url,
                    client_id,
                    client_secret,
                    public_url: public_url.clone(),
                }
            }
            other => {
                return Err(format!(
                    "CHRONO_AUTH_MODE must be \"provider\" or \"trusted\", got {other:?}"
                ))
            }
        };

        Ok(Self {
            bind,
            public_url,
            cors_origins,
            sandbox_base_url,
            sandbox_api_key,
            sandbox_image,
            upstream_base_url,
            upstream_model,
            upstream_wire_api,
            auth_mode,
        })
    }

    /// Name of the configured auth mode, for logging.
    pub fn auth_mode_name(&self) -> &'static str {
        match self.auth_mode {
            AuthMode::Provider { .. } => "provider",
            AuthMode::Trusted { .. } => "trusted",
        }
    }
}

fn var_or(key: &str, default: &str) -> String {
    std::env::var(key)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| default.to_string())
}

fn require(key: &str) -> Result<String, String> {
    std::env::var(key)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .ok_or_else(|| format!("{key} is required and was not set"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_blank_value_falls_back_to_the_default() {
        // An env var set to the empty string is a common deployment mistake and
        // must behave as absent rather than producing an empty bind address.
        assert_eq!(var_or("CHRONO_TEST_ABSENT_KEY", "fallback"), "fallback");
    }

    #[test]
    fn require_names_the_missing_key() {
        let err = require("CHRONO_TEST_DEFINITELY_ABSENT").expect_err("must fail");
        assert!(err.contains("CHRONO_TEST_DEFINITELY_ABSENT"), "got {err}");
    }

    #[test]
    fn auth_mode_names_match_the_variants() {
        let c = Config {
            bind: "0.0.0.0:8080".into(),
            public_url: "http://x".into(),
            cors_origins: vec![],
            sandbox_base_url: "http://r".into(),
            sandbox_api_key: "k".into(),
            sandbox_image: "img".into(),
            upstream_base_url: "http://u".into(),
            upstream_model: "m".into(),
            upstream_wire_api: "responses".into(),
            auth_mode: AuthMode::Trusted {
                subject: "s".into(),
            },
        };
        assert_eq!(c.auth_mode_name(), "trusted");

        let c = Config {
            auth_mode: AuthMode::Provider {
                base_url: "http://p".into(),
                client_id: "c".into(),
                client_secret: "s".into(),
                public_url: "http://x".into(),
            },
            ..c
        };
        assert_eq!(c.auth_mode_name(), "provider");
    }
}
