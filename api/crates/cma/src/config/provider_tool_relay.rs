//! Explicit provider bindings whose native tool transport may use the CMA relay.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProviderToolRelay {
    pub bindings: Vec<String>,
    pub from_endpoint: String,
    pub upstream: String,
}

impl ProviderToolRelay {
    pub(crate) fn applies(&self, binding: &str, endpoint: &str) -> bool {
        self.from_endpoint == endpoint && self.bindings.iter().any(|id| id == binding)
    }
}

pub(super) fn parse(
    lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
    public_origin: &str,
) -> Result<Option<ProviderToolRelay>, String> {
    let invalid = || {
        "CHRONO_PROVIDER_TOOL_RELAY requires exact binding IDs and one canonical gateway migration"
            .to_owned()
    };
    let Some(raw) = lookup("CHRONO_PROVIDER_TOOL_RELAY")?.filter(|raw| !raw.is_empty()) else {
        return Ok(None);
    };
    let mut config: ProviderToolRelay = serde_json::from_str(&raw).map_err(|_| invalid())?;
    let origin = super::parse_canonical_https_origin("CHRONO_PROVIDER_TOOL_RELAY", public_origin)?;
    let from = url::Url::parse(&config.from_endpoint).map_err(|_| invalid())?;
    if from.scheme() != "https"
        || from.host_str().is_none()
        || !from.username().is_empty()
        || from.password().is_some()
        || from.query().is_some()
        || from.fragment().is_some()
        || from.as_str() != config.from_endpoint
        || config.upstream != format!("{}/event-gateway/mcp", origin.trim_end_matches('/'))
        || config.from_endpoint == config.upstream
        || config.bindings.is_empty()
        || config.bindings.len() > 32
        || config.bindings.iter().any(|id| {
            id.strip_prefix("bnd_").is_none_or(|suffix| {
                suffix.len() != 32
                    || !suffix
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            })
        })
    {
        return Err(invalid());
    }
    config.bindings.sort();
    config.bindings.dedup();
    Ok(Some(config))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn relay_is_explicit_and_cannot_forward_to_an_arbitrary_destination() {
        assert!(parse(&mut |_| Ok(None), "https://cma.example")
            .unwrap()
            .is_none());
        let value = serde_json::json!({"bindings":["bnd_0123456789abcdef0123456789abcdef"],"from_endpoint":"https://old.example/mcp","upstream":"https://cma.example/event-gateway/mcp"});
        assert!(
            parse(&mut |_| Ok(Some(value.to_string())), "https://cma.example")
                .unwrap()
                .is_some()
        );
        for upstream in [
            "https://evil.example/mcp",
            "https://cma.example/api/v1/admin",
            "http://cma.example/event-gateway/mcp",
        ] {
            let mut invalid = value.clone();
            invalid["upstream"] = upstream.into();
            assert!(parse(
                &mut |_| Ok(Some(invalid.to_string())),
                "https://cma.example"
            )
            .is_err());
        }
        for binding in ["*", "bnd_short", "bnd_0123456789abcdef0123456789abcdeg"] {
            let mut invalid = value.clone();
            invalid["bindings"] = serde_json::json!([binding]);
            assert!(parse(
                &mut |_| Ok(Some(invalid.to_string())),
                "https://cma.example"
            )
            .is_err());
        }
    }
}
