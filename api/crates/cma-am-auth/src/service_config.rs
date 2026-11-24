//! Explicit provider identity pins, unused while authorization recovery is off.
use url::Url;

#[derive(Clone, Debug)]
pub struct ServiceAuthorizationConfig {
    pub issuer: Url,
    pub api_origin: Url,
}

impl ServiceAuthorizationConfig {
    pub fn from_lookup(
        lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
        enabled: bool,
    ) -> Result<Option<Self>, String> {
        if !enabled {
            return Ok(None);
        }
        let mut origin = |key| -> Result<Url, String> {
            let value = lookup(key)?.ok_or_else(|| {
                format!("{key} is required when service authorization is enabled")
            })?;
            let url = Url::parse(&value).map_err(|_| format!("{key} must be an HTTPS origin"))?;
            if url.scheme() != "https"
                || !url.username().is_empty()
                || url.password().is_some()
                || url.path() != "/"
                || url.query().is_some()
                || url.fragment().is_some()
            {
                return Err(format!("{key} must be an HTTPS origin"));
            }
            Ok(url)
        };
        Ok(Some(Self {
            issuer: origin("CHRONO_SERVICE_AUTHORIZATION_ISSUER")?,
            api_origin: origin("CHRONO_SERVICE_AUTHORIZATION_API_ORIGIN")?,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn disabled_does_not_read_or_require_provider_pins() {
        assert!(
            ServiceAuthorizationConfig::from_lookup(&mut |_| panic!("disabled lookup"), false)
                .unwrap()
                .is_none()
        );
    }
    #[test]
    fn enabled_requires_both_canonical_https_origins() {
        assert!(ServiceAuthorizationConfig::from_lookup(&mut |_| Ok(None), true).is_err());
        for value in [
            "http://identity.example",
            "https://user@identity.example",
            "https://identity.example/path",
            "https://identity.example?query",
        ] {
            assert!(
                ServiceAuthorizationConfig::from_lookup(&mut |_| Ok(Some(value.into())), true)
                    .is_err()
            );
        }
        let config = ServiceAuthorizationConfig::from_lookup(
            &mut |key| {
                Ok(Some(if key.ends_with("ISSUER") {
                    "https://issuer.example".into()
                } else {
                    "https://api.example".into()
                }))
            },
            true,
        )
        .unwrap()
        .unwrap();
        assert_ne!(config.issuer, config.api_origin);
    }
}
