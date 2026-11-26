use super::{model::*, store::Store, Error};
use crate::{
    auth::{AgentCaller, Creator},
    channels::management::ManagementState,
    tools::ToolsState,
};
use cmaeg_core::{
    activity::{ActorKind, ConversationKind},
    seal::{self, EventRef, TokenKind},
};
use sha2::{Digest, Sha256};

#[derive(Clone)]
pub struct ConnectionService {
    pub management: ManagementState,
    pub tools: ToolsState,
    pub store: Store,
}
impl ConnectionService {
    pub fn new(management: ManagementState, tools: ToolsState) -> Self {
        let store = Store::new(
            (*management.records).clone(),
            management.master_keys.clone(),
        );
        Self {
            management,
            tools,
            store,
        }
    }
    pub async fn create(
        &self,
        owner: &Creator,
        agent: AgentCaller,
        request: CreateConnection,
    ) -> Result<ConnectionView, Error> {
        let id = uuid::Uuid::parse_str(&request.record_id)
            .map_err(|_| Error::Invalid("record_id must be a canonical UUID v4"))?;
        if id.get_version_num() != 4 || id.to_string() != request.record_id {
            return Err(Error::Invalid("record_id must be a canonical UUID v4"));
        }
        if request.suggested_name.as_ref().is_some_and(|name| {
            name.is_empty() || name.len() > 64 || name.chars().any(char::is_control)
        }) {
            return Err(Error::Invalid(
                "suggested_name must contain 1..=64 printable bytes",
            ));
        }
        if !self.management.managed_telegram_setup_enabled {
            match self.owned(owner, &request.record_id).await {
                Ok(_) => {}
                Err(Error::NotFound) => {
                    return Err(Error::Forbidden("managed_telegram_setup_disabled"))
                }
                Err(error) => return Err(error),
            }
        }
        let reference: EventRef = request
            .event_ref
            .parse()
            .map_err(|_| Error::Invalid("invalid event reference"))?;
        let (channel, event, _) = self
            .tools
            .authorize_event(agent, &reference, "createChannelConnection")
            .await
            .map_err(|_| Error::NotFound)?;
        // Connection setup stays the default consumer's: a connection binds a
        // platform to the channel, not to one of its consumers.
        if event.consumer.is_some() {
            return Err(Error::Forbidden("consumer_not_supported"));
        }
        let context = self
            .tools
            .context(agent, &reference)
            .await
            .map_err(|_| Error::Unavailable)?;
        if !context.activity_verified
            || context.activity.actor.kind != ActorKind::Human
            || context.activity.conversation.kind != ConversationKind::Private
        {
            return Err(Error::Invalid(
                "connection setup requires a verified private human event",
            ));
        }
        let fingerprint = format!("{:x}",Sha256::digest(serde_json::to_vec(&serde_json::json!({
            "owner":owner.subject,"organization":owner.organization,"event_id":event.event_id,
            "channel":event.channel_id,"activity":event.activity_sha256,"suggested_name":request.suggested_name
        })).map_err(|_|Error::Unavailable)?));
        match self.store.connection(&request.record_id).await {
            Ok((record, _)) => {
                if record.owner != owner.subject || record.organization != owner.organization {
                    return Err(Error::NotFound);
                }
                if record.fingerprint != fingerprint {
                    return Err(Error::Conflict);
                }
                return self.view(&record);
            }
            Err(Error::NotFound) => {}
            Err(e) => return Err(e),
        }
        let record = Connection {
            id: request.record_id.clone(),
            owner: owner.subject.clone(),
            organization: owner.organization,
            created_at: now(),
            expires_at: event.expires_at.min(now() + SETUP_TTL),
            fingerprint,
            origin: Origin {
                event,
                actor: context.activity.actor.id,
                conversation: context.activity.conversation,
                source: context.source,
                binding_id: channel.definition.provider.binding_id.to_string(),
                agent_key_hash: channel.definition.secrets.agent_key_hash.clone(),
            },
            suggested_name: request.suggested_name,
            status: Status::Draft,
            setup_nonce: random_nonce(),
            browser_nonce: None,
            browser_claimed: false,
            requested_manager: false,
            material: None,
            channel_record: None,
            channel_id: None,
            webhook_retiring: false,
            webhook_lease: None,
            notified: false,
            completion_ref: None,
        };
        match self.store.save_connection(&record, None).await {
            Ok(_) => self.view(&record),
            Err(Error::Conflict) => {
                let (other, _) = self.owned(owner, &record.id).await?;
                if other.fingerprint != record.fingerprint {
                    return Err(Error::Conflict);
                }
                self.view(&other)
            }
            Err(e) => Err(e),
        }
    }
    pub async fn owned(&self, owner: &Creator, id: &str) -> Result<(Connection, u64), Error> {
        let (record, version) = self.store.connection(id).await?;
        if record.owner != owner.subject || record.organization != owner.organization {
            return Err(Error::NotFound);
        }
        Ok((record, version))
    }
    pub async fn get(&self, owner: &Creator, id: &str) -> Result<ConnectionView, Error> {
        self.view(&self.owned(owner, id).await?.0)
    }
    pub async fn list(
        &self,
        owner: &Creator,
        after: Option<&str>,
    ) -> Result<(Vec<ConnectionView>, Option<String>), Error> {
        let heads = self.store.page(Some(&owner.subject), after).await?;
        let next = (heads.len() == 100).then(|| heads.last().unwrap().record_id.clone());
        let mut items = Vec::new();
        for head in heads {
            match self.owned(owner, &head.record_id).await {
                Ok((record, _))
                    if record.status == Status::Ready && record.channel_record.is_none() =>
                {
                    items.push(self.view(&record)?)
                }
                Ok(_) | Err(Error::NotFound) => {}
                Err(error) => return Err(error),
            }
        }
        Ok((items, next))
    }
    pub async fn cancel(&self, owner: &Creator, id: &str) -> Result<(), Error> {
        let (mut record, version) = self.owned(owner, id).await?;
        if record.channel_record.is_some() && self.channel_exists(&record).await? {
            return Err(Error::Conflict);
        }
        record.status = Status::Cancelled;
        record.browser_nonce = None;
        record.webhook_retiring |= record.channel_record.is_some()
            || record.channel_id.is_some()
            || record.webhook_lease.is_some();
        self.store.save_connection(&record, Some(version)).await?;
        if record.webhook_retiring {
            match self.reconcile_webhook_retirement(id).await {
                Ok(()) | Err(Error::Conflict) => {}
                Err(error) => return Err(error),
            }
        }
        self.reconcile_terminal_claim(&self.store.connection(id).await?.0)
            .await?;
        Ok(())
    }
    pub fn view(&self, record: &Connection) -> Result<ConnectionView, Error> {
        let status = effective_status(record);
        let setup_url = matches!(status, Status::Draft | Status::AwaitingPlatform)
            .then(|| {
                let token = self.capability(record, "start", &record.setup_nonce)?;
                Ok::<_, Error>(format!(
                    "{}/connect/start/{token}",
                    self.management.advertised_api_url()
                ))
            })
            .transpose()?;
        Ok(ConnectionView {
            connection_ref: record.id.clone(),
            status,
            label: record
                .material
                .as_ref()
                .map(|m| m.name.clone())
                .or_else(|| record.suggested_name.clone()),
            url: record
                .material
                .as_ref()
                .map(|m| format!("https://t.me/{}", m.username)),
            setup_url,
        })
    }
    pub fn capability(
        &self,
        record: &Connection,
        purpose: &str,
        nonce: &str,
    ) -> Result<String, Error> {
        seal::seal(
            &self.management.master_keys,
            TokenKind::ConnectionSetup,
            &SetupCapability {
                purpose: purpose.into(),
                link_nonce: Some(record.setup_nonce.clone()),
                record_id: record.id.clone(),
                nonce: nonce.into(),
                expires_at: record.expires_at,
            },
        )
        .map_err(|_| Error::Unavailable)
    }
    pub fn open_capability(&self, token: &str, purpose: &str) -> Result<SetupCapability, Error> {
        let value: SetupCapability = seal::open(
            &self.management.master_keys,
            TokenKind::ConnectionSetup,
            token,
        )
        .map_err(|_| Error::NotFound)?
        .payload;
        if value.purpose != purpose || value.expires_at <= now() {
            return Err(Error::NotFound);
        }
        Ok(value)
    }
}
pub fn random_nonce() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}
pub fn effective_status(record: &Connection) -> Status {
    if record.expires_at <= now()
        && matches!(record.status, Status::Draft | Status::AwaitingPlatform)
    {
        Status::Expired
    } else {
        record.status
    }
}
