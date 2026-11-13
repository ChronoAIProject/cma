use super::*;
use crate::telemetry::turn_trace::{testing, Context, Operation, SpanId};
use serde_json::Value;
use std::time::Duration;

mod native;

#[tokio::test]
async fn existing_receipts_classify_platform_confirmation_and_partial_delivery_without_raw_values()
{
    let (writer, records, _guard) = testing::capture();
    let parent = Context {
        turn_id: Some(uuid::Uuid::new_v4()),
        span_id: SpanId::fresh(),
        parent_span_id: None,
        request_id: uuid::Uuid::new_v4(),
        ingress_request_id: None,
    };
    for (delivery, expected) in [
        (
            DeliveryOutcome::Confirmed {
                platform_message_id: Some("private-platform-id-canary".into()),
            },
            "confirmed",
        ),
        (
            DeliveryOutcome::Rejected {
                reason: "private-reason-canary".into(),
            },
            "rejected",
        ),
        (
            DeliveryOutcome::Unknown {
                reason: "private-error-canary".into(),
            },
            "unknown",
        ),
        (
            DeliveryOutcome::Unknown {
                reason: "partial_delivery".into(),
            },
            "partial",
        ),
        (DeliveryOutcome::RateLimited { retry_after: 1 }, "rejected"),
        (DeliveryOutcome::Unsupported, "rejected"),
    ] {
        let original = crate::delivery::durable::receipt_for(delivery.clone());
        let trace = BoundOperation::from_operation(Operation::start(
            writer.clone(),
            Stage::Platform,
            Some(parent),
            None,
        ));
        let result = observed(Some(trace), async { original }, receipt).await;
        assert_eq!(result.delivery, delivery);
        let start: Value =
            serde_json::from_slice(&records.recv_timeout(Duration::from_secs(2)).unwrap()).unwrap();
        let end: Value =
            serde_json::from_slice(&records.recv_timeout(Duration::from_secs(2)).unwrap()).unwrap();
        assert_eq!(start["span_id"], end["span_id"]);
        assert_eq!(start["request_id"], end["request_id"]);
        assert_eq!(end["stage"], "platform");
        assert_eq!(end["outcome"], expected);
        assert!(!format!("{start}{end}").contains("private"));
    }
    // Custom local/webhook completion is passed through without a platform span.
    let original = crate::delivery::durable::receipt_for(DeliveryOutcome::Confirmed {
        platform_message_id: None,
    });
    let result = observed(None, async { original }, receipt).await;
    assert!(matches!(result.delivery, DeliveryOutcome::Confirmed { .. }));
    assert!(records.try_recv().is_err());
}

#[test]
fn confirmed_prefix_with_a_rejected_suffix_is_partial() {
    let mut result = crate::delivery::durable::receipt_for(DeliveryOutcome::Rejected {
        reason: "suffix_rejected".into(),
    });
    result
        .message_parts
        .push(super::super::intents::ConfirmedMessagePart {
            index: 0,
            message_ref: "cmaeg1.msg.synthetic".parse().unwrap(),
        });
    assert_eq!(serde_json::to_value(receipt(&result)).unwrap(), "partial");
}

#[tokio::test]
async fn observed_confirmed_prefix_and_unknown_suffix_stay_partial_without_changing_receipts() {
    let (writer, records, _guard) = testing::capture();
    let trace =
        BoundOperation::from_operation(Operation::start(writer, Stage::Platform, None, None));
    let original = crate::delivery::durable::receipt_for(DeliveryOutcome::Unknown {
        reason: "native_whatsapp_acceptance_unconfirmed".into(),
    });
    let result = super::parts::observe(
        trace,
        async {
            super::parts::record(Outcome::Confirmed);
            super::parts::record(Outcome::Unknown);
            original
        },
        receipt,
    )
    .await;
    assert!(matches!(result.delivery, DeliveryOutcome::Unknown { .. }));
    let start: Value =
        serde_json::from_slice(&records.recv_timeout(Duration::from_secs(2)).unwrap()).unwrap();
    let end: Value =
        serde_json::from_slice(&records.recv_timeout(Duration::from_secs(2)).unwrap()).unwrap();
    assert_eq!(start["span_id"], end["span_id"]);
    assert_eq!(end["outcome"], "partial");
    assert!(records.try_recv().is_err());
}

#[tokio::test]
async fn successful_existing_retry_remains_confirmed_after_a_rejected_attempt() {
    let (writer, records, _guard) = testing::capture();
    let trace =
        BoundOperation::from_operation(Operation::start(writer, Stage::Platform, None, None));
    let result = super::parts::observe(
        trace,
        async {
            super::parts::record(Outcome::Rejected);
            super::parts::record(Outcome::Confirmed);
            crate::delivery::durable::receipt_for(DeliveryOutcome::Confirmed {
                platform_message_id: None,
            })
        },
        receipt,
    )
    .await;
    assert!(matches!(result.delivery, DeliveryOutcome::Confirmed { .. }));
    records.recv_timeout(Duration::from_secs(2)).unwrap();
    let end: Value =
        serde_json::from_slice(&records.recv_timeout(Duration::from_secs(2)).unwrap()).unwrap();
    assert_eq!(end["outcome"], "confirmed");
}
