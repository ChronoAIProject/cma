use super::*;
use crate::{
    delivery::durable::receipt_for,
    delivery::intents::DeliveryOutcome,
    ingress::{routing_plan::fixture, routing_store::test_server::Server},
};
use cmaeg_core::{ids::MessageRef, seal::MessageRefPayload};
use serde_json::json;

fn receipt(count: usize, unknown: bool) -> IntentReceipt {
    let mut receipt = receipt_for(if unknown {
        DeliveryOutcome::Unknown {
            reason: "partial_delivery".into(),
        }
    } else {
        DeliveryOutcome::Confirmed {
            platform_message_id: Some("800".into()),
        }
    });
    receipt.message_parts = (0..count)
        .map(|index| {
            let payload: MessageRefPayload = serde_json::from_value(json!({
                "channel_id":"cmaeg1.ch.fixture", "event_id":"telegram:77:700",
                "platform_message_id":(800+index).to_string(), "route_id":"telegram:77:42",
                "consumer":"csm_00000000000a"
            }))
            .unwrap();
            ConfirmedMessagePart {
                index: index as u16,
                message_ref: MessageRef::seal(&payload, &fixture::keys()).unwrap(),
            }
        })
        .collect();
    receipt
}

#[tokio::test]
async fn largest_valid_split_receipt_roundtrips_exact_tokens_in_bounded_owned_pages() {
    let server = Server::start().await;
    let records = server.records.clone().interactions();
    let keys = fixture::keys();
    let store = ReceiptStore {
        records: &records,
        keys: &keys,
        scope: "exact-operation",
        owner: "user-1",
        retained_until: fixture::NOW + 600,
    };
    // The actual text chunker produces 1105 parts for a valid 17-section message
    // with escaped text and short paragraph boundaries (each section <=32KiB).
    let receipt = receipt(1105, true);
    let original = encoded(&receipt).unwrap();
    assert!(original.len() > 256 * 1024);
    let stored = store.retain(&receipt).await.unwrap();
    assert!(matches!(stored, StoredReceipt::Paged { .. }));
    assert!(encoded(&stored).unwrap().len() < INLINE_LIMIT);
    let restart: StoredReceipt = serde_json::from_slice(&encoded(&stored).unwrap()).unwrap();
    let replay = store.load(&restart).await.unwrap();
    assert_eq!(encoded(&replay).unwrap(), original);
    assert!(matches!(replay.delivery, DeliveryOutcome::Unknown { .. }));
    for row in server.memory.rows.lock().unwrap().values() {
        assert_eq!(row.owner, "user-1");
        assert_eq!(row.expiry, fixture::NOW + 600);
        assert!(row.blob.len() <= 65536);
    }
    for (scope, owner, until) in [
        ("other-operation", "user-1", fixture::NOW + 600),
        ("exact-operation", "other-owner", fixture::NOW + 600),
        ("exact-operation", "user-1", fixture::NOW + 601),
    ] {
        let wrong = ReceiptStore {
            records: &records,
            keys: &keys,
            scope,
            owner,
            retained_until: until,
        };
        assert!(wrong.load(&restart).await.is_err());
    }
}

#[tokio::test]
async fn partial_page_write_cannot_publish_pointer_and_lost_reply_requires_exact_readback() {
    for persisted in [false, true] {
        let server = Server::start().await;
        let records = server.records.clone().interactions();
        let keys = fixture::keys();
        let store = ReceiptStore {
            records: &records,
            keys: &keys,
            scope: "operation",
            owner: "user-1",
            retained_until: fixture::NOW + 600,
        };
        let receipt = receipt(80, false);
        let manifest = PartsManifest {
            kind: KIND.into(),
            scope: hash(b"operation"),
            owner: "user-1".into(),
            retained_until: fixture::NOW + 600,
            receipt_sha256: hash(&encoded(&receipt).unwrap()),
            count: 80,
        };
        server.fault(page_id(&manifest, 1), persisted, false);
        let result = store.retain(&receipt).await;
        if persisted {
            let saved = result.unwrap();
            assert_eq!(
                encoded(&store.load(&saved).await.unwrap()).unwrap(),
                encoded(&receipt).unwrap()
            );
        } else {
            assert!(result.is_err());
            assert_eq!(
                server.memory.rows.lock().unwrap().len(),
                1,
                "only the first page exists; no root receipt pointer is returned"
            );
        }
    }
}

#[tokio::test]
async fn missing_tampered_reordered_or_replaced_pages_never_hydrate_a_receipt() {
    for variant in ["missing", "scope", "token", "order", "version"] {
        let server = Server::start().await;
        let records = server.records.clone().interactions();
        let keys = fixture::keys();
        let store = ReceiptStore {
            records: &records,
            keys: &keys,
            scope: "operation",
            owner: "user-1",
            retained_until: fixture::NOW + 600,
        };
        let saved = store.retain(&receipt(80, true)).await.unwrap();
        let StoredReceipt::Paged {
            message_parts_pages: ref manifest,
            ..
        } = saved
        else {
            panic!()
        };
        let id = page_id(manifest, 0);
        match variant {
            "missing" => {
                server.memory.rows.lock().unwrap().remove(&id);
            }
            "version" => {
                server
                    .memory
                    .rows
                    .lock()
                    .unwrap()
                    .get_mut(&id)
                    .unwrap()
                    .version = 2;
            }
            "scope" => server.change(&id, |value| value["scope"] = "other-operation".into()),
            "token" => server.change(&id, |value| {
                value["parts"][0]["message_ref"] = "cmaeg1.mr.corrupt".into()
            }),
            "order" => server.change(&id, |value| {
                value["parts"].as_array_mut().unwrap().swap(0, 1)
            }),
            _ => unreachable!(),
        }
        assert!(store.load(&saved).await.is_err(), "{variant}");
    }
}

#[test]
fn legacy_receipts_keep_identical_stored_bytes_and_unknown_fields_are_refused() {
    let legacy = receipt(0, false);
    let bytes = encoded(&legacy).unwrap();
    let stored: StoredReceipt = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(encoded(&stored).unwrap(), bytes);
    let mut value = serde_json::to_value(legacy).unwrap();
    value["unsupported_field"] = json!(true);
    assert!(serde_json::from_value::<StoredReceipt>(value).is_err());
}

#[test]
fn valid_large_text_and_maximum_public_tool_result_fit_derived_bounds() {
    use crate::delivery::render::{RelayText, Renderer};
    use crate::tools::Capabilities;
    use cmaeg_core::message::OutboundMessage;
    let section = ("_".repeat(500) + "\n\n").repeat(32768 / 502);
    let message: OutboundMessage = serde_json::from_value(json!({
        "text":section,
        "blocks":(0..16).map(|_|json!({"type":"text","text":section})).collect::<Vec<_>>()
    }))
    .unwrap();
    message.validate().unwrap();
    let (parts, _) = RelayText.render(&message, &Capabilities::native_discord());
    assert_eq!(parts.len(), 1105);
    assert!(parts.len() <= MAX_PARTS);
    // Public MCP requests are capped at 128 KiB. Escape expansion and paragraph
    // boundaries cannot produce more than this many 2000-unit platform parts.
    let public_parts = 2 * (2usize * 131072).div_ceil(1998) + 1;
    let reference = MessageRef::new(format!("cmaeg1.mr.{}", "x".repeat(4086))).unwrap();
    let mut receipt = receipt(0, false);
    receipt.message_parts = (0..public_parts)
        .map(|index| ConfirmedMessagePart {
            index: index as u16,
            message_ref: reference.clone(),
        })
        .collect();
    let value = serde_json::to_value(receipt).unwrap();
    let envelope = json!({"content":[{"type":"text","text":value.to_string()}],"structuredContent":value,"isError":false});
    assert!(serde_json::to_vec(&envelope).unwrap().len() < 4 * 1024 * 1024);
}
