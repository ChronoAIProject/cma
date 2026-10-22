use super::{required, value_or};

/// One deployment-owned S3-backed registry, partitioned by authenticated owner.
/// The credential never enters a runtime or browser response.
#[derive(Clone)]
pub(crate) struct DocumentStorageConfig {
    pub platform_origin: String,
    pub bucket: String,
    pub client_id: String,
    pub client_secret: zeroize::Zeroizing<String>,
}
impl DocumentStorageConfig {
    pub(super) fn from_lookup(
        lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
    ) -> Result<Option<Self>, String> {
        let bucket = value_or(lookup, "CHRONO_DOCUMENT_BUCKET", "")?;
        if bucket.is_empty() {
            return Ok(None);
        }
        if bucket.len() < 3
            || bucket.len() > 63
            || !bucket
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            || bucket.starts_with('-')
            || bucket.ends_with('-')
        {
            return Err("CHRONO_DOCUMENT_BUCKET must be a valid bucket name".into());
        }
        let platform_origin = super::parse_canonical_https_origin(
            "CHRONO_PLATFORM_BASE_URL",
            &required(lookup, "CHRONO_PLATFORM_BASE_URL")?,
        )?;
        let client_id = required(lookup, "CHRONO_STORAGE_CLIENT_ID")?;
        let client_secret = required(lookup, "CHRONO_STORAGE_CLIENT_SECRET")?.into();
        Ok(Some(Self {
            platform_origin,
            bucket,
            client_id,
            client_secret,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    fn values() -> HashMap<&'static str, String> {
        HashMap::from([
            ("CHRONO_DOCUMENT_BUCKET", "cma-document-fixture".into()),
            (
                "CHRONO_PLATFORM_BASE_URL",
                "https://identity.example".into(),
            ),
            ("CHRONO_STORAGE_CLIENT_ID", "fixture-machine".into()),
            ("CHRONO_STORAGE_CLIENT_SECRET", "fixture-only".into()),
        ])
    }
    #[test]
    fn unconfigured_storage_is_explicit_and_partial_configuration_is_refused() {
        assert!(DocumentStorageConfig::from_lookup(&mut |_| Ok(None))
            .unwrap()
            .is_none());
        for missing in [
            "CHRONO_PLATFORM_BASE_URL",
            "CHRONO_STORAGE_CLIENT_ID",
            "CHRONO_STORAGE_CLIENT_SECRET",
        ] {
            let mut config = values();
            config.remove(missing);
            assert!(
                DocumentStorageConfig::from_lookup(&mut |key| Ok(config.get(key).cloned()))
                    .is_err()
            );
        }
    }
    #[test]
    fn refuses_embedded_credentials_http_and_bucket_path_injection() {
        for (key, bad) in [
            (
                "CHRONO_PLATFORM_BASE_URL",
                "https://user:password@identity.example",
            ),
            ("CHRONO_PLATFORM_BASE_URL", "http://identity.example"),
            ("CHRONO_PLATFORM_BASE_URL", "https://identity.example/path"),
            ("CHRONO_DOCUMENT_BUCKET", "bucket/other"),
        ] {
            let mut config = values();
            config.insert(key, bad.into());
            assert!(
                DocumentStorageConfig::from_lookup(&mut |key| Ok(config.get(key).cloned()))
                    .is_err()
            );
        }
        let config = values();
        let parsed = DocumentStorageConfig::from_lookup(&mut |key| Ok(config.get(key).cloned()))
            .unwrap()
            .unwrap();
        assert_eq!(parsed.bucket, "cma-document-fixture");
        assert_eq!(parsed.platform_origin, "https://identity.example");
    }
}
