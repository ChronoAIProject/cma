use super::{native_lark_media_fixture as media, *};
use crate::{
    interactions::native_lark::fixture::Fixture,
    telemetry::turn_trace::{scope, testing, Context, SpanId},
};
use cmaeg_core::channel::LarkRegion;
use serde_json::Value;
use std::sync::atomic::Ordering;
use tracing::instrument::WithSubscriber;
use tracing_subscriber::prelude::*;

#[tokio::test]
async fn trace_compound_cards_separate_uploads_from_delivery_without_extra_writes() {
    for fault in [0, 1, 3] {
        let mut parity = Vec::new();
        for enabled in [false, true] {
            let f = Fixture::new(false, false, LarkRegion::Lark).await;
            let (mut sender, api) = media::sender(&f);
            sender.relay = sender.relay.with_native_lark_multi_image_controls(true);
            api.unique_images.store(true, Ordering::SeqCst);
            api.fault.store(fault, Ordering::SeqCst);
            api.upload_fault_at.store(2, Ordering::SeqCst);
            let mut message = media::message(false, "image/png");
            message
                .native_attachments
                .push(message.native_attachments[0].clone());
            let gate = RecordingGate::new(true);
            let (writer, records, _guard) = testing::capture();
            let _installed = enabled.then(|| testing::install(writer.clone()));
            let turn = uuid::Uuid::new_v4();
            let subscriber = tracing_subscriber::registry().with(scope::layer());
            let result = async {
                let span = scope::span();
                scope::attach(
                    &span,
                    Context {
                        turn_id: Some(turn),
                        span_id: SpanId::fresh(),
                        parent_span_id: None,
                        request_id: uuid::Uuid::new_v4(),
                        ingress_request_id: None,
                    },
                );
                sender
                    .reply(
                        &SendContext {
                            trace: &span,
                            channel: &f.channel,
                            first_send_done: false,
                            gate: &gate,
                        },
                        &f.event,
                        &message,
                    )
                    .await
                    .0
            }
            .with_subscriber(subscriber)
            .await;
            parity.push((
                result.delivery.kind(),
                gate.calls(),
                api.calls.lock().unwrap().len(),
                f.server.memory.writes.load(Ordering::SeqCst),
            ));
            if !enabled {
                assert!(records.try_recv().is_err());
                continue;
            }
            let sends = usize::from(fault != 1);
            let lines: Vec<Value> = (0..2 * (3 + sends))
                .map(|_| {
                    serde_json::from_slice(&records.recv_timeout(Duration::from_secs(2)).unwrap())
                        .unwrap()
                })
                .collect();
            let ends: Vec<_> = lines.iter().filter(|r| r["phase"] == "end").collect();
            assert_eq!(ends[0]["stage"], "channel_part");
            assert_eq!(ends[0]["part_index"], 0);
            assert_eq!(ends[0]["outcome"], "completed");
            assert_eq!(ends[1]["stage"], "channel_part");
            assert_eq!(ends[1]["part_index"], 1);
            assert_eq!(
                ends[1]["outcome"],
                if fault == 1 { "unknown" } else { "completed" }
            );
            if sends == 1 {
                assert_eq!(ends[2]["stage"], "platform");
                assert_eq!(ends[2]["part_index"], 0);
                assert_eq!(
                    ends[2]["outcome"],
                    if fault == 0 { "confirmed" } else { "unknown" }
                );
            }
            for end in ends {
                let start = lines
                    .iter()
                    .find(|r| r["phase"] == "start" && r["span_id"] == end["span_id"])
                    .unwrap();
                assert_eq!(start["request_id"], end["request_id"]);
                assert_eq!(end["turn_id"], turn.to_string());
                assert!(!end.to_string().contains("native_chat"));
                assert!(!end.to_string().contains("confirmed_card"));
            }
            assert_eq!(writer.dropped(), 0);
            assert!(records.try_recv().is_err());
        }
        assert_eq!(parity[0], parity[1], "fault {fault}");
    }
}
