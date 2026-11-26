//! PR B desired arbitration rows. Stored snapshots model both ordinary-close
//! positions; production admission/execution/receipt replay is exercised below.
use super::{super::replica_test_fixture::*, mongo_progress_fixture::*, *};
use crate::{
    delivery::{
        durable::receipt_for,
        intents::{ConfirmedMessagePart, DeliveryOutcome},
    },
    turns::routed::tool_reply_store::{Admission, Kind, ReplyStore},
};
use cmaeg_core::{ids::MessageRef, seal::MessageRefPayload};
use std::sync::atomic::Ordering;

#[derive(Clone, Copy, Debug)]
enum AttemptState {
    None,
    Pending,
    Confirmed,
    Unknown,
    Rejected,
    RejectedAfterParts,
    UnknownParts,
}
#[derive(Clone, Copy, Debug)]
enum Expected {
    Send,
    Wait,
    Uncertain,
}

async fn row(closed: bool, automatic: bool, state: AttemptState, expected: Expected) {
    combined_row(closed, automatic, state, None, expected).await;
}
async fn combined_row(
    closed: bool,
    automatic: bool,
    state: AttemptState,
    peer: Option<AttemptState>,
    expected: Expected,
) {
    with_harness(|h| Box::pin(async move {
        let (owner, mut parent) = h.accepted().await;
        let journal = h.nodes[owner].journal();
        let guard = ReplyStore(journal.clone(), parent.partition.clone());
        if !matches!(state, AttemptState::None) {
            let kind = if automatic { Kind::Automatic } else { Kind::Tool };
            let Admission::Claimed(claim) = guard.admit(&h.plan, h.target(), kind, "d".repeat(64)).await.unwrap() else { panic!("initial attempt") };
            if !matches!(state, AttemptState::Pending) {
                let mut saved = match state {
                    AttemptState::Confirmed => receipt(),
                    AttemptState::Unknown | AttemptState::UnknownParts => receipt_for(DeliveryOutcome::Unknown { reason: "fixture_receipt_lost".into() }),
                    AttemptState::Rejected | AttemptState::RejectedAfterParts => receipt_for(DeliveryOutcome::Rejected { reason: "fixture_part_rejected".into() }),
                    _ => unreachable!(),
                };
                if matches!(state, AttemptState::RejectedAfterParts | AttemptState::UnknownParts) {
                    let reference: MessageRefPayload = serde_json::from_value(serde_json::json!({
                        "channel_id":h.event().channel_id,"event_id":h.event().event_id,
                        "consumer":h.event().consumer,"platform_message_id":"800","route_id":"telegram:77:42"
                    })).unwrap();
                    saved.message_parts.push(ConfirmedMessagePart { index: 0, message_ref: MessageRef::seal(&reference, &h.database.keys()).unwrap() });
                }
                guard.finish(&claim, &saved).await.unwrap();
            }
        }
        let (mut record, version) = guard.operator_record(&h.plan, h.target()).await.unwrap();
        if let Some(peer) = peer {
            let receipt = match peer {
                AttemptState::Confirmed => Some(receipt()),
                AttemptState::Unknown => Some(receipt_for(DeliveryOutcome::Unknown { reason: "fixture_other_slot".into() })),
                AttemptState::Pending => None,
                _ => unreachable!(),
            };
            let slot = if automatic { "tool" } else { "automatic" };
            record[slot] = serde_json::json!({"claim":uuid::Uuid::new_v4().to_string(),"operation":"e".repeat(64),"until":(journal.clock)()+30,"receipt":receipt});
            if automatic && matches!(peer, AttemptState::Confirmed) { record["confirmed_tool"] = serde_json::json!(true); }
        }
        let guard_id = record["id"].as_str().unwrap().to_owned();
        if closed {
            // Establish a real, proved ordinary close before restoring a later
            // guard snapshot. Do not synthesize Completed without its checkpoint.
            let mut empty = record.clone();
            empty["tool"] = serde_json::Value::Null;
            empty["automatic"] = serde_json::Value::Null;
            empty["confirmed_tool"] = serde_json::json!(false);
            journal.write_fenced(&parent.partition, &guard_id, &h.plan.identity.owner,
                h.plan.retained_until, &empty, (version > 0).then_some(version)).await.unwrap();
            h.finish_parent(owner, &mut parent).await;
            let (_, closed_version) = journal.read::<serde_json::Value>(&guard_id).await.unwrap().unwrap();
            record["closed"] = serde_json::json!(true);
            // Restore retained history, including defensive inconsistent cases.
            // This fixture write grants no live ownership or transport permission.
            journal.write(&guard_id, &h.plan.identity.owner, h.plan.retained_until,
                &record, Some(closed_version)).await.unwrap();
        } else {
            journal.write_fenced(&parent.partition, &guard_id, &h.plan.identity.owner,
                h.plan.retained_until, &record, (version > 0).then_some(version)).await.unwrap();
            journal.release(&mut parent).await.unwrap();
        }
        let mut capability = h.capability(); capability.issued_at -= 5;
        let key = "new-delayed-occurrence";
        let first = h.tools[0].reply_delayed(caller(), &capability, key, message()).await;
        assert!(matches!(first, Err(ExecutionError::Busy) | Ok(Some(_))), "admission: {first:?}");
        let id = effect_id(h, &capability, key);
        let (sink, attempts) = transport(h, Duration::ZERO);
        let mut result = first;
        for _ in 0..3 {
            sweep(h, &id).await;
            result = h.tools[1].reply_delayed(caller(), &capability, key, message()).await;
            if !matches!(result, Err(ExecutionError::Busy)) || matches!(expected, Expected::Wait) { break; }
        }
        let sent = attempts.load(Ordering::SeqCst);
        let correct = match (&expected, &result) {
            (Expected::Send, Ok(Some(r))) => matches!(r.delivery, DeliveryOutcome::Confirmed { .. }) && sent == 1,
            (Expected::Wait, Err(ExecutionError::Busy)) => sent == 0,
            (Expected::Uncertain, Ok(Some(r))) => matches!(
                &r.delivery,
                DeliveryOutcome::Unknown { reason } if reason == "delayed_reply_guard_uncertain"
            ) && sent == 0,
            _ => false,
        };
        assert!(correct, "PR B arbitration: closed={closed}, automatic={automatic}, state={state:?}, expected={expected:?}, sends={sent}, observed={result:?}");
        if let Ok(Some(saved)) = result {
            let replay = h.tools[0].reply_delayed(caller(), &capability, key, message()).await.unwrap().unwrap();
            assert_eq!(super::super::storage::bytes(&saved).unwrap(), super::super::storage::bytes(&replay).unwrap());
            assert_eq!(attempts.load(Ordering::SeqCst), sent, "same-key receipt replay cannot send again");
        }
        sink.abort();
    })).await;
}
macro_rules! case {
    ($name:ident, $closed:expr, $automatic:expr, $state:ident, $expected:ident) => {
        #[tokio::test]
        #[ignore = "requires authenticated disposable Mongo replica set"]
        async fn $name() {
            row(
                $closed,
                $automatic,
                AttemptState::$state,
                Expected::$expected,
            )
            .await;
        }
    };
}
case!(a01_open_auto_none_sends_once, false, true, None, Send);
case!(
    a02_open_auto_pending_waits_without_send,
    false,
    true,
    Pending,
    Wait
);
case!(
    a03_open_auto_confirmed_allows_distinct_occurrence_once,
    false,
    true,
    Confirmed,
    Send
);
case!(
    a04_open_auto_unknown_retains_unknown_without_send,
    false,
    true,
    Unknown,
    Uncertain
);
case!(
    a05_open_auto_rejected_after_parts_is_uncertain,
    false,
    true,
    RejectedAfterParts,
    Uncertain
);
case!(a06_open_tool_none_sends_once, false, false, None, Send);
case!(
    a07_open_tool_pending_waits_without_send,
    false,
    false,
    Pending,
    Wait
);
case!(
    a08_open_tool_confirmed_allows_distinct_occurrence_once,
    false,
    false,
    Confirmed,
    Send
);
case!(
    a09_open_tool_unknown_retains_unknown_without_send,
    false,
    false,
    Unknown,
    Uncertain
);
case!(
    a10_open_tool_rejected_after_parts_is_uncertain,
    false,
    false,
    RejectedAfterParts,
    Uncertain
);
case!(a11_closed_auto_none_sends_once, true, true, None, Send);
case!(
    a12_closed_auto_pending_waits_without_send,
    true,
    true,
    Pending,
    Wait
);
case!(
    a13_closed_auto_confirmed_allows_distinct_occurrence_once,
    true,
    true,
    Confirmed,
    Send
);
case!(
    a14_closed_auto_unknown_retains_unknown_without_send,
    true,
    true,
    Unknown,
    Uncertain
);
case!(
    a15_closed_auto_rejected_after_parts_is_uncertain,
    true,
    true,
    RejectedAfterParts,
    Uncertain
);
case!(a16_closed_tool_none_sends_once, true, false, None, Send);
case!(
    a17_closed_tool_pending_waits_without_send,
    true,
    false,
    Pending,
    Wait
);
case!(
    a18_closed_tool_confirmed_allows_distinct_occurrence_once,
    true,
    false,
    Confirmed,
    Send
);
case!(
    a19_closed_tool_unknown_retains_unknown_without_send,
    true,
    false,
    Unknown,
    Uncertain
);
case!(
    a20_closed_tool_rejected_after_parts_is_uncertain,
    true,
    false,
    RejectedAfterParts,
    Uncertain
);

case!(
    a21_open_auto_unknown_parts_no_send,
    false,
    true,
    UnknownParts,
    Uncertain
);
case!(
    a22_open_tool_unknown_parts_no_send,
    false,
    false,
    UnknownParts,
    Uncertain
);
case!(
    a23_closed_auto_unknown_parts_no_send,
    true,
    true,
    UnknownParts,
    Uncertain
);
case!(
    a24_closed_tool_unknown_parts_no_send,
    true,
    false,
    UnknownParts,
    Uncertain
);
case!(
    a25_open_auto_rejected_sends_once,
    false,
    true,
    Rejected,
    Send
);
case!(
    a26_open_tool_rejected_sends_once,
    false,
    false,
    Rejected,
    Send
);
case!(
    a27_closed_auto_rejected_sends_once,
    true,
    true,
    Rejected,
    Send
);
case!(
    a28_closed_tool_rejected_sends_once,
    true,
    false,
    Rejected,
    Send
);

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn both_slots_unknown_takes_precedence_over_pending_in_either_order() {
    combined_row(
        false,
        true,
        AttemptState::Pending,
        Some(AttemptState::Unknown),
        Expected::Uncertain,
    )
    .await;
    combined_row(
        true,
        false,
        AttemptState::Pending,
        Some(AttemptState::Unknown),
        Expected::Uncertain,
    )
    .await;
}
#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn both_slots_pending_takes_precedence_over_confirmation_in_either_order() {
    combined_row(
        false,
        true,
        AttemptState::Confirmed,
        Some(AttemptState::Pending),
        Expected::Wait,
    )
    .await;
    combined_row(
        true,
        false,
        AttemptState::Confirmed,
        Some(AttemptState::Pending),
        Expected::Wait,
    )
    .await;
}
