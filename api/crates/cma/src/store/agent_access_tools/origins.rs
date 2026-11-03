//! Explicit migration readers preserve the authority installed in a live Agent.
use super::*;

pub(super) struct Authority {
    origin: String,
    signing_key: ring::hmac::Key,
}

impl Authority {
    fn new(
        database: &str,
        origin: &str,
        encryption: &AuthEncryptionKey,
    ) -> std::result::Result<Self, String> {
        let origin = url::Url::parse(origin)
            .map_err(|_| "Agent access tools need a valid HTTPS public origin")?;
        if origin.scheme() != "https"
            || origin.host_str().is_none()
            || !origin.username().is_empty()
            || origin.password().is_some()
            || origin.query().is_some()
            || origin.fragment().is_some()
            || origin.path() != "/"
        {
            return Err("Agent access tools need a clean HTTPS public origin".into());
        }
        let origin = origin.to_string();
        let namespace = serde_json::to_string(&[database, &origin])
            .map_err(|_| "Agent access tools namespace could not be encoded")?;
        Ok(Self {
            origin,
            signing_key: encryption.agent_access_signing_key(&namespace),
        })
    }
}

impl MongoAgentAccessToolSource {
    pub(crate) fn with_retained_origins(
        store: Arc<MongoSandboxStore>,
        public_origin: &str,
        retained_origins: &[String],
        encryption: &AuthEncryptionKey,
    ) -> std::result::Result<Arc<Self>, String> {
        if retained_origins.len() > 4 {
            return Err("Agent access tools support at most four retained origins".into());
        }
        let database = store.agent_access_tools.namespace().db;
        let primary = Authority::new(&database, public_origin, encryption)?;
        let mut retained = Vec::<Authority>::new();
        for value in retained_origins {
            let authority = Authority::new(&database, value, encryption)?;
            if authority.origin != primary.origin
                && !retained
                    .iter()
                    .any(|known| known.origin == authority.origin)
            {
                retained.push(authority);
            }
        }
        Ok(Arc::new(Self {
            store,
            origin: primary.origin,
            signing_key: primary.signing_key,
            retained_origins: retained,
        }))
    }

    pub(super) fn authority(&self, index: usize) -> Result<(&str, &ring::hmac::Key)> {
        if index == 0 {
            Ok((&self.origin, &self.signing_key))
        } else {
            self.retained_origins
                .get(index - 1)
                .map(|value| (value.origin.as_str(), &value.signing_key))
                .ok_or(Error::Refused)
        }
    }

    pub(super) fn record_ids(&self, scope: &Scope) -> Result<Vec<String>> {
        (0..=self.retained_origins.len())
            .map(|index| {
                self.capability_at(scope, index)
                    .map(|value| records::hash(&value))
            })
            .collect()
    }
}
