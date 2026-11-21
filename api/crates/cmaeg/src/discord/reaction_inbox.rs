//! Reaction custody precedes the Gateway checkpoint and preserves original targets.
use super::{
    inbox_reader::Body,
    journal::DurableJournal,
    journal_model::{Authority, Inbox},
    normalize::{snowflake, Normalized},
    reactions::{self, Target},
    receipts::Receipt,
    Checkpoint, Dispatch, GatewayError,
};
use crate::connections::store::index_id;
use cmaeg_core::ids::{EventId, RecordId};
use serde_json::Value;

impl DurableJournal {
    pub(super) async fn persist_reaction(
        &self,
        authority: &Authority,
        checkpoint: &Checkpoint,
        event: &Dispatch,
    ) -> Result<(), GatewayError> {
        let id = reactions::delivery_id(authority, &checkpoint.session_id, event.sequence)?;
        let refuse = |reason| self.reaction_refused(authority, &id, reason);
        let raw = &event.data;
        let (Ok(message), Ok(channel)) =
            (snowflake(&raw["message_id"]), snowflake(&raw["channel_id"]))
        else {
            return refuse("discord_reaction_invalid_target");
        };
        let opted = authority.intents
            & if raw.get("guild_id").is_some() {
                1 << 10
            } else {
                1 << 13
            }
            != 0;
        if !opted {
            return refuse("discord_reaction_intent_not_enabled");
        }
        let inbox_id = index_id(
            "discord-native-message-event-v1",
            &[&authority.record, &authority.bot_id, id.as_str()],
        );
        if let Some((retained, version)) = self.store.load::<Inbox>(&inbox_id, true).await? {
            if version != 1
                || retained.id()? != inbox_id
                || retained.source_revision != authority.revision
            {
                return Err(GatewayError::Protocol);
            }
            let bytes = self.verify_inbox(&retained, authority, message).await?;
            let body: Body = serde_json::from_slice(&bytes).map_err(|_| GatewayError::Protocol)?;
            if body.dispatch != *raw
                || body.message_event.as_ref().is_none_or(|snapshot| {
                    snapshot.activity.event_id != id
                        || operation(&event.name) != snapshot_operation(snapshot)
                })
            {
                return Err(GatewayError::Protocol);
            }
            return self.observe_inbox(&retained, authority, &bytes).await;
        }
        if self.pending_reaction_replay(authority, event, &id).await? {
            return Ok(());
        }
        let actor = if matches!(
            event.name.as_str(),
            "MESSAGE_REACTION_REMOVE_ALL" | "MESSAGE_REACTION_REMOVE_EMOJI"
        ) {
            None
        } else {
            let Ok(user_id) = snowflake(&raw["user_id"]) else {
                return refuse("discord_reaction_actor_unproven");
            };
            if user_id == authority.bot_id {
                return refuse("discord_reaction_bot_sender");
            }
            let user = match raw.pointer("/member/user") {
                Some(user)
                    if event.name == "MESSAGE_REACTION_ADD" && raw.get("guild_id").is_some() =>
                {
                    user.clone()
                }
                _ => {
                    self.api
                        .get::<Value>(&format!("users/{user_id}"), &authority.token)
                        .await?
                }
            };
            match reactions::actor(raw, &user, &authority.bot_id) {
                Ok(actor) => Some(actor),
                Err(_) => return refuse("discord_reaction_actor_unproven_or_automated"),
            }
        };
        let target = match self
            .store
            .reaction_target(authority, channel, message)
            .await
        {
            Ok(Some(target)) => target,
            Ok(None) => {
                return self
                    .persist_pending_reaction(authority, event, id, actor)
                    .await
            }
            Err(GatewayError::Authority) => {
                return refuse("discord_reaction_target_authority_changed")
            }
            Err(error) => return Err(error),
        };
        let expiry = target.retained_until;
        let snapshot = match reactions::normalize(
            authority,
            &event.name,
            raw,
            target,
            actor,
            id.clone(),
            (self.store.clock)() / 1000,
        ) {
            Ok(snapshot) => snapshot,
            Err(_) => return refuse("discord_reaction_target_actor_or_delta_refused"),
        };
        self.persist_change_snapshot(authority, message, raw, snapshot, expiry)
            .await
    }
    fn reaction_refused(
        &self,
        authority: &Authority,
        id: &EventId,
        reason: &'static str,
    ) -> Result<(), GatewayError> {
        let record = RecordId::new(&authority.record).map_err(|_| GatewayError::Protocol)?;
        self.activity.refused(&record, "discord", Some(id), reason);
        Ok(())
    }
}
fn operation(name: &str) -> &str {
    match name {
        "MESSAGE_REACTION_ADD" => "add",
        "MESSAGE_REACTION_REMOVE" => "remove",
        "MESSAGE_REACTION_REMOVE_ALL" => "clear_all",
        "MESSAGE_REACTION_REMOVE_EMOJI" => "clear_emoji",
        _ => "invalid",
    }
}
fn snapshot_operation(snapshot: &super::message_event::Snapshot) -> &str {
    match &snapshot.activity.kind {
        cmaeg_core::activity::ActivityKind::Event { ce, .. } => ce
            .data
            .as_ref()
            .and_then(|data| data["operation"].as_str())
            .unwrap_or("invalid"),
        _ => "invalid",
    }
}

impl super::record_store::Store {
    pub(super) async fn reaction_target(
        &self,
        authority: &Authority,
        channel: &str,
        message: &str,
    ) -> Result<Option<Target>, GatewayError> {
        let id = index_id(
            "discord-native-message-v1",
            &[&authority.record, &authority.bot_id, message],
        );
        if let Some((original, version)) = self.load::<Inbox>(&id, true).await? {
            if version != 1
                || original.kind != "discord_inbox_v1"
                || original.source_revision != authority.revision
            {
                return Err(GatewayError::Authority);
            }
            let bytes = self.verify_inbox(&original, authority, message).await?;
            let body: Body = serde_json::from_slice(&bytes).map_err(|_| GatewayError::Protocol)?;
            if body.channel.owner.subject != authority.owner
                || body.channel.record_id.as_str() != authority.record
                || body.component.is_some()
                || body.message_event.is_some()
                || body.dispatch["channel_id"].as_str() != Some(channel)
            {
                return Err(GatewayError::Authority);
            }
            let Normalized::Activity(activity) = body.activity(&authority.bot_id)? else {
                return Ok(None);
            };
            return Ok(Some(Target {
                original: activity.event_id.clone(),
                activity: *activity,
                guild: body
                    .dispatch
                    .get("guild_id")
                    .map(snowflake)
                    .transpose()?
                    .map(str::to_owned),
                outbound_consumer: None,
                retained_until: original.retained_until,
            }));
        }
        let id = super::receipts::id(&authority.record, &authority.bot_id, channel, message);
        let Some((receipt, version)) = self.load::<Receipt>(&id, true).await? else {
            return Ok(None);
        };
        if version != 1
            || receipt.id() != id
            || !receipt.valid(authority, channel, message, (self.clock)() / 1000)
        {
            return Err(GatewayError::Authority);
        }
        let plan =
            crate::ingress::routing_store::PlanStore::new(self.channels.clone(), self.keys.clone())
                .load(&receipt.original)
                .await
                .map_err(|_| GatewayError::Unavailable)?
                .ok_or(GatewayError::Authority)?;
        if plan.activity != receipt.activity
            || plan
                .targets
                .iter()
                .find(|target| target.launch.consumer == receipt.consumer)
                .is_none_or(|target| {
                    target.launch.provider != receipt.provider
                        || target.launch.event.partition_key != receipt.partition_key
                        || target.launch.event.conversation_id != receipt.conversation_id
                })
        {
            return Err(GatewayError::Authority);
        }
        Ok(Some(Target {
            activity: receipt.activity,
            original: receipt.original.event_id,
            guild: receipt.guild_id,
            outbound_consumer: Some(receipt.consumer),
            retained_until: receipt.retained_until,
        }))
    }
}
