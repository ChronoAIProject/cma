//! Paired measurements of real provider SSE and relay HTTP transports.
//! This isolates the two-hop gateway workload; it is not a production load test.
mod fixture;

use super::*;
use crate::{
    delivery::{
        intents::DeliveryOutcome,
        relay_client::{Credential, RelayClient},
    },
    telemetry::turn_trace::{scope, testing, BoundOperation, Outcome, Stage},
};
use cmaeg_core::{
    channel::{Platform, Secret},
    ids::{ConversationId, IdempotencyKey, ProviderSlug},
    seal::ReplyAddress,
};
use futures_util::{stream, StreamExt};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};
use tracing::instrument::WithSubscriber;
use tracing_subscriber::prelude::*;

async fn batch(client: &ProviderClient, relay: &RelayClient, enabled: bool, count: usize) {
    let provider = ProviderSlug::new("nyxbot").unwrap();
    let credential = Secret::new(format!("nyx_{}", uuid::Uuid::new_v4().simple()));
    let address = ReplyAddress::NyxidRelay {
        platform: Platform::Telegram,
        message_id: "private-capability-canary".into(),
        route_id: "benchmark-route".into(),
        reply_token: Secret::new("private-reply-canary"),
        reply_token_expires_at: i64::MAX,
        thread_reply: false,
    };
    stream::iter(0..count)
        .for_each_concurrent(16, |index| {
            let (provider, credential, address) = (&provider, &credential, &address);
            async move {
                let mut response = client
                    .start_turn(
                        provider,
                        TurnRequest {
                            nyxid_trace: enabled,
                            timing: None,
                            conversation_id: ConversationId::new("benchmark-conversation").unwrap(),
                            input: vec![json!({"role":"user","content":"private-input-canary"})],
                            idempotency_key: IdempotencyKey::new(format!("benchmark-{index}"))
                                .unwrap(),
                            metadata: BTreeMap::new(),
                            agent_key: credential.clone(),
                            event_context: None,
                        },
                    )
                    .await
                    .unwrap();
                response.accepted.await.unwrap();
                let mut completed = false;
                while let Some(event) = response.events.recv().await {
                    completed |= matches!(event, ResponsesEvent::Completed { .. });
                }
                assert!(completed);
                let association = response.trace.as_ref().and_then(|slot| slot.get()).copied();
                assert_eq!(association.is_some(), enabled);
                let trace = BoundOperation::start(association, Stage::Platform);
                let send = relay.send(
                    address,
                    Credential::ChannelKey(credential),
                    "private-output-canary",
                );
                let outcome = if let Some(trace) = trace {
                    let outcome = trace.within(send).await;
                    trace.finish(if matches!(outcome, DeliveryOutcome::Confirmed { .. }) {
                        Outcome::Confirmed
                    } else {
                        Outcome::Unknown
                    });
                    outcome
                } else {
                    send.await
                };
                assert!(matches!(outcome, DeliveryOutcome::Confirmed { .. }));
            }
        })
        .await;
}

fn setting(name: &str, default: usize) -> usize {
    std::env::var(name).map_or(default, |value| value.parse().unwrap())
}

#[tokio::test]
async fn trace_admission_relay_throughput() {
    let count = setting("CMAEG_TRACE_BENCH_TURNS", 64);
    let rounds = setting("CMAEG_TRACE_BENCH_ROUNDS", 2);
    assert!((32..=16384).contains(&count) && (2..=32).contains(&rounds));
    for delay in [0, 10] {
        let (mut client, base, calls, server) = fixture::start(delay).await;
        let relay = RelayClient::new(&base, Duration::from_secs(5)).unwrap();
        let (writer, records, _guard) = testing::capture();
        let mut samples = [Vec::new(), Vec::new()];
        let mut total = 0;
        // Alternate AB/BA order. Warm both HTTP pools before collecting samples.
        for round in 0..=rounds {
            for enabled in if round % 2 == 0 {
                [false, true]
            } else {
                [true, false]
            } {
                client.turn_trace_enabled = enabled;
                let _installed = enabled.then(|| testing::install(writer.clone()));
                let subscriber = tracing_subscriber::registry().with(scope::layer());
                let turns = if round == 0 { 32 } else { count };
                let start = Instant::now();
                batch(&client, &relay, enabled, turns)
                    .with_subscriber(subscriber)
                    .await;
                let elapsed = start.elapsed().as_secs_f64();
                total += turns;
                if round > 0 {
                    samples[usize::from(enabled)].push(elapsed);
                }
                // Serialization runs during the measured workload. Validate and
                // drain outside it, including every source link and paired span.
                for _ in 0..if enabled { turns * 9 } else { 0 } {
                    let line = records.recv_timeout(Duration::from_secs(5)).unwrap();
                    let record: Value = serde_json::from_slice(&line).unwrap();
                    assert_eq!(record["schema_version"], 1);
                    assert!(!String::from_utf8(line).unwrap().contains("private-"));
                }
                assert_eq!(writer.dropped(), 0);
                assert!(records.try_recv().is_err());
            }
        }
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), total * 2);
        let medians = samples.each_ref().map(|sample| {
            let mut sample = sample.clone();
            sample.sort_by(f64::total_cmp);
            (sample[(sample.len() - 1) / 2] + sample[sample.len() / 2]) / 2.0
        });
        println!(
            "TRACE_THROUGHPUT {}",
            json!({
                "provider_delay_ms":delay, "concurrency":16, "turns_per_sample":count,
                "off_seconds":samples[0], "on_seconds":samples[1],
                "off_turns_per_second":count as f64/medians[0],
                "on_turns_per_second":count as f64/medians[1],
                "median_throughput_regression_percent":100.0*(1.0-medians[0]/medians[1]),
                "records_per_enabled_turn":9, "dropped":writer.dropped(),
                "external_calls_per_turn":2
            })
        );
        server.abort();
    }
}
