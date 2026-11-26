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
    /// Derived from the stored capability hash, never trusted from persisted data.
    #[serde(skip)]
    pub authority: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport_origin: Option<String>,
    pub scope: Scope,
    pub request: AgentOpenRequest,
    pub fence: u64,
    pub conversation: Option<String>,
}

impl MongoAgentAccessToolSource {
    pub(super) async fn current_record(
        &self,
        scope: &Scope,
        session: &mut ClientSession,
    ) -> Result<Option<OpenRecord>> {
        let mut cursor = self
            .store
            .agent_access_tools
            .find(doc! {"_id": {"$in": self.records.record_ids(scope)?}})
            .limit(2)
            .session(&mut *session)
            .await?;
        let record = cursor
            .next(&mut *session)
            .await
            .transpose()?
            .as_ref()
            .map(|document| self.records.read(document))
            .transpose()?;
        // Never guess which endpoint was installed if two namespaces claim one incarnation.
        if cursor.next(&mut *session).await.transpose()?.is_some() {
            return Err(Error::Refused);
        }
        Ok(record)
    }

    #[cfg(test)]
    pub(super) fn capability(&self, scope: &Scope) -> Result<String> {
        self.records.capability_at(scope, 0)
    }

    pub(super) fn capability_at(&self, scope: &Scope, authority: usize) -> Result<String> {
        self.records.capability_at(scope, authority)
    }

    pub(super) fn authority(&self, authority: usize) -> Result<(&str, &ring::hmac::Key)> {
        self.records.authority(authority)
    }

    pub(super) fn transport_origin<'a>(&'a self, record: &'a OpenRecord) -> Result<&'a str> {
        match &record.transport_origin {
            Some(origin) => Ok(origin),
            None => self.authority(record.authority).map(|(origin, _)| origin),
        }
    }

    pub(super) async fn save(
        &self,
        record: &OpenRecord,
        session: &mut ClientSession,
    ) -> Result<()> {
        let id = hash(&self.capability_at(&record.scope, record.authority)?);
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

impl AccessRecordAuthority {
    pub(super) fn read(&self, document: &Document) -> Result<OpenRecord> {
        let mut record: OpenRecord = read_record(document).map_err(|_| Error::Refused)?;
        record.authority = self
            .record_ids(&record.scope)?
            .iter()
            .position(|id| document.get_str("_id").ok() == Some(id.as_str()))
            .ok_or(Error::Refused)?;
        if record
            .transport_origin
            .as_ref()
            .is_some_and(|origin| !self.recognizes_origin(origin))
        {
            return Err(Error::Refused);
        }
        if document.get_str("agent_id").ok() != Some(record.scope.creation.agent_id.as_str())
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
