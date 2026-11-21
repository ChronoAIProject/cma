//! Exact confirmed outbound destinations, sealed only after Discord returns an ID.
use super::{journal_model::Authority, record_store::Store, GatewayError};
use crate::{
    channels::resolver::ResolvedChannel,
    connections::store::index_id,
    ingress::routing_store::{EventIdentity, PlanStore},
    records::RecordClient,
};
use cmaeg_core::{
    activity::{Activity, SourceKind},
    channel::ProviderBinding,
    ids::{ConsumerId, ConversationId},
    seal::{EventRefPayload, MasterKeyRing, ReplyAddress},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Receipt {
    pub kind: String,
    pub original: EventIdentity,
    pub activity: Activity,
    pub source_revision: String,
    pub consumer: ConsumerId,
    pub provider: ProviderBinding,
    pub partition_key: String,
    pub conversation_id: ConversationId,
    pub bot_id: String,
    pub channel_id: String,
    pub guild_id: Option<String>,
    pub message_id: String,
    pub confirmed_at: i64,
    pub retained_until: i64,
}
pub(crate) struct Prepared {
    store: Store,
    receipt: Receipt,
}
impl Prepared {
    pub(crate) async fn new(
        records: RecordClient,
        keys: Arc<MasterKeyRing>,
        channel: &ResolvedChannel,
        event: &EventRefPayload,
    ) -> Result<Self, GatewayError> {
        let authority = Authority::from_definition(&channel.definition)?;
        let ReplyAddress::Discord {
            bot_id,
            channel_id,
            thread_id,
            ..
        } = &event.reply
        else {
            return Err(GatewayError::Authority);
        };
        let original = EventIdentity {
            owner: authority.owner.clone(),
            channel_id: channel.id.clone(),
            record_id: channel.definition.record_id.clone(),
            event_id: event.event_id.clone(),
        };
        let plan = PlanStore::new(records.clone(), keys.clone())
            .load(&original)
            .await
            .map_err(|_| GatewayError::Unavailable)?
            .ok_or(GatewayError::Unavailable)?;
        let consumer = event
            .consumer
            .clone()
            .unwrap_or_else(ConsumerId::default_consumer);
        let target = plan
            .targets
            .iter()
            .find(|target| target.launch.consumer == consumer)
            .ok_or(GatewayError::Authority)?;
        if bot_id != &authority.bot_id
            || !event.verifies_activity(&plan.activity)
            || plan.activity.source
                != (SourceKind::Discord {
                    bot_id: bot_id.clone(),
                })
            || plan.activity.conversation.id != *channel_id
            || plan.activity.conversation.thread_id != *thread_id
            || plan.source
                != *channel
                    .definition
                    .source_of(&plan.activity.source)
                    .ok_or(GatewayError::Authority)?
            || target.launch.event != *event
            || channel
                .consumer(event.consumer.as_ref())
                .is_none_or(|current| {
                    current.pending
                        || current.credential.is_none()
                        || current.provider != &target.launch.provider
                })
        {
            return Err(GatewayError::Authority);
        }
        let body: serde_json::Value =
            serde_json::from_slice(&plan.source_body).map_err(|_| GatewayError::Protocol)?;
        let guild_id = body["dispatch"]
            .get("guild_id")
            .map(|id| super::normalize::snowflake(id).map(str::to_owned))
            .transpose()?;
        Ok(Self {
            store: Store::new(records, keys),
            receipt: Receipt {
                kind: "discord_confirmed_outbound_v1".into(),
                original,
                activity: plan.activity,
                source_revision: authority.revision,
                consumer,
                provider: target.launch.provider.clone(),
                partition_key: event.partition_key.clone(),
                conversation_id: event.conversation_id.clone(),
                bot_id: bot_id.clone(),
                channel_id: channel_id.clone(),
                guild_id,
                message_id: String::new(),
                confirmed_at: 0,
                retained_until: plan.retained_until,
            },
        })
    }
    pub(crate) async fn confirm(&self, message: &str) -> Result<(), GatewayError> {
        if !cmaeg_core::channel::valid_discord_id(message) {
            return Err(GatewayError::Protocol);
        }
        let mut receipt = self.receipt.clone();
        receipt.message_id = message.into();
        receipt.confirmed_at = (self.store.clock)() / 1000;
        if receipt.confirmed_at >= receipt.retained_until {
            return Err(GatewayError::Authority);
        }
        self.store
            .immutable(
                &receipt.id(),
                &receipt.original.owner,
                &receipt,
                receipt.retained_until,
            )
            .await
    }
}
impl Receipt {
    pub(super) fn id(&self) -> String {
        id(
            self.original.record_id.as_str(),
            &self.bot_id,
            &self.channel_id,
            &self.message_id,
        )
    }
    pub(super) fn valid(
        &self,
        authority: &Authority,
        channel: &str,
        message: &str,
        now: i64,
    ) -> bool {
        self.kind == "discord_confirmed_outbound_v1"
            && self.original.owner == authority.owner
            && self.original.record_id.as_str() == authority.record
            && self.source_revision == authority.revision
            && self.bot_id == authority.bot_id
            && self.channel_id == channel
            && self.message_id == message
            && self.confirmed_at > 0
            && self.confirmed_at <= now
            && self.retained_until > now
            && self.retained_until - self.confirmed_at <= 7 * 86_400
            && self.activity.event_id == self.original.event_id
            && self.activity.source
                == (SourceKind::Discord {
                    bot_id: self.bot_id.clone(),
                })
            && self.activity.conversation.id == channel
            && self.activity.validate().is_ok()
    }
}
pub(super) fn id(record: &str, bot: &str, channel: &str, message: &str) -> String {
    index_id(
        "discord-confirmed-outbound-v1",
        &[record, bot, channel, message],
    )
}
