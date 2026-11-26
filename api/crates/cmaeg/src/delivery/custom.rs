use super::{
    gate::{expired, SendGate, NO_CHAT_ROUTE},
    intents::{DeliveryOutcome, IntentReceipt},
    render::custom,
    sync::SyncRegistry,
    webhook_client::WebhookClient,
};
use crate::{channels::resolver::ResolvedChannel, turns::outcomes::TurnOutcome};
use cmaeg_core::{
    channel::Sink,
    message::OutboundMessage,
    problem::{Problem, ProblemCode},
    seal::{EventRefPayload, ReplyAddress},
};
use serde_json::{json, Value};
use std::sync::Arc;
#[derive(Clone)]
pub struct CustomDelivery {
    pub public_origin: String,
    pub waiters: Arc<SyncRegistry>,
    pub webhooks: WebhookClient,
    pub activity: Arc<dyn crate::turns::observer::ActivityObserver>,
}
impl CustomDelivery {
    pub fn new(public_origin: String, waiters: Arc<SyncRegistry>) -> Self {
        Self {
            public_origin,
            waiters,
            webhooks: WebhookClient::default(),
            activity: Arc::new(crate::turns::observer::NoopObserver),
        }
    }
    pub fn with_activity(
        mut self,
        activity: Arc<dyn crate::turns::observer::ActivityObserver>,
    ) -> Self {
        self.activity = activity;
        self
    }
    pub async fn reply(
        &self,
        gate: &dyn SendGate,
        channel: &ResolvedChannel,
        event: &EventRefPayload,
        message: &OutboundMessage,
    ) -> IntentReceipt {
        if message.structured_content.is_some()
            || message.native_media_update.is_some()
            || message.clear_native_attachments
        {
            return super::send::unsupported();
        }
        let data = custom::data(event, message.text.as_deref(), Some(message), None);
        // Commit point (Amendment R3 §R3.1): for a Sync-origin event the
        // emitted frame is the first side effect. Emitting to any other
        // address does nothing, so its first webhook attempt commits.
        if matches!(event.reply, ReplyAddress::Sync { .. }) && !gate.commit() {
            return IntentReceipt {
                message_parts: vec![],
                message_ref: None,
                rendering: custom::report(message),
                delivery: expired(),
            };
        }
        let streamed = self.emit(event, "cmaeg.message", &data, false);
        self.observe_local(event, streamed).await;
        let delivery = self
            .webhook(
                gate,
                channel,
                event,
                ("cmaeg.response.message", data),
                streamed,
                None,
            )
            .await;
        IntentReceipt {
            message_parts: vec![],
            message_ref: None,
            rendering: custom::report(message),
            delivery,
        }
    }
    pub fn stream(&self, event: &EventRefPayload, outcome: &TurnOutcome) -> bool {
        let frame = match outcome {
            TurnOutcome::TextDelta { text } => Some((
                "cmaeg.text.delta",
                json!({"event_id":event.event_id,"conversation_id":event.conversation_id,"text":text}),
            )),
            TurnOutcome::MessageCommitted { text } => Some((
                "cmaeg.message",
                custom::data(
                    event,
                    Some(text),
                    Some(&OutboundMessage {
                        whatsapp_template: None,
                        structured_content: None,
                        reply_with_received_sticker: false,
                        clear_native_attachments: false,
                        native_media_update: None,
                        native_attachments: Vec::new(),
                        thread_reply: false,
                        text: Some(text.clone()),
                        blocks: vec![],
                        reply_to: None,
                    }),
                    None,
                ),
            )),
            _ => terminal(event, outcome).map(|(_, data)| {
                (
                    if matches!(outcome, TurnOutcome::Completed { .. }) {
                        "cmaeg.completed"
                    } else {
                        "cmaeg.failed"
                    },
                    data,
                )
            }),
        };
        if let Some((name, data)) = frame {
            self.emit(event, name, &data, outcome.terminal())
        } else {
            false
        }
    }
    /// Deliver a terminal outcome to the result webhooks; `None` when the
    /// outcome is not terminal and nothing was sent.
    #[tracing::instrument(name="cmaeg.deliver", skip_all, fields(
        channel = %crate::telemetry::channel_hash(channel.id.as_str()), event_id = %event.event_id))]
    pub async fn outcome(
        &self,
        gate: &dyn SendGate,
        channel: &ResolvedChannel,
        event: &EventRefPayload,
        outcome: &TurnOutcome,
        answer_response_id: Option<&str>,
    ) -> Option<DeliveryOutcome> {
        let body = terminal(event, outcome)?;
        let answer_response_id = answer_response_id.filter(|_| {
            matches!(outcome, TurnOutcome::Completed { final_text: Some(text) } if !text.trim().is_empty())
        });
        let mut observation = crate::telemetry::operations::Operation::new(
            crate::telemetry::names::DELIVERY_DURATION,
            crate::telemetry::names::PLATFORM,
            "custom",
        );
        let delivered = self
            .webhook(gate, channel, event, body, false, answer_response_id)
            .await;
        observation.finish(delivered.kind());
        tracing::info!(
            outcome = delivered.kind(),
            "custom terminal delivery finished"
        );
        Some(delivered)
    }
    fn emit(
        &self,
        event: &EventRefPayload,
        name: &'static str,
        data: &Value,
        terminal: bool,
    ) -> bool {
        if let ReplyAddress::Sync { waiter } = &event.reply {
            self.waiters.emit(waiter, name, data, terminal)
        } else {
            false
        }
    }
    async fn webhook(
        &self,
        gate: &dyn SendGate,
        channel: &ResolvedChannel,
        event: &EventRefPayload,
        (kind, data): (&str, Value),
        streamed: bool,
        answer_response_id: Option<&str>,
    ) -> DeliveryOutcome {
        let envelope = custom::result(&self.public_origin, event, kind, data);
        let mut result = if streamed {
            DeliveryOutcome::Confirmed {
                platform_message_id: None,
            }
        } else {
            DeliveryOutcome::Unsupported
        };
        let mut attempted = streamed;
        let mut ticket = None;
        for sink in &channel.definition.reply.sinks {
            let Sink::ResultWebhook { url, hmac_key_id } = sink else {
                continue;
            };
            if ticket.is_none() {
                match gate.enter_route(NO_CHAT_ROUTE).await {
                    Ok(admitted) => ticket = Some(admitted),
                    Err(refusal) => return refusal.outcome(),
                }
            }
            let outcome = match channel
                .definition
                .secrets
                .hmac_secrets
                .iter()
                .find(|(id, _)| id == hmac_key_id)
            {
                Some((_, key)) => self.webhooks.send_gated(url, key, &envelope, gate).await,
                None => DeliveryOutcome::Rejected {
                    reason: "webhook_key_missing".into(),
                },
            };
            let step = if kind == "cmaeg.response.failed" {
                crate::turns::observer::DeliveryStep::Failure
            } else {
                crate::turns::observer::DeliveryStep::Answer
            };
            crate::telemetry::delivery::webhook(step, &outcome);
            self.activity
                .checkpoint(
                    &event.record_id,
                    &event.event_id,
                    crate::activity::facts::delivery(
                        event.consumer.as_ref().map_or("default", |id| id.as_str()),
                        step,
                        &outcome,
                        crate::turns::observer::Destination::Origin,
                        &ReplyAddress::ResultWebhook {
                            url: url.clone(),
                            hmac_key_id: String::new(),
                        },
                        None,
                    )
                    .with_answer_response(answer_response_id),
                )
                .await;
            result = if attempted
                && (!matches!(result, DeliveryOutcome::Confirmed { .. })
                    || !matches!(outcome, DeliveryOutcome::Confirmed { .. }))
            {
                DeliveryOutcome::Unknown {
                    reason: "partial_sink_delivery".into(),
                }
            } else {
                outcome
            };
            attempted = true;
        }
        result
    }
    pub(crate) async fn observe_local(&self, event: &EventRefPayload, buffered: bool) {
        if !matches!(event.reply, ReplyAddress::Sync { .. }) {
            return;
        }
        crate::telemetry::delivery::local(buffered);
        let mut change = crate::activity::facts::delivery(
            event.consumer.as_ref().map_or("default", |id| id.as_str()),
            crate::turns::observer::DeliveryStep::Answer,
            &DeliveryOutcome::Unknown {
                reason: "local_stream".into(),
            },
            crate::turns::observer::Destination::Origin,
            &event.reply,
            None,
        );
        if let Some(delivery) = &mut change.delivery {
            delivery.outcome = if buffered { "buffered" } else { "unavailable" }.into();
        }
        self.activity
            .checkpoint(&event.record_id, &event.event_id, change)
            .await;
    }
}
fn terminal(event: &EventRefPayload, outcome: &TurnOutcome) -> Option<(&'static str, Value)> {
    match outcome {
        TurnOutcome::Completed { final_text } => Some((
            "cmaeg.response.completed",
            custom::data(event, final_text.as_deref(), None, None),
        )),
        TurnOutcome::Failed { .. } | TurnOutcome::Incomplete { .. } => Some((
            "cmaeg.response.failed",
            custom::data(
                event,
                None,
                None,
                Some(Problem::new(
                    ProblemCode::ProviderUnavailable,
                    502,
                    "provider turn did not complete",
                )),
            ),
        )),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::delivery::gate::recording::RecordingGate;
    use cmaeg_core::{channel::ChannelDefinition, ids::ChannelId, seal::ChannelPointer};
    use futures_util::{future::BoxFuture, StreamExt};

    pub(super) fn channel(edit: impl FnOnce(&mut Value)) -> ResolvedChannel {
        let mut value: Value = serde_json::from_str(include_str!(
            "../../../cmaeg-core/tests/fixtures/channel.json"
        ))
        .unwrap();
        edit(&mut value);
        let definition: ChannelDefinition = serde_json::from_value(value).unwrap();
        ResolvedChannel {
            id: ChannelId::new("cmaeg1.ch.gate-fixture").unwrap(),
            pointer: ChannelPointer {
                v: 1,
                record_id: definition.record_id.clone(),
                record_url: "https://cma.example/api/v1/gateway/channel-records/record-1".into(),
            },
            definition,
            agent_key_hash: [0; 32],
            consumer_keys: Vec::new(),
            loaded_at: std::time::Instant::now(),
            version: 1,
        }
    }

    pub(super) fn event(channel: &ResolvedChannel, reply: ReplyAddress) -> EventRefPayload {
        EventRefPayload {
            telegram_admission: None,
            v: 1,
            activity_sha256: None,
            channel_id: channel.id.clone(),
            record_id: channel.pointer.record_id.clone(),
            event_id: "msg-1".parse().unwrap(),
            partition_key: "msg-1".into(),
            conversation_id: "conversation-1".parse().unwrap(),
            expires_at: i64::MAX,
            reply,
            consumer: None,
            lineage: None,
        }
    }

    pub(super) fn message() -> OutboundMessage {
        OutboundMessage {
            whatsapp_template: None,
            structured_content: None,
            reply_with_received_sticker: false,
            clear_native_attachments: false,
            native_media_update: None,
            native_attachments: Vec::new(),
            thread_reply: false,
            text: Some("hello".into()),
            blocks: vec![],
            reply_to: None,
        }
    }

    #[tokio::test]
    async fn a_sync_origin_reply_commits_before_its_frame_is_emitted() {
        let channel = channel(|_| {});
        for commits in [false, true] {
            let waiters = Arc::new(SyncRegistry::default());
            let (waiter, frames) = waiters.register().unwrap();
            let delivery = CustomDelivery::new("https://gateway.example".into(), waiters.clone());
            let event = event(
                &channel,
                ReplyAddress::Sync {
                    waiter: waiter.clone(),
                },
            );
            let gate = RecordingGate::new(commits);
            let receipt = delivery.reply(&gate, &channel, &event, &message()).await;
            assert_eq!(gate.calls(), ["commit"]);
            // A sentinel marks the end of whatever the reply emitted.
            assert!(waiters.emit(&waiter, "cmaeg.completed", &json!({"sentinel":true}), true));
            let frames: Vec<_> = frames.stream().collect().await;
            if commits {
                assert_eq!(
                    receipt.delivery,
                    DeliveryOutcome::Confirmed {
                        platform_message_id: None
                    }
                );
                assert_eq!(frames.len(), 2, "the frame follows the commit");
            } else {
                assert_eq!(receipt.delivery, expired());
                assert_eq!(frames.len(), 1, "a refused commit emits nothing");
            }
        }
    }

    /// Accepts every webhook at a public address.
    struct Accepting;
    impl crate::delivery::webhook_client::WebhookTransport for Accepting {
        fn resolve(
            &self,
            _url: reqwest::Url,
        ) -> BoxFuture<'_, Result<Vec<std::net::SocketAddr>, crate::net::exchange::TransportError>>
        {
            Box::pin(async { Ok(vec!["8.8.8.8:443".parse().unwrap()]) })
        }
        fn post(
            &self,
            _url: reqwest::Url,
            _addresses: Vec<std::net::SocketAddr>,
            _headers: reqwest::header::HeaderMap,
            _body: Arc<Vec<u8>>,
        ) -> BoxFuture<'_, Result<u16, crate::net::exchange::TransportError>> {
            Box::pin(async { Ok(200) })
        }
    }

    #[tokio::test]
    async fn a_webhook_only_reply_commits_inside_its_first_attempt() {
        let channel = channel(|value| {
            value["reply"]["sinks"] = json!([{"type":"result_webhook","url":"https://receiver.example/events","hmac_key_id":"result"}]);
            value["secrets"]["hmac_secrets"] = json!([["result", "result-key"]]);
        });
        let mut delivery = CustomDelivery::new("https://gateway.example".into(), Arc::default());
        delivery.webhooks = WebhookClient::with_transport(Arc::new(Accepting));
        let gate = RecordingGate::new(true);
        let receipt = delivery
            .reply(
                &gate,
                &channel,
                &event(&channel, ReplyAddress::None),
                &message(),
            )
            .await;
        assert_eq!(
            receipt.delivery,
            DeliveryOutcome::Confirmed {
                platform_message_id: None
            }
        );
        assert_eq!(
            gate.calls(),
            ["enter_route", "attempt", "commit", "progress"]
        );
    }
}

#[cfg(test)]
#[path = "custom_activity_tests.rs"]
mod activity_tests;
