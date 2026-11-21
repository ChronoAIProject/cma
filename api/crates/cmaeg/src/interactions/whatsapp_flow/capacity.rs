//! Reserve durable capacity before any asset generation is allocated.
use super::super::Card;
use serde_json::json;

pub(super) fn fits(card: &Card) -> bool {
    let Some(a) = &card.native_whatsapp_flow else {
        return false;
    };
    // Message IDs permit quotes/backslashes: use the largest escaped encoding.
    let message = format!("wamid.{}", "\\".repeat(506));
    let event_id = format!("native-whatsapp-flow:{}", "a".repeat(64));
    let mut event = serde_json::to_value(&a.original).expect("event serializes");
    event["event_id"] = json!(event_id);
    event["expires_at"] = json!(i64::MAX);
    event["activity_sha256"] = json!("a".repeat(64));
    event["reply"]["message_id"] = json!(message);
    let receipt = json!({"kind":"native_whatsapp_receipt_v1",
        "original":{"owner":card.owner,"channel_id":card.channel_id,
            "record_id":a.original.record_id,"event_id":card.event_id},
        "source":a.activity.source,"recipient":card.actor,"message_id":message,
        "consumer":card.consumer.clone().unwrap_or_else(cmaeg_core::ids::ConsumerId::default_consumer),
        "provider":a.provider,"partition_key":a.original.partition_key,
        "conversation_id":a.original.conversation_id,"confirmed_at":i64::MAX,"retained_until":i64::MAX});
    let mut maximum = serde_json::to_value(card).expect("card serializes");
    maximum["reference"] = json!("a".repeat(cmaeg_core::seal::MAX_TOKEN_LENGTH));
    maximum["consumed"] = json!({"event_id":event_id,"callback_id":message,
        "action":u16::MAX,"selection":null,"submitted":null,"form_launch":null,
        "occurred_at":"a".repeat(40)});
    maximum["native_whatsapp_flow"]["phase"] =
        json!({"phase":"bound","key":"a".repeat(64),"receipt":receipt});
    maximum["native_whatsapp_flow"]["pending"] = json!({"event":event,
        "reference":"a".repeat(cmaeg_core::seal::MAX_TOKEN_LENGTH),"admitted":false,
        "callback":{"kind":"native_whatsapp_flow_completion_v1","card":card.id,
            "revision":1,"source":a.activity.source,"id":message,"sender":card.actor,
            "context":message,"timestamp":i64::MAX,
            // A string of this length dominates every allowed serialized fields object.
            "fields":"a".repeat(crate::whatsapp::forms::MAX_SUBMITTED_BYTES)}});
    serde_json::to_vec(&maximum).is_ok_and(|bytes| bytes.len() <= cmaeg_core::seal::MAX_BLOB_BYTES)
}
