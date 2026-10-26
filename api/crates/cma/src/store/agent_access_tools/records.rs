use super::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Scope {
    pub creation: AgentCreationProvenance,
    pub runtime: RuntimeBinding,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct OpenRecord {
    pub scope: Scope,
    pub request: AgentOpenRequest,
    pub fence: u64,
    pub conversation: Option<String>,
}

impl MongoAgentAccessToolSource {
    pub(super) fn capability(&self, scope: &Scope) -> Result<String> {
        derive_capability(&self.signing_key, scope)
    }

    pub(super) fn read(&self, document: &Document) -> Result<OpenRecord> {
        let record: OpenRecord = read_record(document).map_err(|_| Error::Refused)?;
        let capability = self.capability(&record.scope)?;
        if document.get_str("_id").ok() != Some(hash(&capability).as_str())
            || document.get_str("agent_id").ok() != Some(record.scope.creation.agent_id.as_str())
            || !record.request.has_valid_sha256()
            || record.request.options.slot.0 != record.scope.creation.agent_id.as_str()
            || record.fence == 0
            || record
                .conversation
                .as_deref()
                .is_some_and(|value| !validation::valid_conversation(value))
        {
            return Err(Error::Refused);
        }
        Ok(record)
    }

    pub(super) async fn save(
        &self,
        record: &OpenRecord,
        session: &mut ClientSession,
    ) -> Result<()> {
        let id = hash(&self.capability(&record.scope)?);
        let document = doc! {
            "_id": &id,
            "agent_id": record.scope.creation.agent_id.as_str(),
            "document_schema_version": DOCUMENT_SCHEMA_VERSION,
            "record": record_bson(record).map_err(|_| Error::Unavailable)?,
        };
        self.store
            .agent_access_tools
            .replace_one(doc! {"_id": id}, document)
            .upsert(true)
            .session(session)
            .await?;
        Ok(())
    }
}

pub(super) fn derive_capability(key: &ring::hmac::Key, scope: &Scope) -> Result<String> {
    let bytes = serde_json::to_vec(scope).map_err(|_| Error::Unavailable)?;
    let mut context = ring::hmac::Context::with_key(key);
    context.update(b"chrono.agent.access.capability.v1\0");
    context.update(&bytes);
    Ok(format!("cma_at_{}", hex(context.sign().as_ref())))
}

pub(super) fn valid_capability(value: &str) -> bool {
    value.strip_prefix("cma_at_").is_some_and(|value| {
        value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}
pub(super) fn hash(value: &str) -> String {
    hex(&Sha256::digest(value.as_bytes()))
}
fn hex(value: &[u8]) -> String {
    value.iter().map(|byte| format!("{byte:02x}")).collect()
}
