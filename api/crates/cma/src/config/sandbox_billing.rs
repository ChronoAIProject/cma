//! Optional NyxID prepaid Sandbox funding configuration.

#[derive(Clone, Debug)]
pub(crate) struct SandboxBillingConfig {
    pub base_url: reqwest::Url,
    /// The environment variable name containing the credential. The secret is
    /// resolved only at request time and is never retained in configuration.
    pub credential_ref: String,
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
        let credential_ref =
            super::required(lookup, "CHRONO_NYXID_SANDBOX_BILLING_CREDENTIAL_REF")?;
        if credential_ref.len() > 128
            || !credential_ref
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        {
            return Err("CHRONO_NYXID_SANDBOX_BILLING_CREDENTIAL_REF must name an environment or secret reference".into());
        }
        let producer_id = super::required(lookup, "CHRONO_NYXID_SANDBOX_BILLING_PRODUCER_ID")?;
        if !uuid::Uuid::parse_str(&producer_id)
            .is_ok_and(|id| id.get_version_num() == 4 && id.to_string() == producer_id)
        {
            return Err("CHRONO_NYXID_SANDBOX_BILLING_PRODUCER_ID is invalid".into());
        }
        Ok(Some(Self {
            base_url,
            credential_ref,
            producer_id,
        }))
    }
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
