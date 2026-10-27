//! Native form launch is not provider work; only a valid whole submission is.
use super::{
    components::{parse, Snapshot},
    journal::DurableJournal,
    journal_model::{Authority, Inbox},
    normalize::snowflake,
    Dispatch, GatewayError,
};
use crate::{
    ingress::admission::{admit, AdmissionFacts},
    interactions::{discord_form, discord_form_store::Callback, Error},
};
use cmaeg_core::{
    activity::{ActivityKind, ConversationKind},
    channel::Source,
};
use serde_json::{json, Value};
use std::time::Duration;

impl DurableJournal {
    pub(super) async fn persist_modal(
        &self,
        authority: &Authority,
        event: &Dispatch,
    ) -> Result<(), GatewayError> {
        let Some(runtime) = &self.api.components else {
            return Ok(());
        };
        let raw = &event.data;
        let Some(custom) = raw["data"]["custom_id"].as_str() else {
            return Ok(());
        };
        let submit = raw["type"] == 5;
        let prefix = if submit { "cdm1" } else { "cdf1" };
        let (card_id, _, _) = match discord_form::decode(custom, prefix) {
            Ok(value) => value,
            Err(_) => return Ok(()),
        };
        let id = match snowflake(&raw["id"]) {
            Ok(id) => id,
            Err(_) => return Ok(()),
        };
        let inbox_id = crate::connections::store::index_id(
            "discord-native-message-v1",
            &[&authority.record, &authority.bot_id, id],
        );
        if submit {
            if let Some((inbox, 1)) = self.store.load::<Inbox>(&inbox_id, true).await? {
                let bytes = self.verify_inbox(&inbox, authority, id).await?;
                return self.observe_inbox(&inbox, authority, &bytes).await;
            }
        }
        let rejected = crate::connections::store::index_id(
            "discord-modal-rejected-v1",
            &[&authority.record, &authority.bot_id, id],
        );
        let Some(store) = &runtime.ingress.interactions else {
            return Ok(());
        };
        let retained = match store.load(&card_id).await {
            Ok((card, _)) => card,
            Err(Error::Denied) => return Ok(()),
            Err(_) => return Err(GatewayError::Unavailable),
        };
        let recovery = submit && retained.consumed.is_some();
        // Accepted custody wins over an old or concurrently written refusal.
        // The store still requires the exact consumed identity and field set;
        // a rejection only blocks changing a previously unaccepted operation.
        if !recovery && self.store.load::<Value>(&rejected, true).await?.is_some() {
            return Ok(());
        }
        let created = (id.parse::<u64>().map_err(|_| GatewayError::Protocol)? >> 22) as i64
            + 1_420_070_400_000;
        let elapsed = (self.store.clock)().saturating_sub(created);
        if elapsed < 0 {
            return Ok(());
        }
        let deadline = tokio::time::Instant::now()
            + Duration::from_millis(2500_i64.saturating_sub(elapsed).max(0) as u64);
        if elapsed >= 2500 && !recovery {
            return Ok(());
        }
        let work = async {
            let current = super::runtime_source::resolve(
                &self.store.channels,
                &self.store.keys,
                &runtime.ingress.resolver,
                &authority.record,
            )
            .await?;
            if current.revision != authority.revision {
                return Err(GatewayError::Authority);
            }
            let mut normalized = raw.clone();
            if submit {
                // Component-origin modal may omit message; its retained launch
                // supplies the confirmed original destination, never client IDs.
                if raw.get("message").is_none() {
                    let destination = retained
                        .destination
                        .as_ref()
                        .ok_or(GatewayError::Authority)?;
                    normalized["message"] = json!({"id":destination.platform_id,"channel_id":retained.chat_id(),"author":{"id":authority.bot_id,"bot":true}});
                }
                normalized["data"] = json!({"custom_id":custom,"component_type":2});
            }
            let (mut activity, values) = match parse(&normalized, &authority.bot_id, created) {
                Ok(value) => value,
                Err(_) => return Ok(()),
            };
            if !values.is_empty() || (!submit && raw["data"]["component_type"] != 2) {
                return Ok(());
            }
            let Some(Source::Discord { admission, .. }) =
                current.channel.definition.source_of(&activity.source)
            else {
                return Err(GatewayError::Authority);
            };
            if admit(
                admission,
                &AdmissionFacts {
                    receiving_bot_id: Some(&authority.bot_id),
                    mentioned: Some(true),
                    sender_id: &activity.actor.id,
                    chat_id: &activity.conversation.id,
                    is_group: activity.conversation.kind == ConversationKind::Group,
                    is_bot_sender: false,
                    is_system: false,
                    addressed: Some(true),
                },
            )
            .is_err()
            {
                return Ok(());
            }
            let thread = match self
                .api
                .component_thread(
                    &current.channel,
                    store,
                    &mut activity,
                    &normalized,
                    deadline,
                )
                .await
            {
                Ok(thread) => thread,
                Err(Error::Denied) => return Ok(()),
                Err(_) => return Err(GatewayError::Unavailable),
            };
            let message = snowflake(&normalized["message"]["id"])?;
            let callback = Callback {
                interaction: id,
                custom,
                message,
                guild: raw["guild_id"].as_str(),
                thread: thread.as_ref(),
            };
            if !submit {
                match store
                    .launch_discord_form(&current.channel, &activity, &callback, deadline)
                    .await
                {
                    Ok(_) => {}
                    Err(Error::Denied) => return Ok(()),
                    Err(_) => return Err(GatewayError::Unavailable),
                }
                self.current_authority().await?;
                self.fenced().await?;
                let card = match store
                    .claim_discord_modal(&current.channel, &activity, &callback, deadline)
                    .await
                {
                    Ok(Some(card)) => card,
                    Ok(None) | Err(Error::Denied) => return Ok(()),
                    Err(_) => return Err(GatewayError::Unavailable),
                };
                let body = discord_form::modal(
                    &card,
                    card.discord_form_launch
                        .as_ref()
                        .ok_or(GatewayError::Authority)?,
                )
                .map_err(|_| GatewayError::Protocol)?;
                self.current_authority().await?;
                self.fenced().await?;
                self.modal_response(id, raw, &body, deadline).await;
                return Ok(());
            }
            let accepted = match store
                .submit_discord_form(
                    &current.channel,
                    &activity,
                    &callback,
                    &raw["data"]["components"],
                    deadline,
                )
                .await
            {
                Ok(accepted) => accepted,
                Err(Error::Denied) => return Ok(()),
                Err(_) => return Err(GatewayError::Unavailable),
            };
            let Some((result, card)) = accepted else {
                if tokio::time::Instant::now() >= deadline {
                    return Ok(());
                }
                // Persist refusal before ACK so a changed replay cannot become
                // a valid submission under the same interaction identity.
                if self
                    .store
                    .put(
                        &rejected,
                        &authority.owner,
                        &json!({"kind":"discord_modal_rejected_v1","interaction":id}),
                        None,
                        Some(created / 1000 + super::journal_model::RETENTION_SECONDS),
                    )
                    .await
                    != Ok(1)
                {
                    return Ok(());
                }
                return self.modal_ack(authority, id, created, raw, &json!({"type":4,"data":{"content":"Please check the form values and open the form again to retry.","flags":64,"allowed_mentions":{"parse":[]}}}), deadline).await;
            };
            activity.occurred_at = card.consumed_at().ok_or(GatewayError::Authority)?.into();
            let identity = card.discord.ok_or(GatewayError::Authority)?;
            activity.kind = ActivityKind::Interaction(result);
            activity.routing = Some(identity.routing.clone());
            activity.validate().map_err(|_| GatewayError::Protocol)?;
            let snapshot = Snapshot {
                activity,
                identity,
                message_id: message.into(),
            };
            let mut safe = raw.clone();
            safe.as_object_mut()
                .ok_or(GatewayError::Protocol)?
                .remove("token");
            self.persist_inbox(
                authority,
                &Dispatch {
                    name: event.name.clone(),
                    sequence: event.sequence,
                    data: safe,
                },
                Some(&snapshot),
            )
            .await?;
            self.modal_ack(authority, id, created, raw, &json!({"type":6}), deadline)
                .await
        };
        if recovery {
            work.await
        } else {
            // A cancelled storage write may have committed. The next replay
            // reads retained authority; no timeout authorizes a second response.
            tokio::time::timeout_at(deadline, work)
                .await
                .unwrap_or(Err(GatewayError::Unavailable))
        }
    }
    async fn modal_response(
        &self,
        id: &str,
        raw: &Value,
        body: &Value,
        deadline: tokio::time::Instant,
    ) {
        if let Some(token) = raw["token"].as_str() {
            if self
                .api
                .acknowledge_interaction(
                    id,
                    token,
                    body,
                    deadline.saturating_duration_since(tokio::time::Instant::now()),
                )
                .await
                .is_err()
            {
                tracing::warn!(
                    "native Discord modal response unavailable; retained authority preserved"
                );
            }
        }
    }
    #[allow(clippy::too_many_arguments)]
    async fn modal_ack(
        &self,
        authority: &Authority,
        id: &str,
        created: i64,
        raw: &Value,
        body: &Value,
        deadline: tokio::time::Instant,
    ) -> Result<(), GatewayError> {
        self.current_authority().await?;
        self.fenced().await?;
        if tokio::time::Instant::now() >= deadline {
            return Ok(());
        }
        let claim = crate::connections::store::index_id(
            "discord-component-ack-v1",
            &[&authority.record, &authority.bot_id, id],
        );
        if self
            .store
            .put(
                &claim,
                &authority.owner,
                &json!({"kind":"discord_component_ack_v1","interaction":id}),
                None,
                Some(created / 1000 + super::journal_model::RETENTION_SECONDS),
            )
            .await
            != Ok(1)
        {
            return Ok(());
        }
        self.current_authority().await?;
        self.fenced().await?;
        self.modal_response(id, raw, body, deadline).await;
        Ok(())
    }
}
