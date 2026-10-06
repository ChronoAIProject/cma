//! Durable completion notification to the initiating provider conversation.
use super::{model::*, ConnectionService, Error};
use crate::ingress::launch::{LaunchOutcome, LaunchRequest, TurnLauncher};
use cmaeg_core::{
    activity::{Activity, ActivityKind, Actor, ActorKind},
    cloudevent::CloudEvent,
    ids::{EventId, IdempotencyKey},
    seal::{self, EventRef},
};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

pub struct CompletionWorker(tokio::task::JoinHandle<()>);
impl Drop for CompletionWorker {
    fn drop(&mut self) {
        self.0.abort();
    }
}
impl ConnectionService {
    pub fn start_completion_worker(&self, launcher: Arc<dyn TurnLauncher>) -> CompletionWorker {
        let service = self.clone();
        CompletionWorker(tokio::spawn(async move {
            let mut cursor = None;
            let mut ticker = tokio::time::interval(Duration::from_secs(5));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                ticker.tick().await;
                let heads = match service.store.page(None, cursor.as_deref()).await {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                cursor = if heads.len() == 100 {
                    heads.last().map(|h| h.record_id.clone())
                } else {
                    None
                };
                for head in heads {
                    let (mut record, mut version) =
                        match service.store.connection(&head.record_id).await {
                            Ok(v) => v,
                            Err(_) => continue,
                        };
                    if record.channel_id.is_some() || record.webhook_retiring {
                        let _ = service
                            .recover_deleted_channel(record.clone(), version)
                            .await;
                        match service.store.connection(&head.record_id).await {
                            Ok((current, revision)) => {
                                record = current;
                                version = revision;
                            }
                            Err(_) => continue,
                        }
                    }
                    // Cancellation and claim acquisition are separate CAS
                    // records. Recover a late claim even after either replica
                    // dies or loses the final save response.
                    if matches!(
                        super::service::effective_status(&record),
                        Status::Cancelled | Status::Expired
                    ) {
                        let _ = service.reconcile_terminal_claim(&record).await;
                        continue;
                    }
                    if record.status == Status::AwaitingPlatform
                        && record.requested_manager
                        && record.material.is_some()
                        && record.expires_at > now()
                    {
                        if let Ok((next, next_version)) =
                            service.refresh_management(record.clone(), version).await
                        {
                            record = next;
                            version = next_version;
                        }
                    }
                    if record.status != Status::Ready
                        || record.notified
                        || record.origin.event.expires_at <= now()
                    {
                        continue;
                    }
                    if record.completion_ref.is_none() {
                        let request = match service.completion_request(&record).await {
                            Ok(v) => v,
                            Err(_) => continue,
                        };
                        record.completion_ref = Some(request.event_ref.as_str().into());
                        version = match service.store.save_connection(&record, Some(version)).await
                        {
                            Ok(v) => v,
                            Err(_) => continue,
                        };
                    }
                    let result = tokio::time::timeout(
                        Duration::from_secs(10),
                        service.notify_completion(&record, launcher.as_ref()),
                    )
                    .await;
                    if matches!(result, Ok(Ok(()))) {
                        record.notified = true;
                        let _ = service.store.save_connection(&record, Some(version)).await;
                    }
                }
            }
        }))
    }
    pub async fn refresh_management(
        &self,
        record: Connection,
        version: u64,
    ) -> Result<(Connection, u64), Error> {
        if record.status != Status::AwaitingPlatform
            || !record.requested_manager
            || record.expires_at <= now()
        {
            return Ok((record, version));
        }
        let material = record.material.as_ref().ok_or(Error::Conflict)?;
        let token = cmaeg_core::channel::Secret::new(material.token.clone());
        let bot = self
            .telegram()?
            .get_me(&token)
            .await
            .map_err(|_| Error::Unavailable)?;
        if bot.id != material.bot_id {
            return Err(Error::Conflict);
        }
        if !bot.can_manage_bots {
            return Ok((record, version));
        }
        let id = record.id.clone();
        self.complete(record, version, bot, token).await?;
        self.store.connection(&id).await
    }
    pub(crate) async fn completion_request(
        &self,
        record: &Connection,
    ) -> Result<LaunchRequest, Error> {
        let channel = self.origin_channel(record).await?;
        let event_id = EventId::new(format!("connection:{}:ready", record.id))
            .map_err(|_| Error::Unavailable)?;
        let occurred_at = time::OffsetDateTime::from_unix_timestamp(record.created_at)
            .map_err(|_| Error::Unavailable)?
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(|_| Error::Unavailable)?;
        let activity = Activity {
            schema_version: 1,
            event_id: event_id.clone(),
            actor: Actor {
                id: record.origin.actor.clone(),
                display_name: None,
                kind: ActorKind::System,
            },
            conversation: record.origin.conversation.clone(),
            source: record.origin.source.clone(),
            occurred_at: occurred_at.clone(),
            kind: ActivityKind::Event {
                ce: CloudEvent {
                    specversion: "1.0".into(),
                    id: event_id.to_string(),
                    source: self.management.public_url.clone(),
                    r#type: "gateway.connection.ready".into(),
                    subject: Some(record.id.clone()),
                    time: Some(occurred_at),
                    datacontenttype: Some("application/json".into()),
                    dataschema: None,
                    data: Some(serde_json::json!({"connection_ref":record.id,"status":"ready"})),
                    extensions: Default::default(),
                },
            },
        };
        activity.validate().map_err(|_| Error::Unavailable)?;
        if activity.partition_key(&channel.definition.partition)
            != record.origin.event.partition_key
        {
            return Err(Error::Conflict);
        }
        let mut event = record.origin.event.clone();
        event.event_id = event_id.clone();
        event.activity_sha256 = Some(seal::event_ref::activity_digest(&activity));
        let reference = match &record.completion_ref {
            Some(reference) => reference
                .parse::<EventRef>()
                .map_err(|_| Error::Unavailable)?,
            None => EventRef::seal(&event, &self.management.master_keys)
                .map_err(|_| Error::Unavailable)?,
        };
        let verified = reference
            .open(&self.management.master_keys, now())
            .map_err(|_| Error::NotFound)?
            .payload;
        if verified != event {
            return Err(Error::Conflict);
        }
        Ok(LaunchRequest {
            channel,
            activity,
            conversation_id: event.conversation_id,
            event_ref: reference,
            idempotency_key: IdempotencyKey::from_event(&event_id),
            received_at: Instant::now(),
            trace: tracing::Span::current(),
            timing: None,
        })
    }
    pub async fn notify_completion(
        &self,
        record: &Connection,
        launcher: &dyn TurnLauncher,
    ) -> Result<(), Error> {
        if record.completion_ref.is_none() {
            return Err(Error::Conflict);
        }
        match launcher
            .launch(self.completion_request(record).await?)
            .await
        {
            LaunchOutcome::Accepted { .. } => Ok(()),
            LaunchOutcome::Deferred | LaunchOutcome::Refused(_) => Err(Error::Unavailable),
        }
    }
}
