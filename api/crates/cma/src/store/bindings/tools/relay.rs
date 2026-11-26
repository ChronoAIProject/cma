//! Relay enrollment is distinct from the original provider open receipt.
use super::*;
use crate::config::ProviderToolRelay;
use crate::store::{AuthEncryptionKey, MongoSandboxCredentialStore};

pub(super) struct Authority {
    pub config: ProviderToolRelay,
    pub origin: String,
    pub custody: Arc<MongoSandboxCredentialStore>,
    signing: ring::hmac::Key,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Record {
    pub scope: Scope,
    pub request: AgentOpenRequest,
    pub fence: u64,
    pub origin: String,
    pub conversation: Option<String>,
    pub boot_id: Option<String>,
    pub confirmed: bool,
    pub migration_sequence: Option<u64>,
}

impl MongoProviderToolSource {
    pub(crate) fn with_relay(
        store: Arc<MongoSandboxStore>,
        custody: Arc<MongoSandboxCredentialStore>,
        config: Option<ProviderToolRelay>,
        origin: &str,
        encryption: &AuthEncryptionKey,
    ) -> std::result::Result<Arc<Self>, String> {
        let Some(config) = config else {
            return Ok(Self::new(store));
        };
        let origin = url::Url::parse(origin)
            .map_err(|_| "invalid provider relay origin")?
            .to_string();
        let namespace =
            serde_json::to_string(&[store.bindings.tool_relays.namespace().db.as_str(), &origin])
                .map_err(|_| "invalid provider relay namespace")?;
        Ok(Arc::new(Self {
            standing: Arc::new(layer2_core::account_deletion::AvailableAccounts),
            store,
            relay: Some(Authority {
                config,
                origin,
                custody,
                signing: encryption.provider_tool_signing_key(&namespace),
            }),
        }))
    }

    pub(super) fn relay_authority(&self, scope: &Scope) -> layer3_adapter::Result<&Authority> {
        self.relay
            .as_ref()
            .filter(|authority| authority.config.applies(&scope.binding_id, &scope.endpoint))
            .ok_or_else(refused)
    }

    pub(super) fn relay_binding(
        &self,
        binding: HostedProviderToolBinding,
        scope: &Scope,
    ) -> layer3_adapter::Result<HostedProviderToolBinding> {
        let authority = self.relay_authority(scope)?;
        binding.with_transport(&authority.origin, authority.capability(&scope.id()?).into())
    }

    pub(super) async fn relay_record(
        &self,
        scope: &Scope,
        session: &mut ClientSession,
    ) -> transaction::Result<Option<Record>> {
        self.store
            .bindings
            .tool_relays
            .find_one(doc! {"_id": scope.id()?})
            .session(&mut *session)
            .await?
            .as_ref()
            .map(|document| self.read_relay(document))
            .transpose()
            .map_err(Into::into)
    }

    pub(super) fn read_relay(&self, document: &Document) -> layer3_adapter::Result<Record> {
        let record: Record = read_record(document).map_err(unavailable)?;
        let authority = self.relay_authority(&record.scope)?;
        if document.get_str("_id").ok() != Some(record.scope.id()?.as_str())
            || record.origin != authority.origin
            || record.fence == 0
            || !record.request.has_valid_sha256()
            || !record.scope.matches_open(&record.request)
            || record.request.options.slot.0 != record.scope.agent_id.as_str()
            || record.confirmed && (record.conversation.is_none() || record.boot_id.is_none())
            || record.conversation.as_ref().is_some_and(|id| {
                id.is_empty() || id.len() > 512 || id.chars().any(char::is_control)
            })
        {
            return Err(refused());
        }
        Ok(record)
    }

    pub(super) async fn save_relay(
        &self,
        record: &Record,
        session: &mut ClientSession,
    ) -> transaction::Result<()> {
        let id = record.scope.id()?;
        let document = doc! {"_id": &id,"document_schema_version":DOCUMENT_SCHEMA_VERSION,"agent_id":record.scope.agent_id.as_str(),"record":record_bson(record).map_err(unavailable)?};
        self.read_relay(&document)?;
        self.store
            .bindings
            .tool_relays
            .replace_one(doc! {"_id":id}, document)
            .upsert(true)
            .session(session)
            .await?;
        Ok(())
    }

    pub(super) async fn relay_for_open(
        &self,
        attachment: HostedProviderToolBinding,
        scope: &Scope,
        previously_opened: bool,
        session: &mut ClientSession,
    ) -> transaction::Result<HostedProviderToolBinding> {
        let Some(authority) = self
            .relay
            .as_ref()
            .filter(|a| a.config.applies(&scope.binding_id, &scope.endpoint))
        else {
            return Ok(attachment);
        };
        let existing = self.relay_record(scope, session).await?;
        let (_, request, fence, _) = attachment.scope();
        let retain_relay = if !existing.as_ref().is_some_and(|record| record.confirmed)
            && request.restart_runtime.is_some()
        {
            self.predecessor_used_relay(scope, request, session).await?
        } else {
            false
        };
        // Existing connections change transport only through idle maintenance.
        // A successor may enroll setup for the predecessor's confirmed relay.
        if existing.is_none() && previously_opened && !retain_relay {
            return Ok(attachment);
        }
        let mut record = existing.unwrap_or_else(|| Record {
            scope: scope.clone(),
            request: request.clone(),
            fence,
            origin: authority.origin.clone(),
            conversation: None,
            boot_id: None,
            confirmed: false,
            migration_sequence: None,
        });
        record.request = request.clone();
        record.fence = fence;
        self.save_relay(&record, session).await?;
        if record.confirmed || !previously_opened || retain_relay {
            Ok(self.relay_binding(attachment, scope)?)
        } else {
            Ok(attachment)
        }
    }

    async fn predecessor_used_relay(
        &self,
        scope: &Scope,
        request: &AgentOpenRequest,
        session: &mut ClientSession,
    ) -> transaction::Result<bool> {
        use crate::store::agent_control_transaction::restart_dispatch::read_work;
        use layer2_core::sandbox::AgentWorkStep;

        let row = self
            .store
            .agents
            .find_one(doc! {"_id": scope.agent_id.as_str()})
            .session(&mut *session)
            .await?
            .ok_or_else(refused)?;
        let agent = crate::store::agents::read_agent(&row).map_err(unavailable)?;
        if agent.work.step != AgentWorkStep::RestartConversation {
            return Ok(false);
        }
        let work = read_work(&row, &agent).map_err(unavailable)?;
        if work.dispatched_open.as_ref() != Some(request) {
            return Err(refused().into());
        }
        let mut predecessor = scope.clone();
        let open = work.acceptance.source.predecessor_open;
        predecessor.request_sha256 = open.request_sha256;
        predecessor.numbered_open = layer3_adapter::AgentProcessAction::Open
            .incarnation(scope.agent_id.as_str(), &open.operation_id)
            .filter(|number| *number > 1)
            .map(|_| open.operation_id.clone());
        let previous = self.relay_record(&predecessor, session).await?;
        Ok(previous.is_some_and(|record| {
            record.confirmed
                && record.request.operation_id == open.operation_id
                && record.conversation.as_ref() == request.resume_conversation.as_ref()
                && record.boot_id == scope.runtime.boot_id
        }))
    }

    pub(super) async fn confirm_relay_open(
        &self,
        attachment: &HostedProviderToolBinding,
        scope: &Scope,
        conversation: &str,
        session: &mut ClientSession,
    ) -> transaction::Result<()> {
        if attachment.transport_origin().is_none() {
            return Ok(());
        }
        let mut record = self
            .relay_record(scope, session)
            .await?
            .ok_or_else(refused)?;
        let expected = self.relay_binding(attachment.clone(), scope)?;
        if !expected.same_transport(attachment)
            || record.fence != attachment.scope().2
            || record
                .conversation
                .as_deref()
                .is_some_and(|id| id != conversation)
            || record.migration_sequence.is_some() && !record.confirmed
        {
            return Err(refused().into());
        }
        record.conversation = Some(conversation.into());
        record.boot_id = Some(attachment.observed().ok_or_else(refused)?.boot_id.clone());
        record.confirmed = true;
        self.save_relay(&record, session).await
    }
}

impl Authority {
    pub(super) fn capability(&self, id: &str) -> String {
        let signature = ring::hmac::sign(&self.signing, id.as_bytes());
        let hex: String = signature
            .as_ref()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        format!("cma_pt_{id}_{hex}")
    }

    pub(super) fn verify<'a>(&self, value: &'a str) -> layer3_adapter::Result<&'a str> {
        let (id, signature) = value
            .strip_prefix("cma_pt_")
            .and_then(|value| value.split_once('_'))
            .ok_or_else(refused)?;
        if [id, signature].iter().any(|value| {
            value.len() != 64
                || !value
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        }) {
            return Err(refused());
        }
        let bytes: Vec<u8> = (0..64)
            .step_by(2)
            .map(|offset| {
                u8::from_str_radix(&signature[offset..offset + 2], 16).map_err(|_| refused())
            })
            .collect::<layer3_adapter::Result<_>>()?;
        ring::hmac::verify(&self.signing, id.as_bytes(), &bytes).map_err(|_| refused())?;
        Ok(id)
    }
}
