//! A distinct, conversation-scoped relay capability; never an issuer credential.
use super::*;
use crate::RuntimeCredential;

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Relay {
    pub origin: String,
    pub endpoint: String,
    pub credential: RuntimeCredential,
}

impl HostedProviderToolBinding {
    /// Trusted source only: the capability must resolve to this exact provider
    /// binding, Agent creation, native conversation and runtime incarnation.
    /// The relay supplies the current issuer credential only on the server.
    pub fn with_transport(
        mut self,
        origin: &str,
        credential: RuntimeCredential,
    ) -> crate::Result<Self> {
        let mut endpoint = url::Url::parse(origin).map_err(|_| invalid())?;
        if endpoint.scheme() != "https"
            || endpoint.host_str().is_none()
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.query().is_some()
            || endpoint.fragment().is_some()
            || !matches!(endpoint.path(), "" | "/")
            || !valid_capability(credential.expose(), self.provenance_sha256())
        {
            return Err(invalid());
        }
        let origin = endpoint.to_string();
        endpoint.set_path("/api/v1/provider-tools");
        self.relay = Some(Relay {
            origin,
            endpoint: endpoint.to_string(),
            credential,
        });
        Ok(self)
    }

    pub fn transport_origin(&self) -> Option<&str> {
        self.relay.as_ref().map(|relay| relay.origin.as_str())
    }

    /// Compare transport independently of the adapter's fresh proxy observation.
    pub fn same_transport(&self, other: &Self) -> bool {
        self.relay == other.relay
    }
}

fn valid_capability(value: &str, provenance: &str) -> bool {
    value.strip_prefix("cma_pt_").is_some_and(|parts| {
        parts.split_once('_').is_some_and(|(scope, signature)| {
            scope == provenance
                && signature.len() == 64
                && signature
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
    })
}
