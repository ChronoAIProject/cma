//! CMAEG management transport and its externally reachable callback origin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GatewayConfig {
    pub base_url: String,
    pub public_url: String,
    pub public_api_url: String,
    pub public_api_aliases: Vec<String>,
    /// Writers stay off until both services and rollback readers are compatible.
    pub binding_preparation_enabled: bool,
}
impl GatewayConfig {
    pub(super) fn from_lookup(
        lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
    ) -> Result<Option<Self>, String> {
        let binding_preparation_enabled = match lookup("CHRONO_CMAEG_BINDING_PREPARATION_ENABLED")?
            .as_deref()
        {
            None | Some("false") => false,
            Some("true") => true,
            _ => {
                return Err("CHRONO_CMAEG_BINDING_PREPARATION_ENABLED must be true or false".into())
            }
        };
        let base = lookup("CHRONO_CMAEG_BASE_URL")?.filter(|s| !s.is_empty());
        let public = lookup("CHRONO_CMAEG_PUBLIC_URL")?.filter(|s| !s.is_empty());
        let public_api = lookup("CHRONO_CMAEG_PUBLIC_API_URL")?.filter(|s| !s.is_empty());
        let public_api_aliases = parse_public_api_aliases(
            &lookup("CHRONO_CMAEG_PUBLIC_API_ALIASES")?.unwrap_or_default(),
        )?;
        match (base, public) {
            (None, None)
                if public_api.is_none()
                    && public_api_aliases.is_empty()
                    && !binding_preparation_enabled =>
            {
                Ok(None)
            }
            (Some(base_url), Some(public_url)) => {
                for (key, value) in [
                    ("CHRONO_CMAEG_BASE_URL", &base_url),
                    ("CHRONO_CMAEG_PUBLIC_URL", &public_url),
                ] {
                    super::validate_origin(key, value)?;
                }
                let public_api_url = match public_api {
                    Some(value) => validate_public_api_url(&value)?,
                    None => public_url.trim_end_matches('/').to_owned(),
                };
                Ok(Some(Self {
                    base_url,
                    public_url,
                    public_api_url,
                    public_api_aliases,
                    binding_preparation_enabled,
                }))
            }
            _ => Err(
                "CHRONO_CMAEG_BASE_URL and CHRONO_CMAEG_PUBLIC_URL must be configured together"
                    .into(),
            ),
        }
    }
}

fn parse_public_api_aliases(value: &str) -> Result<Vec<String>, String> {
    if value.is_empty() {
        return Ok(vec![]);
    }
    let values = value.split(',').collect::<Vec<_>>();
    if values.len() > 8 {
        return Err("CHRONO_CMAEG_PUBLIC_API_ALIASES allows at most eight exact bases".into());
    }
    values.into_iter().map(validate_public_api_url).collect()
}

fn validate_public_api_url(value: &str) -> Result<String, String> {
    let key = "CHRONO_CMAEG_PUBLIC_API_URL";
    let url =
        reqwest::Url::parse(value).map_err(|_| format!("{key} must be a public API base URL"))?;
    let local = url.host().is_some_and(|host| match host {
        url::Host::Domain(domain) => domain == "localhost",
        url::Host::Ipv4(ip) => ip.is_loopback(),
        url::Host::Ipv6(ip) => ip.is_loopback(),
    });
    if !(url.scheme() == "https" || url.scheme() == "http" && local)
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || value.ends_with('/')
        || value.len() > 2048
        || value != url.as_str().trim_end_matches('/')
        || (url.path() != "/"
            && url
                .path()
                .strip_prefix('/')
                .unwrap_or_default()
                .split('/')
                .any(|part| {
                    part.is_empty()
                        || !part
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
                }))
    {
        return Err(format!("{key} must be a canonical HTTPS base with clean path segments and no credentials, query, fragment or trailing slash"));
    }
    Ok(value.to_owned())
}

#[cfg(test)]
mod public_api_tests {
    use super::*;
    #[test]
    fn binding_preparation_writers_are_explicit_and_default_off() {
        for (value, expected) in [(None, false), (Some("false"), false), (Some("true"), true)] {
            let config = GatewayConfig::from_lookup(&mut |key| {
                Ok(match key {
                    "CHRONO_CMAEG_BASE_URL" | "CHRONO_CMAEG_PUBLIC_URL" => {
                        Some("https://gateway.example".into())
                    }
                    "CHRONO_CMAEG_BINDING_PREPARATION_ENABLED" => value.map(str::to_owned),
                    _ => None,
                })
            })
            .unwrap()
            .unwrap();
            assert_eq!(config.binding_preparation_enabled, expected);
        }
        assert!(GatewayConfig::from_lookup(&mut |key| Ok((key
            == "CHRONO_CMAEG_BINDING_PREPARATION_ENABLED")
            .then(|| "true".into())))
        .is_err());
        assert!(GatewayConfig::from_lookup(&mut |key| Ok((key
            == "CHRONO_CMAEG_BINDING_PREPARATION_ENABLED")
            .then(|| "1".into())))
        .is_err());
    }
    #[test]
    fn public_api_base_requires_a_clean_canonical_namespace() {
        for valid in [
            "https://bot.example/event-gateway",
            "https://gateway.example",
            "http://localhost:8090/event-gateway",
        ] {
            assert_eq!(validate_public_api_url(valid).unwrap(), valid);
        }
        for invalid in [
            "https://bot.example/event-gateway/",
            "https://bot.example/a/../event-gateway",
            "https://bot.example//event-gateway",
            "https://bot.example/%2e%2e/event-gateway",
            "https://user@bot.example/event-gateway",
            "https://bot.example/event-gateway?q=1",
            "https://bot.example/event-gateway#fragment",
            "http://bot.example/event-gateway",
        ] {
            assert!(validate_public_api_url(invalid).is_err(), "{invalid}");
        }
    }
}

#[cfg(test)]
mod alias_tests {
    use super::*;

    #[test]
    fn domain_aliases_are_bounded_exact_clean_bases() {
        assert!(parse_public_api_aliases("").unwrap().is_empty());
        assert_eq!(
            parse_public_api_aliases(
                "https://old.example/event-gateway,https://new.example/event-gateway"
            )
            .unwrap()
            .len(),
            2
        );
        for invalid in [
            "https://old.example/event-gateway,",
            "https://old.example/event-gateway,*",
            "https://new.example/path?query=1",
            "http://public.example",
        ] {
            assert!(parse_public_api_aliases(invalid).is_err());
        }
        assert!(parse_public_api_aliases(&["https://old.example"; 9].join(",")).is_err());
    }
}
