use layer3_adapter::{provider_request_digest, AdapterError};
use mongodb::bson::{doc, Document};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Backend {
    AgentSandbox,
    Opensandbox,
}

impl Backend {
    pub(super) fn from_allocation_choice(agent_sandbox: bool) -> Self {
        if agent_sandbox {
            Self::AgentSandbox
        } else {
            Self::Opensandbox
        }
    }

    pub(super) fn from_runtime_id(runtime_id: &str) -> Self {
        Self::from_allocation_choice(runtime_id.starts_with("agent-sandbox:"))
    }

    pub(super) fn is_agent_sandbox(self) -> bool {
        self == Self::AgentSandbox
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::AgentSandbox => "agent-sandbox",
            Self::Opensandbox => "opensandbox",
        }
    }
}

pub(super) fn read_backend(row: &Document) -> layer3_adapter::Result<Backend> {
    match row.get_str("backend") {
        Ok("agent-sandbox") => Ok(Backend::AgentSandbox),
        Ok("opensandbox") => Ok(Backend::Opensandbox),
        _ => Err(AdapterError::Rejected(
            "invalid recorded runtime provider".into(),
        )),
    }
}

pub(super) struct RouteKey<'a> {
    id: String,
    deployment: &'a str,
    sandbox: &'a str,
    generation: u64,
}

impl<'a> RouteKey<'a> {
    pub(super) fn new(
        deployment: &'a str,
        sandbox: &'a str,
        generation: u64,
    ) -> layer3_adapter::Result<Self> {
        Ok(Self {
            id: provider_request_digest(&(deployment, sandbox, generation))?,
            deployment,
            sandbox,
            generation,
        })
    }

    pub(super) fn filter(&self) -> Document {
        doc! {"_id": &self.id}
    }

    pub(super) fn insert_fields(&self, backend: Backend) -> layer3_adapter::Result<Document> {
        let generation = i64::try_from(self.generation)
            .map_err(|_| AdapterError::Rejected("runtime generation is out of range".into()))?;
        Ok(doc! {
            "document_schema_version": 1_i64,
            "sandbox_id": self.sandbox,
            "generation": generation,
            "deployment": self.deployment,
            "backend": backend.as_str()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_route_key_and_insert_fields_match_v1() {
        let key = RouteKey::new("fixture", "sbx_fixture", 7).unwrap();
        assert_eq!(
            key.filter(),
            doc! {"_id": "46967dc86922b585b60de967dbf4b3af0d7604a6ea81b946b376a985190b192f"}
        );
        for (backend, serialized) in [
            (Backend::AgentSandbox, "agent-sandbox"),
            (Backend::Opensandbox, "opensandbox"),
        ] {
            let fields = key.insert_fields(backend).unwrap();
            assert_eq!(
                fields,
                doc! {
                    "document_schema_version": 1_i64,
                    "sandbox_id": "sbx_fixture",
                    "generation": 7_i64,
                    "deployment": "fixture",
                    "backend": serialized
                }
            );
            assert_eq!(read_backend(&fields).unwrap(), backend);
        }
    }

    #[test]
    fn route_key_stays_readable_when_generation_cannot_be_inserted() {
        let key = RouteKey::new("fixture", "sbx_fixture", u64::MAX).unwrap();
        assert!(key.filter().get_str("_id").is_ok());
        assert!(matches!(
            key.insert_fields(Backend::Opensandbox),
            Err(AdapterError::Rejected(_))
        ));
    }
}
