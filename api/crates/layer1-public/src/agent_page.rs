//! Browser links to actual CMA Agents, derived only from deployment configuration.

use url::Url;

#[derive(Debug, Clone)]
pub struct AgentPageOrigin(Url);

#[derive(Debug, thiserror::Error)]
#[error(
    "the Agent page origin must be an HTTP(S) origin without credentials, path, query or fragment"
)]
pub struct InvalidAgentPageOrigin;

impl AgentPageOrigin {
    pub fn new(value: &str) -> Result<Self, InvalidAgentPageOrigin> {
        let parsed = Url::parse(value).map_err(|_| InvalidAgentPageOrigin)?;
        if !matches!(parsed.scheme(), "http" | "https")
            || parsed.host_str().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.path() != "/"
            || parsed.query().is_some()
            || parsed.fragment().is_some()
        {
            return Err(InvalidAgentPageOrigin);
        }
        Ok(Self(
            Url::parse(&parsed.origin().ascii_serialization())
                .expect("an HTTP(S) origin is an absolute URL"),
        ))
    }

    pub fn agent_url(&self, id: &str) -> Option<String> {
        if id.trim().is_empty() {
            return None;
        }
        let mut url = self.0.clone();
        url.path_segments_mut().ok()?.push("agents").push(id);
        Some(url.into())
    }

    #[cfg(test)]
    pub(crate) fn test() -> Self {
        Self::new("https://cma.example").unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_origin_and_agent_segment_form_the_browser_link() {
        for base in ["https://cma.example", "https://cma.example/"] {
            let origin = AgentPageOrigin::new(base).unwrap();
            assert_eq!(
                origin.agent_url("agt_example").unwrap(),
                "https://cma.example/agents/agt_example"
            );
            assert_eq!(
                origin.agent_url("a/b?c").unwrap(),
                "https://cma.example/agents/a%2Fb%3Fc"
            );
            assert_eq!(origin.agent_url("  "), None);
        }
        assert_eq!(
            AgentPageOrigin::new("https://staging-cma.example:8443")
                .unwrap()
                .agent_url("agt_example")
                .unwrap(),
            "https://staging-cma.example:8443/agents/agt_example"
        );
        for invalid in [
            "ftp://cma.example",
            "https://cma.example/path",
            "https://cma.example/?q=1",
            "https://user@cma.example",
            "https://cma.example/#fragment",
        ] {
            assert!(AgentPageOrigin::new(invalid).is_err());
        }
    }
}
