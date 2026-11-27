use super::UsageCaptureConfig;

#[derive(Clone, Debug)]
pub(crate) struct UsageExportConfig {
    pub base_url: reqwest::Url,
    pub namespace: String,
    pub credential_ref: String,
    pub client_credentials: Option<(String, String)>,
}

impl UsageExportConfig {
    pub(super) fn from_lookup(
        lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
    ) -> Result<Option<Self>, String> {
        match lookup("CHRONO_SANDBOX_USAGE_EXPORT_ENABLED")?.as_deref() {
            None | Some("false") => return Ok(None),
            Some("true") => {}
            _ => return Err("CHRONO_SANDBOX_USAGE_EXPORT_ENABLED must be true or false".into()),
        }
        let capture = UsageCaptureConfig::from_lookup(lookup)?
            .ok_or("usage export requires usage capture")?;
        let base = super::required(lookup, "CHRONO_SANDBOX_USAGE_EXPORT_BASE_URL")?;
        let base_url = reqwest::Url::parse(&format!("{}/", base.trim_end_matches('/')))
            .map_err(|_| "usage export destination must be HTTPS")?;
        if base_url.scheme() != "https"
            || base_url.host_str().is_none()
            || !base_url.username().is_empty()
            || base_url.password().is_some()
            || base_url.query().is_some()
            || base_url.fragment().is_some()
        {
            return Err(
                "usage export destination must be HTTPS without credentials or query".into(),
            );
        }
        let id = lookup("CHRONO_SANDBOX_USAGE_EXPORT_CLIENT_ID_REF")?;
        let secret = lookup("CHRONO_SANDBOX_USAGE_EXPORT_CLIENT_SECRET_REF")?;
        let client_credentials = match (id, secret) {
            (None, None) => None,
            (Some(id), Some(secret)) => {
                reference(&id)?;
                reference(&secret)?;
                Some((id, secret))
            }
            _ => return Err("usage export requires both client credential references".into()),
        };
        let credential_ref = if client_credentials.is_none() {
            let value = super::required(lookup, "CHRONO_SANDBOX_USAGE_EXPORT_CREDENTIAL_REF")?;
            reference(&value)?;
            value
        } else {
            String::new()
        };
        Ok(Some(Self {
            base_url,
            namespace: capture.namespace,
            credential_ref,
            client_credentials,
        }))
    }

    pub(crate) fn destination(&self) -> String {
        use sha2::{Digest, Sha256};
        let mut digest = Sha256::new();
        digest.update(b"cma-usage-destination-v1\0");
        digest.update(self.base_url.as_str());
        digest.update([0]);
        digest.update(&self.namespace);
        format!("{:x}", digest.finalize())
    }
}

fn reference(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
    {
        return Err("usage export credentials require bounded environment references".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn off_reads_no_destination_identity_or_credential() {
        for flag in [None, Some("false")] {
            assert!(UsageExportConfig::from_lookup(&mut |key| {
                assert_eq!(key, "CHRONO_SANDBOX_USAGE_EXPORT_ENABLED");
                Ok(flag.map(str::to_owned))
            })
            .unwrap()
            .is_none());
        }
    }

    #[test]
    fn enabling_export_requires_capture_and_a_safe_destination() {
        assert!(UsageExportConfig::from_lookup(&mut |key| {
            Ok((key == "CHRONO_SANDBOX_USAGE_EXPORT_ENABLED").then(|| "true".into()))
        })
        .is_err());
        for base in [
            "http://usage.example",
            "https://user@usage.example",
            "https://usage.example/?token=value",
            "https://usage.example/#fragment",
        ] {
            assert!(UsageExportConfig::from_lookup(&mut |key| Ok(Some(
                match key {
                    "CHRONO_SANDBOX_USAGE_EXPORT_ENABLED"
                    | "CHRONO_SANDBOX_USAGE_CAPTURE_ENABLED" => "true",
                    "CHRONO_SANDBOX_USAGE_COMPLETENESS_ENABLED" => "false",
                    "CHRONO_SANDBOX_USAGE_PRODUCER_NAMESPACE" => "fixture",
                    "CHRONO_SANDBOX_USAGE_OWNER_ISSUER" => "https://identity.example",
                    "CHRONO_SANDBOX_USAGE_EXPORT_BASE_URL" => base,
                    _ => panic!("invalid destination reached credentials"),
                }
                .into()
            )))
            .is_err());
        }
        let first = UsageExportConfig {
            base_url: "https://usage.example/api/".parse().unwrap(),
            namespace: "fixture".into(),
            credential_ref: "FIRST_TOKEN".into(),
            client_credentials: None,
        };
        let mut rotated = first.clone();
        rotated.credential_ref = "NEXT_TOKEN".into();
        rotated.client_credentials = Some(("CLIENT_ID".into(), "CLIENT_SECRET".into()));
        assert_eq!(first.destination(), rotated.destination());
        rotated.namespace = "other".into();
        assert_ne!(first.destination(), rotated.destination());
        rotated = first.clone();
        rotated.base_url = "https://replacement.example/api/".parse().unwrap();
        assert_ne!(first.destination(), rotated.destination());
    }
}
