//! Optional NyxID prepaid Sandbox funding configuration.

#[derive(Clone, Debug)]
pub(crate) struct SandboxBillingConfig {
    pub base_url: reqwest::Url,
    /// The environment variable name containing the credential. The secret is
    /// resolved only at request time and is never retained in configuration.
    pub credential_ref: String,
    /// Environment reference names only; credential values never enter Config.
    pub client_credentials: Option<(String, String)>,
    pub producer_id: String,
}

impl SandboxBillingConfig {
    pub(super) fn from_lookup(
        lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
    ) -> Result<Option<Self>, String> {
        match lookup("CHRONO_NYXID_SANDBOX_BILLING_ENABLED")?.as_deref() {
            None | Some("false") => return Ok(None),
            Some("true") => {}
            _ => return Err("CHRONO_NYXID_SANDBOX_BILLING_ENABLED must be true or false".into()),
        }
        if lookup("CHRONO_USAGE_BILLING_ENABLED")?.as_deref() == Some("true") {
            return Err(
                "NyxID funding requires CHRONO_USAGE_BILLING_ENABLED=false (single emitter)".into(),
            );
        }
        let base = super::required(lookup, "CHRONO_NYXID_SANDBOX_BILLING_BASE_URL")?;
        let base_url =
            reqwest::Url::parse(&format!("{}/", base.trim_end_matches('/'))).map_err(|_| {
                "CHRONO_NYXID_SANDBOX_BILLING_BASE_URL must be an HTTPS URL".to_string()
            })?;
        if base_url.scheme() != "https"
            || base_url.host_str().is_none()
            || !base_url.username().is_empty()
            || base_url.password().is_some()
            || base_url.query().is_some()
            || base_url.fragment().is_some()
        {
            return Err("CHRONO_NYXID_SANDBOX_BILLING_BASE_URL must be an HTTPS URL without credentials or query parameters".into());
        }
        let id_key = "CHRONO_NYXID_SANDBOX_BILLING_CLIENT_ID_REF";
        let secret_key = "CHRONO_NYXID_SANDBOX_BILLING_CLIENT_SECRET_REF";
        let id = lookup(id_key)?.filter(|value| !value.is_empty());
        let secret = lookup(secret_key)?.filter(|value| !value.is_empty());
        let client_credentials = match (id, secret) {
            (None, None) => None,
            (Some(id), Some(secret)) => {
                for (key, reference) in [(id_key, &id), (secret_key, &secret)] {
                    validate_reference(key, reference)?;
                    let value = lookup(reference).map_err(|_| {
                        format!("{key} references an unavailable environment value")
                    })?;
                    if !value.is_some_and(|value| !value.trim().is_empty()) {
                        return Err(format!(
                            "{key} must reference a present, nonempty environment value"
                        ));
                    }
                }
                Some((id, secret))
            }
            _ => return Err(format!("{id_key} and {secret_key} must both be set")),
        };
        let credential_ref = if client_credentials.is_none() {
            let key = "CHRONO_NYXID_SANDBOX_BILLING_CREDENTIAL_REF";
            let reference = super::required(lookup, key)?;
            validate_reference(key, &reference)?;
            reference
        } else {
            String::new()
        };
        let producer_id = super::required(lookup, "CHRONO_NYXID_SANDBOX_BILLING_PRODUCER_ID")?;
        if !uuid::Uuid::parse_str(&producer_id)
            .is_ok_and(|id| id.get_version_num() == 4 && id.to_string() == producer_id)
        {
            return Err("CHRONO_NYXID_SANDBOX_BILLING_PRODUCER_ID is invalid".into());
        }
        Ok(Some(Self {
            base_url,
            credential_ref,
            client_credentials,
            producer_id,
        }))
    }
}

fn validate_reference(key: &str, value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(format!(
            "{key} must name an environment or secret reference"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup(key: &str) -> Option<String> {
        Some(
            match key {
                "CHRONO_NYXID_SANDBOX_BILLING_ENABLED" => "true",
                "CHRONO_NYXID_SANDBOX_BILLING_BASE_URL" => {
                    "https://nyx.example/api/v1/billing/sandboxes"
                }
                "CHRONO_NYXID_SANDBOX_BILLING_CREDENTIAL_REF" => "NYXID_PRODUCER_TOKEN",
                "CHRONO_NYXID_SANDBOX_BILLING_PRODUCER_ID" => {
                    "6e660487-dba4-4be9-ae06-4593b41c4a30"
                }
                _ => return None,
            }
            .into(),
        )
    }

    #[test]
    fn disabled_is_default() {
        assert!(SandboxBillingConfig::from_lookup(&mut |_| Ok(None))
            .unwrap()
            .is_none());
    }

    #[test]
    fn enabled_requires_reference_only() {
        let config = SandboxBillingConfig::from_lookup(&mut |key| Ok(lookup(key))).unwrap();
        assert_eq!(config.unwrap().credential_ref, "NYXID_PRODUCER_TOKEN");
    }
}

#[cfg(test)]
mod validation_tests {
    use super::*;
    fn valid(key: &str) -> Option<String> {
        Some(
            match key {
                "CHRONO_NYXID_SANDBOX_BILLING_ENABLED" => "true",
                "CHRONO_NYXID_SANDBOX_BILLING_BASE_URL" => {
                    "https://nyx.example/api/v1/billing/sandboxes"
                }
                "CHRONO_NYXID_SANDBOX_BILLING_CREDENTIAL_REF" => "NYXID_TOKEN",
                "CHRONO_NYXID_SANDBOX_BILLING_PRODUCER_ID" => {
                    "6e660487-dba4-4be9-ae06-4593b41c4a30"
                }
                _ => return None,
            }
            .into(),
        )
    }
    #[test]
    fn validates_configuration_without_reading_secrets_or_allowing_two_emitters() {
        for (key, value) in [
            ("CHRONO_NYXID_SANDBOX_BILLING_ENABLED", "yes"),
            ("CHRONO_USAGE_BILLING_ENABLED", "true"),
            (
                "CHRONO_NYXID_SANDBOX_BILLING_BASE_URL",
                "http://example.com",
            ),
            (
                "CHRONO_NYXID_SANDBOX_BILLING_BASE_URL",
                "https://user:secret@example.com",
            ),
            (
                "CHRONO_NYXID_SANDBOX_BILLING_BASE_URL",
                "https://example.com?token=secret",
            ),
            (
                "CHRONO_NYXID_SANDBOX_BILLING_BASE_URL",
                "https://example.com#secret",
            ),
            ("CHRONO_NYXID_SANDBOX_BILLING_BASE_URL", "invalid"),
            (
                "CHRONO_NYXID_SANDBOX_BILLING_CREDENTIAL_REF",
                "secret/value",
            ),
            ("CHRONO_NYXID_SANDBOX_BILLING_PRODUCER_ID", "not-uuid"),
        ] {
            assert!(
                SandboxBillingConfig::from_lookup(&mut |name| Ok(if name == key {
                    Some(value.into())
                } else {
                    valid(name)
                }))
                .is_err(),
                "{key}"
            );
        }
        for key in [
            "CHRONO_NYXID_SANDBOX_BILLING_BASE_URL",
            "CHRONO_NYXID_SANDBOX_BILLING_CREDENTIAL_REF",
            "CHRONO_NYXID_SANDBOX_BILLING_PRODUCER_ID",
        ] {
            assert!(
                SandboxBillingConfig::from_lookup(&mut |name| Ok(if name == key {
                    None
                } else {
                    valid(name)
                }))
                .is_err()
            );
        }
        assert!(SandboxBillingConfig::from_lookup(&mut |key| {
            assert_eq!(key, "CHRONO_NYXID_SANDBOX_BILLING_ENABLED");
            Ok(Some("false".into()))
        })
        .unwrap()
        .is_none());
    }
}

#[cfg(test)]
mod credentials_tests {
    use super::*;
    const ID: &str = "CHRONO_NYXID_SANDBOX_BILLING_CLIENT_ID_REF";
    const SECRET: &str = "CHRONO_NYXID_SANDBOX_BILLING_CLIENT_SECRET_REF";
    fn configured(key: &str) -> Option<String> {
        Some(
            match key {
                "CHRONO_NYXID_SANDBOX_BILLING_ENABLED" => "true",
                "CHRONO_NYXID_SANDBOX_BILLING_BASE_URL" => {
                    "https://nyx.example/api/v1/billing/sandboxes/"
                }
                "CHRONO_NYXID_SANDBOX_BILLING_PRODUCER_ID" => {
                    "6e660487-dba4-4be9-ae06-4593b41c4a30"
                }
                ID => "NYXID_CMA_BILLING_PRODUCER_CLIENT_ID",
                SECRET => "NYXID_CMA_BILLING_PRODUCER_CLIENT_SECRET",
                "NYXID_CMA_BILLING_PRODUCER_CLIENT_ID" => "test id",
                "NYXID_CMA_BILLING_PRODUCER_CLIENT_SECRET" => "test secret",
                _ => return None,
            }
            .into(),
        )
    }
    #[test]
    fn paired_refs_select_oauth_without_requiring_static_credentials() {
        let config = SandboxBillingConfig::from_lookup(&mut |key| Ok(configured(key)))
            .unwrap()
            .unwrap();
        assert!(config.credential_ref.is_empty());
        assert_eq!(
            config.client_credentials,
            Some((
                "NYXID_CMA_BILLING_PRODUCER_CLIENT_ID".into(),
                "NYXID_CMA_BILLING_PRODUCER_CLIENT_SECRET".into()
            ))
        );
        assert!(!format!("{config:?}").contains("test secret"));
    }
    #[test]
    fn partial_empty_invalid_and_missing_environment_values_are_rejected() {
        for (changed, replacement) in [
            (ID, None),
            (SECRET, None),
            (ID, Some("")),
            (SECRET, Some("")),
            (ID, Some("invalid ref")),
            (SECRET, Some(" ")),
            ("NYXID_CMA_BILLING_PRODUCER_CLIENT_ID", None),
            ("NYXID_CMA_BILLING_PRODUCER_CLIENT_SECRET", None),
            ("NYXID_CMA_BILLING_PRODUCER_CLIENT_SECRET", Some("  ")),
        ] {
            let error = SandboxBillingConfig::from_lookup(&mut |key| {
                Ok(if key == changed {
                    replacement.map(str::to_owned)
                } else {
                    configured(key)
                })
            })
            .unwrap_err();
            assert!(error.contains("CLIENT_ID_REF") || error.contains("CLIENT_SECRET_REF"));
            assert!(!error.contains("test secret"));
        }
    }
    #[test]
    fn empty_new_refs_preserve_static_mode_and_do_not_read_its_secret() {
        let config = SandboxBillingConfig::from_lookup(&mut |key| {
            Ok(match key {
                ID | SECRET => Some(String::new()),
                "CHRONO_NYXID_SANDBOX_BILLING_CREDENTIAL_REF" => Some("STATIC_TOKEN".into()),
                "STATIC_TOKEN" => panic!("static token remains resolved at request time"),
                _ => configured(key),
            })
        })
        .unwrap()
        .unwrap();
        assert!(config.client_credentials.is_none());
        assert_eq!(config.credential_ref, "STATIC_TOKEN");
    }
}
