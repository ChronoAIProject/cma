//! Delivery to a channel's default reply destination (decision D-e): a chat
//! of its native Telegram bot, a NyxID relay conversation through the
//! proactive send, or its result webhook sinks.
//!
//! Each transport is today's machinery on today's engine (Amendment R6
//! §R6.1): every chat part waits on the shared route pacer, a 429 takes
//! today's single cooldown retry, and sinks are posted with today's webhook
//! client and retries. The destination is resolved per send from the
//! snapshot the command carries and is never sealed.
use super::{
    address::{EffectiveAddress, DEFAULT_REPLY_UNAVAILABLE, INTERACTIVE_DESTINATION_UNSUPPORTED},
    gate::expired,
    intents::{DeliveryOutcome, IntentReceipt},
    part_keys::PartKeys,
    relay_client::{unknown, DEFAULT_REPLY_UNSUPPORTED},
    render::{custom, RelayText},
    send::{refused, SendContext, Sender},
    telegram::NativeChat,
};
use crate::{
    channels::resolver::ResolvedChannel,
    telemetry::{channel_hash, names},
    tools::Capabilities,
};
use cmaeg_core::{
    channel::Platform,
    ids::MessageRef,
    message::{OutboundMessage, RenderingReport},
    seal::{EventRefPayload, MessageRefPayload},
};

/// A part NyxID rate-limited twice. Its limiter runs before any delivery
/// work, so nothing of that part was sent.
pub(crate) const DEFAULT_REPLY_RATE_LIMITED: &str = "default_reply_rate_limited";

/// The `transport` label of a default destination's metric and logs.
fn transport(address: &EffectiveAddress) -> Option<&'static str> {
    match address {
        EffectiveAddress::DefaultTelegramChat { .. } => Some("telegram"),
        EffectiveAddress::DefaultRelayConversation { .. } => Some("nyxid_relay"),
        EffectiveAddress::DefaultSinks => Some("webhook"),
        EffectiveAddress::Origin(_) | EffectiveAddress::Rejected { .. } => None,
    }
}

impl Sender {
    /// Send `message` for `event` to the default destination `address`.
    /// `keys` derive a relay send's per-part idempotency keys. A message
    /// with interactive controls is refused: cards bind actor and chat at
    /// the origin (G33), so none can be sent anywhere else. Typing and
    /// message updates never come here; they stay at the origin.
    pub(super) async fn default_reply(
        &self,
        ctx: &SendContext<'_>,
        event: &EventRefPayload,
        address: &EffectiveAddress,
        message: &OutboundMessage,
        keys: Option<&PartKeys>,
    ) -> IntentReceipt {
        let Some(transport) = transport(address) else {
            tracing::error!("a send to the origin reached the default transports");
            return refused(custom::report(message), unavailable());
        };
        let receipt = if crate::interactions::has_controls(message) {
            let rejected = DeliveryOutcome::Rejected {
                reason: INTERACTIVE_DESTINATION_UNSUPPORTED.into(),
            };
            refused(rendering(address, message, ctx.label), rejected)
        } else {
            match address {
                EffectiveAddress::DefaultTelegramChat {
                    bot_id,
                    chat_id,
                    thread_id,
                } => {
                    let chat = NativeChat {
                        bot_id,
                        chat_id,
                        thread_id: thread_id.as_deref(),
                    };
                    self.native_send(ctx, event, chat, message).await.0
                }
                EffectiveAddress::DefaultRelayConversation { platform, route_id } => {
                    self.initiated_reply(ctx, event, *platform, route_id, message, keys)
                        .await
                }
                _ => {
                    self.custom
                        .default_reply(ctx.gate, ctx.channel, event, message, ctx.consumer.clone())
                        .await
                }
            }
        };
        record(ctx.channel, transport, &receipt.delivery);
        receipt
    }

    /// Send `message` to the relay conversation `route_id` through NyxID's
    /// proactive send, one idempotency key per part, sharing the per-route
    /// pacer with relay replies so the route stays below NyxID's initiate
    /// limit of one per second.
    async fn initiated_reply(
        &self,
        ctx: &SendContext<'_>,
        event: &EventRefPayload,
        platform: Platform,
        route_id: &str,
        message: &OutboundMessage,
        keys: Option<&PartKeys>,
    ) -> IntentReceipt {
        let mut capabilities = Capabilities::relay(platform);
        capabilities.buttons = "numbered_text".into();
        let (parts, rendering) = RelayText.render_labeled(message, &capabilities, ctx.label);
        let mut result = IntentReceipt {
            message_ref: None,
            rendering,
            delivery: DeliveryOutcome::Rejected {
                reason: "empty_message".into(),
            },
        };
        let (Some(key), Some(keys)) = (ctx.channel.definition.secrets.channel_key.as_ref(), keys)
        else {
            // The address resolves only with a channel key, and every caller
            // passes keys for a relay destination.
            tracing::error!("default relay send without a channel key or part keys");
            result.delivery = unavailable();
            return result;
        };
        let _ticket = match ctx.gate.enter_route(route_id).await {
            Ok(ticket) => ticket,
            Err(refusal) => {
                result.delivery = refusal.outcome();
                return result;
            }
        };
        let mut confirmed_parts = 0;
        let mut first_platform_id = None;
        for (index, part) in parts.iter().enumerate() {
            let Some(mut slot) = self.pacing.acquire(route_id).await else {
                result.delivery = DeliveryOutcome::Rejected {
                    reason: "pacing_capacity".into(),
                };
                break;
            };
            let permit = match ctx.gate.attempt().await {
                Ok(permit) => permit,
                Err(refusal) => {
                    result.delivery = refusal.outcome();
                    break;
                }
            };
            if index == 0 && !ctx.gate.commit() {
                result.delivery = expired();
                break;
            }
            let idempotency_key = keys.key(index, &part.text);
            let mut outcome = self
                .relay
                .send_initiated(route_id, key, &part.text, &idempotency_key)
                .await;
            drop(permit);
            if let DeliveryOutcome::RateLimited { retry_after } = outcome {
                // Today's single cooldown retry, inside the worker, with the
                // part's own key.
                slot.cooldown(retry_after);
                slot.retry().await;
                let retry = match ctx.gate.attempt().await {
                    Ok(permit) => permit,
                    Err(refusal) => {
                        result.delivery = refusal.outcome();
                        break;
                    }
                };
                outcome = self
                    .relay
                    .send_initiated(route_id, key, &part.text, &idempotency_key)
                    .await;
                drop(retry);
                if let DeliveryOutcome::RateLimited { retry_after } = outcome {
                    slot.cooldown(retry_after);
                    outcome = DeliveryOutcome::Rejected {
                        reason: DEFAULT_REPLY_RATE_LIMITED.into(),
                    };
                }
            }
            ctx.gate.progress();
            tracing::info!(
                outcome = outcome.kind(),
                part = index,
                "default relay delivery finished"
            );
            if let DeliveryOutcome::Confirmed {
                platform_message_id: Some(id),
            } = &outcome
            {
                if result.message_ref.is_none() {
                    first_platform_id = Some(id.clone());
                    result.message_ref = MessageRef::seal(
                        &MessageRefPayload {
                            channel_id: event.channel_id.clone(),
                            event_id: event.event_id.clone(),
                            platform_message_id: id.clone(),
                            route_id: route_id.into(),
                            interaction_record_id: None,
                            consumer: event.consumer.clone(),
                        },
                        &self.keys,
                    )
                    .ok();
                }
            }
            let confirmed = matches!(outcome, DeliveryOutcome::Confirmed { .. });
            result.delivery = outcome;
            if !confirmed {
                break;
            }
            confirmed_parts += 1;
        }
        if confirmed_parts > 0 {
            result.delivery = if confirmed_parts == parts.len() {
                DeliveryOutcome::Confirmed {
                    platform_message_id: first_platform_id,
                }
            } else {
                unknown("partial_delivery")
            };
        }
        result
    }
}

/// The rendering report of a message refused before any send.
fn rendering(
    address: &EffectiveAddress,
    message: &OutboundMessage,
    label: Option<&str>,
) -> RenderingReport {
    let capabilities = match address {
        EffectiveAddress::DefaultTelegramChat { .. } => Capabilities::native_telegram(),
        EffectiveAddress::DefaultRelayConversation { platform, .. } => {
            Capabilities::relay(*platform)
        }
        _ => return custom::report(message),
    };
    RelayText.render_labeled(message, &capabilities, label).1
}

fn unavailable() -> DeliveryOutcome {
    DeliveryOutcome::Rejected {
        reason: DEFAULT_REPLY_UNAVAILABLE.into(),
    }
}

/// The `outcome` label of a default send's delivery; `None` for an unknown
/// outcome, which is logged but neither confirmed nor refused.
fn outcome_label(delivery: &DeliveryOutcome) -> Option<&'static str> {
    match delivery {
        DeliveryOutcome::Confirmed { .. } => Some("confirmed"),
        DeliveryOutcome::RateLimited { .. } => Some("rate_limited"),
        DeliveryOutcome::Unsupported => Some("unsupported"),
        DeliveryOutcome::Rejected { reason } => Some(match reason.as_str() {
            DEFAULT_REPLY_RATE_LIMITED => "rate_limited",
            DEFAULT_REPLY_UNSUPPORTED => "unsupported",
            DEFAULT_REPLY_UNAVAILABLE => "unavailable",
            _ => "refused",
        }),
        DeliveryOutcome::Unknown { .. } => None,
    }
}

/// Count a default send in `cmaeg_default_reply_total` and log any refusal
/// with the channel hash, the transport and the reason; never a chat id,
/// route id, credential or text.
pub(super) fn record(
    channel: &ResolvedChannel,
    transport: &'static str,
    delivery: &DeliveryOutcome,
) {
    let channel = channel_hash(channel.id.as_str());
    match delivery {
        DeliveryOutcome::Confirmed { .. } => {}
        DeliveryOutcome::Unknown { reason } => {
            tracing::warn!(%channel, transport, reason, "default reply unconfirmed")
        }
        DeliveryOutcome::Rejected { reason } => {
            tracing::warn!(%channel, transport, reason, "default reply refused")
        }
        other => tracing::warn!(
            %channel,
            transport,
            reason = other.kind(),
            "default reply refused"
        ),
    }
    if let Some(outcome) = outcome_label(delivery) {
        metrics::counter!(names::DEFAULT_REPLY, names::TRANSPORT => transport,
            names::OUTCOME => outcome)
        .increment(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::delivery::relay_client::DEFAULT_REPLY_REFUSED;

    #[test]
    fn outcomes_map_onto_the_metric_labels() {
        let rejected = |reason: &str| DeliveryOutcome::Rejected {
            reason: reason.into(),
        };
        let cases = [
            (
                DeliveryOutcome::Confirmed {
                    platform_message_id: None,
                },
                Some("confirmed"),
            ),
            (rejected(DEFAULT_REPLY_REFUSED), Some("refused")),
            (rejected("telegram_rejected"), Some("refused")),
            (
                rejected(INTERACTIVE_DESTINATION_UNSUPPORTED),
                Some("refused"),
            ),
            (rejected(DEFAULT_REPLY_UNSUPPORTED), Some("unsupported")),
            (DeliveryOutcome::Unsupported, Some("unsupported")),
            (rejected(DEFAULT_REPLY_RATE_LIMITED), Some("rate_limited")),
            (
                DeliveryOutcome::RateLimited { retry_after: 3 },
                Some("rate_limited"),
            ),
            (rejected(DEFAULT_REPLY_UNAVAILABLE), Some("unavailable")),
            (unknown("partial_delivery"), None),
        ];
        for (delivery, label) in cases {
            assert_eq!(outcome_label(&delivery), label, "{delivery:?}");
        }
    }

    fn channel() -> ResolvedChannel {
        let definition: cmaeg_core::channel::ChannelDefinition = serde_json::from_str(
            include_str!("../../../cmaeg-core/tests/fixtures/channel.json"),
        )
        .unwrap();
        ResolvedChannel {
            id: cmaeg_core::ids::ChannelId::new("cmaeg1.ch.default-fixture").unwrap(),
            pointer: cmaeg_core::seal::ChannelPointer {
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

    #[test]
    fn default_sends_are_counted_by_transport_and_outcome() {
        let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        let channel = channel();
        let rejected = |reason: &str| DeliveryOutcome::Rejected {
            reason: reason.into(),
        };
        metrics::with_local_recorder(&recorder, || {
            let confirmed = DeliveryOutcome::Confirmed {
                platform_message_id: None,
            };
            record(&channel, "telegram", &confirmed);
            record(&channel, "telegram", &confirmed);
            record(&channel, "nyxid_relay", &rejected(DEFAULT_REPLY_REFUSED));
            record(
                &channel,
                "nyxid_relay",
                &rejected(DEFAULT_REPLY_RATE_LIMITED),
            );
            record(
                &channel,
                "nyxid_relay",
                &rejected(DEFAULT_REPLY_UNSUPPORTED),
            );
            record(&channel, "webhook", &unknown("partial_sink_delivery"));
        });
        let rendered = handle.render();
        for line in [
            r#"cmaeg_default_reply_total{transport="telegram",outcome="confirmed"} 2"#,
            r#"cmaeg_default_reply_total{transport="nyxid_relay",outcome="refused"} 1"#,
            r#"cmaeg_default_reply_total{transport="nyxid_relay",outcome="rate_limited"} 1"#,
            r#"cmaeg_default_reply_total{transport="nyxid_relay",outcome="unsupported"} 1"#,
        ] {
            assert!(rendered.contains(line), "{rendered}");
        }
        assert!(!rendered.contains("webhook"), "{rendered}");
    }

    #[test]
    fn only_default_destinations_name_a_transport() {
        assert_eq!(transport(&EffectiveAddress::DefaultSinks), Some("webhook"));
        assert_eq!(
            transport(&EffectiveAddress::Origin(
                cmaeg_core::seal::ReplyAddress::None
            )),
            None
        );
        assert_eq!(
            transport(&EffectiveAddress::Rejected {
                reason: DEFAULT_REPLY_UNAVAILABLE
            }),
            None
        );
    }
}
