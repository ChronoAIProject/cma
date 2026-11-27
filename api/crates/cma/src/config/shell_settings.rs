//! HTTP, database, and authentication settings shared by both startup modes.
use super::*;
use cma_service_auth::mongodb_authority::MongoAuthorityMode;

pub(crate) struct ServerSettings {
    pub bind: SocketAddr,
    pub metrics_bind: SocketAddr,
    pub public_url: String,
    pub cors_origins: Vec<HeaderValue>,
}

pub(super) fn server(
    mut lookup: impl FnMut(&str) -> Result<Option<String>, String>,
) -> Result<ServerSettings, String> {
    let bind = value_or(&mut lookup, "CHRONO_SERVER_BIND", DEFAULT_BIND)?
        .parse::<SocketAddr>()
        .map_err(|error| format!("CHRONO_SERVER_BIND is not a socket address: {error}"))?;
    let metrics_bind = value_or(&mut lookup, "CHRONO_METRICS_BIND", "0.0.0.0:9091")?
        .parse::<SocketAddr>()
        .map_err(|error| format!("CHRONO_METRICS_BIND is not a socket address: {error}"))?;
    if metrics_bind == bind && bind.port() != 0 {
        return Err("CHRONO_METRICS_BIND must differ from CHRONO_SERVER_BIND".into());
    }
    let public_url = value_or(&mut lookup, "CHRONO_SERVER_PUBLIC_URL", DEFAULT_PUBLIC_URL)?;
    validate_origin("CHRONO_SERVER_PUBLIC_URL", &public_url)?;
    let cors_origins = value_or(
        &mut lookup,
        "CHRONO_SERVER_CORS_ORIGIN",
        DEFAULT_CORS_ORIGIN,
    )?
    .split(',')
    .map(str::trim)
    .filter(|value| !value.is_empty())
    .map(|value| {
        validate_origin("CHRONO_SERVER_CORS_ORIGIN", value)?;
        HeaderValue::from_str(value).map_err(|error| {
            format!("CHRONO_SERVER_CORS_ORIGIN contains an invalid header value: {error}")
        })
    })
    .collect::<Result<Vec<_>, _>>()?;
    if cors_origins.is_empty() {
        return Err("CHRONO_SERVER_CORS_ORIGIN must contain at least one origin".into());
    }

    Ok(ServerSettings {
        bind,
        metrics_bind,
        public_url,
        cors_origins,
    })
}

pub(crate) struct DatabaseSettings {
    pub mongodb_uri: String,
    pub mongodb_database: String,
    pub mongodb_authority: MongoAuthorityMode,
    pub deployment_id: String,
}

pub(super) fn database(
    mut lookup: impl FnMut(&str) -> Result<Option<String>, String>,
) -> Result<DatabaseSettings, String> {
    let mongodb_uri = required(&mut lookup, "CHRONO_MONGODB_URI")?;
    validate_mongodb_uri(&mongodb_uri)?;
    let mongodb_database = value_or(&mut lookup, "CHRONO_MONGODB_DATABASE", DEFAULT_DATABASE)?;
    if mongodb_database.is_empty()
        || mongodb_database.len() > 63
        || mongodb_database
            .bytes()
            .any(|byte| byte.is_ascii_whitespace() || b"/\\.\"$".contains(&byte))
    {
        return Err("CHRONO_MONGODB_DATABASE is not a valid MongoDB database name".into());
    }

    let deployment_id = runtime::deployment_id_from_lookup(&mut lookup)?;

    let mongodb_authority =
        MongoAuthorityMode::parse(lookup("CHRONO_MONGODB_AUTHORITY_MODE")?.as_deref())
            .map_err(str::to_owned)?;
    Ok(DatabaseSettings {
        mongodb_uri,
        mongodb_database,
        mongodb_authority,
        deployment_id,
    })
}

pub(super) fn activity_retention(
    mut lookup: impl FnMut(&str) -> Result<Option<String>, String>,
) -> Result<Duration, String> {
    let activity_retention_days = parse_u64(
        "CHRONO_ACTIVITY_RETENTION_DAYS",
        &value_or(
            &mut lookup,
            "CHRONO_ACTIVITY_RETENTION_DAYS",
            &DEFAULT_ACTIVITY_RETENTION_DAYS.to_string(),
        )?,
        1,
        3_650,
    )?;
    let activity_retention = Duration::from_secs(
        activity_retention_days
            .checked_mul(86_400)
            .expect("the validated day count fits in seconds"),
    );

    Ok(activity_retention)
}

pub(crate) struct AuthenticationSettings {
    pub auth_mode: AuthMode,
    pub nyxid_public_url: Option<String>,
}

pub(crate) struct BackendFreeConfig {
    pub server: ServerSettings,
    pub database: DatabaseSettings,
    pub authentication: AuthenticationSettings,
    pub encryption_key: AuthEncryptionKey,
    pub activity_retention: Duration,
    pub startup_schema: StartupSchemaMode,
    pub rate_limits: RateLimitConfig,
    pub trusted_proxies: TrustedProxyConfig,
    pub nyxid_service_id: Option<String>,
    pub nyxid_oauth_api_resource: Option<String>,
    pub nyxid_proxy_issuer: Option<String>,
    pub nyxid_proxy_jwks_url: Option<String>,
    pub nyxid_proxy_audience: Option<String>,
    pub nyxid_resource_request: bool,
    pub nyxid_proxy: bool,
    pub silent_sign_in: bool,
    pub browser_session_join: bool,
    pub model_budget_writes: bool,
    pub model_request_budget_writes: bool,
    pub platform_spend_writes: bool,
    pub platform_counter_shards: bool,
    pub visitor_two_sandbox_defaults: bool,
    pub admission_ceilings_writes: bool,
}

impl BackendFreeConfig {
    pub(super) fn from_lookup(
        mut lookup: impl FnMut(&str) -> Result<Option<String>, String>,
    ) -> Result<Self, String> {
        use super::features::flag;

        let server = server(&mut lookup)?;
        let authentication = authentication(&mut lookup, &server.public_url)?;
        Ok(Self {
            server,
            authentication,
            database: database(&mut lookup)?,
            encryption_key: AuthEncryptionKey::from_base64(&required(
                &mut lookup,
                "CHRONO_AUTH_ENCRYPTION_KEY",
            )?)?,
            rate_limits: RateLimitConfig::from_lookup(&mut lookup)?,
            trusted_proxies: TrustedProxyConfig::from_lookup(&mut lookup)?,
            activity_retention: activity_retention(&mut lookup)?,
            startup_schema: StartupSchemaMode::from_lookup(&mut lookup)?,
            nyxid_service_id: lookup("CHRONO_NYXID_SERVICE_ID")?.filter(|id| !id.trim().is_empty()),
            nyxid_oauth_api_resource: oauth_resource::parse(lookup(
                "CHRONO_NYXID_OAUTH_API_RESOURCE",
            )?)?,
            nyxid_proxy_issuer: lookup("CHRONO_NYXID_PROXY_ISSUER")?,
            nyxid_proxy_jwks_url: lookup("CHRONO_NYXID_PROXY_JWKS_URL")?,
            nyxid_proxy_audience: lookup("CHRONO_NYXID_PROXY_AUDIENCE")?,
            nyxid_resource_request: flag(&mut lookup, "CHRONO_NYXID_RESOURCE_REQUEST_ENABLED")?,
            nyxid_proxy: flag(&mut lookup, "CHRONO_NYXID_PROXY_ENABLED")?,
            silent_sign_in: flag(&mut lookup, "CHRONO_SILENT_SIGN_IN_ENABLED")?,
            browser_session_join: browser_session::from_lookup(&mut lookup)?,
            model_budget_writes: flag(&mut lookup, "CHRONO_MODEL_BUDGET_WRITES_ENABLED")?,
            model_request_budget_writes: flag(
                &mut lookup,
                "CHRONO_MODEL_REQUEST_BUDGET_WRITES_ENABLED",
            )?,
            platform_spend_writes: flag(&mut lookup, "CHRONO_PLATFORM_SPEND_WRITES_ENABLED")?,
            platform_counter_shards: flag(&mut lookup, "CHRONO_PLATFORM_COUNTER_SHARDS_ENABLED")?,
            visitor_two_sandbox_defaults: flag(
                &mut lookup,
                "CHRONO_VISITOR_TWO_SANDBOX_DEFAULTS_ENABLED",
            )?,
            admission_ceilings_writes: flag(
                &mut lookup,
                "CHRONO_ADMISSION_CEILINGS_WRITES_ENABLED",
            )?,
        })
    }
}

pub(super) fn authentication(
    mut lookup: impl FnMut(&str) -> Result<Option<String>, String>,
    public_url: &str,
) -> Result<AuthenticationSettings, String> {
    let (auth_mode, nyxid_public_url) =
        match value_or(&mut lookup, "CHRONO_AUTH_MODE", "provider")?.as_str() {
            "trusted" => (
                AuthMode::Trusted {
                    subject: value_or(&mut lookup, "CHRONO_TRUSTED_SUBJECT", "local-developer")?,
                },
                None,
            ),
            "provider" => (
                AuthMode::Provider {
                    base_url: required(&mut lookup, "CHRONO_NYXID_BASE_URL")?,
                    client_id: required(&mut lookup, "CHRONO_NYXID_CLIENT_ID")?,
                    client_secret: required(&mut lookup, "CHRONO_NYXID_CLIENT_SECRET")?,
                    public_url: public_url.to_owned(),
                },
                Some(parse_canonical_https_origin(
                    "CHRONO_NYXID_PUBLIC_URL",
                    &required(&mut lookup, "CHRONO_NYXID_PUBLIC_URL")?,
                )?),
            ),
            value => {
                return Err(format!(
                    "CHRONO_AUTH_MODE must be \"provider\" or \"trusted\", got {value:?}"
                ))
            }
        };
    if let AuthMode::Provider { base_url, .. } = &auth_mode {
        validate_origin("CHRONO_NYXID_BASE_URL", base_url)?;
    }

    Ok(AuthenticationSettings {
        auth_mode,
        nyxid_public_url,
    })
}

#[cfg(test)]
mod tests {
    use base64::{engine::general_purpose::STANDARD, Engine as _};

    use super::*;
    use crate::config::runtime::{Provider, RuntimeMode};

    #[test]
    fn provider_selection_only_allows_explicit_backend_free_development_and_staging() {
        for (provider, expected) in [
            (None, Provider::OpenSandbox),
            (Some("opensandbox"), Provider::OpenSandbox),
            (Some("agent-sandbox"), Provider::AgentSandbox),
        ] {
            let selected = RuntimeMode::from_lookup(&mut |key| {
                assert_eq!(key, "CHRONO_SANDBOX_PROVIDER");
                Ok(provider.map(str::to_owned))
            })
            .unwrap();
            assert_eq!(selected, RuntimeMode::Execution(expected));
        }
        for environment in [
            None,
            Some("production"),
            Some(""),
            Some("test"),
            Some("Staging"),
        ] {
            assert!(select_disabled(environment).is_err());
        }
        for environment in ["development", "staging"] {
            assert_eq!(
                select_disabled(Some(environment)).unwrap(),
                RuntimeMode::BackendFree
            );
        }
    }

    #[test]
    fn backend_free_startup_uses_no_backend_configuration_and_validates_account_dependencies() {
        for environment in ["development", "staging"] {
            let config = StartupConfig::from_lookup(|key| {
                assert!(
                    !key.starts_with("CHRONO_SANDBOX_") || key == "CHRONO_SANDBOX_PROVIDER",
                    "{key}"
                );
                assert!(!key.starts_with("CHRONO_UPSTREAM_"), "{key}");
                assert!(!key.starts_with("CHRONO_NATIVE_"), "{key}");
                assert!(!key.starts_with("CHRONO_AGENT_SANDBOX_"), "{key}");
                assert!(
                    !matches!(
                        key,
                        "CHRONO_RETAIN_OPENSANDBOX" | "CHRONO_PLATFORM_BASE_URL"
                    ),
                    "{key}"
                );
                Ok(match key {
                    "CHRONO_SANDBOX_PROVIDER" => Some("none".into()),
                    "CHRONO_ENVIRONMENT" => Some(environment.into()),
                    "CHRONO_AUTH_MODE" => Some("trusted".into()),
                    "CHRONO_DEPLOYMENT_ID" => Some("fixture".into()),
                    "CHRONO_AUTH_ENCRYPTION_KEY" => Some(STANDARD.encode([7; 32])),
                    "CHRONO_MONGODB_URI" => Some("mongodb://127.0.0.1:27017".into()),
                    _ => None,
                })
            })
            .unwrap();
            let StartupConfig::BackendFree(config) = config else {
                panic!("explicit none needs its own configuration");
            };
            assert_eq!(config.server.public_url, DEFAULT_PUBLIC_URL);
            assert_eq!(config.database.mongodb_database, DEFAULT_DATABASE);
            assert!(matches!(
                config.authentication.auth_mode,
                AuthMode::Trusted { .. }
            ));
        }

        let missing_auth = StartupConfig::from_lookup(|key| {
            Ok(match key {
                "CHRONO_SANDBOX_PROVIDER" => Some("none".into()),
                "CHRONO_ENVIRONMENT" => Some("staging".into()),
                _ => None,
            })
        });
        assert_eq!(
            missing_auth.err().as_deref(),
            Some("CHRONO_NYXID_BASE_URL is required")
        );
    }

    fn select_disabled(environment: Option<&str>) -> Result<RuntimeMode, String> {
        RuntimeMode::from_lookup(&mut |key| {
            Ok(match key {
                "CHRONO_SANDBOX_PROVIDER" => Some("none".into()),
                "CHRONO_ENVIRONMENT" => environment.map(str::to_owned),
                _ => panic!("unexpected lookup {key}"),
            })
        })
    }
}
