//! Explicit migration readers preserve the authority installed in a live Agent.
use super::*;

struct Authority {
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

pub(in crate::store) struct AccessRecordAuthority {
    keys: Vec<Authority>,
}

impl AccessRecordAuthority {
    pub(in crate::store) fn new(
        database: &str,
        public_origin: &str,
        retained_origins: &[String],
        encryption: &AuthEncryptionKey,
    ) -> std::result::Result<Self, String> {
        if retained_origins.len() > 4 {
            return Err("Agent access tools support at most four retained origins".into());
        }
        let mut keys = vec![Authority::new(database, public_origin, encryption)?];
        for value in retained_origins {
            let authority = Authority::new(database, value, encryption)?;
            if !keys.iter().any(|known| known.origin == authority.origin) {
                keys.push(authority);
            }
        }
        Ok(Self { keys })
    }

    pub(super) fn authority(&self, index: usize) -> Result<(&str, &ring::hmac::Key)> {
        self.keys
            .get(index)
            .map(|value| (value.origin.as_str(), &value.signing_key))
            .ok_or(Error::Refused)
    }

    pub(super) fn recognizes_origin(&self, target: &str) -> bool {
        self.keys.iter().any(|authority| authority.origin == target)
    }

    pub(super) fn capability_at(&self, scope: &Scope, index: usize) -> Result<String> {
        records::derive_capability(self.authority(index)?.1, scope)
    }

    pub(super) fn record_ids(&self, scope: &Scope) -> Result<Vec<String>> {
        (0..self.keys.len())
            .map(|index| {
                self.capability_at(scope, index)
                    .map(|value| records::hash(&value))
            })
            .collect()
    }

    pub(in crate::store) fn namespace_scope(&self) -> Vec<(&str, Vec<u8>)> {
        self.keys
            .iter()
            .map(|authority| {
                let binding =
                    ring::hmac::sign(&authority.signing_key, b"cma.online-backfill.scope.v1");
                (authority.origin.as_str(), binding.as_ref().to_vec())
            })
            .collect()
    }

    pub(in crate::store) fn lookup_filter(
        &self,
        creation: &AgentCreationProvenance,
        runtime: &RuntimeBinding,
    ) -> Option<Document> {
        let ids = self
            .record_ids(&Scope {
                creation: creation.clone(),
                runtime: runtime.clone(),
            })
            .ok()?;
        Some(doc! {"_id": {"$in": ids}})
    }

    pub(in crate::store) fn acknowledged_transport_open(
        &self,
        document: &Document,
        creation: &AgentCreationProvenance,
        runtime: &RuntimeBinding,
        conversation: &str,
        target: &str,
    ) -> std::result::Result<Option<AgentOpenRequest>, ()> {
        let record = self.read(document).map_err(|_| ())?;
        if record.scope.creation != *creation
            || record.scope.runtime != *runtime
            || record.conversation.as_deref() != Some(conversation)
        {
            return Err(());
        }
        Ok((record.transport_origin.as_deref() == Some(target)).then_some(record.request))
    }
}

impl MongoAgentAccessToolSource {
    pub(crate) fn with_retained_origins(
        store: Arc<MongoSandboxStore>,
        public_origin: &str,
        retained_origins: &[String],
        encryption: &AuthEncryptionKey,
    ) -> std::result::Result<Arc<Self>, String> {
        let database = store.agent_access_tools.namespace().db;
        let records =
            AccessRecordAuthority::new(&database, public_origin, retained_origins, encryption)?;
        Ok(Arc::new(Self {
            standing: Arc::new(layer2_core::account_deletion::AvailableAccounts),
            store,
            records,
        }))
    }
}
