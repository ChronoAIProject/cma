use super::*;
use crate::bridge::*;
use serde_json::json;

mod fixtures;
use fixtures::*;

#[test]
fn local_identity_is_stable_but_changed_content_conflicts() {
    let a = items("Final output", false);
    let b = items("Changed output", false);
    assert_eq!(a[0].id, b[0].id);
    assert_eq!(
        a[0].require_same_output(&b[0]),
        Err(OutboxError::SourceMismatch)
    );
    let reopened: OutboxItem =
        serde_json::from_value(serde_json::to_value(&a[0]).unwrap()).unwrap();
    reopened.require_same_output(&a[0]).unwrap();
}

#[test]
fn chunks_require_retained_key_and_confirmed_predecessor() {
    let intent = intent(&"x".repeat(5000), "request");
    assert!(matches!(
        OutboxItem::from_tool_intent(&intent, token(), 1, None, 100),
        Err(OutboxError::RetainedKeyRequired)
    ));
    let mut batch = items(&"x".repeat(5000), false);
    assert_eq!(batch.len(), 2);
    assert!(!batch[0].last_chunk);
    assert!(batch[1].last_chunk);
    let first = batch[0].clone();
    assert_eq!(
        batch[1].claim("worker".into(), 101, 1000, Some(&first)),
        Err(OutboxError::PredecessorPending)
    );
    send(&mut batch[0], DeliveryOutcome::Confirmed(receipt()));
    let first = batch[0].clone();
    batch[1]
        .claim("worker".into(), 110, 1000, Some(&first))
        .unwrap();
}

#[test]
fn crash_before_send_is_recoverable_but_after_send_is_unknown() {
    let mut before = items("final", true).remove(0);
    before.claim("worker".into(), 101, 200, None).unwrap();
    before.recover_expired(200).unwrap();
    assert_eq!(before.state, DeliveryState::Pending);
    let attempt = before.claim("new-worker".into(), 201, 300, None).unwrap();
    before.begin_send("new-worker", &attempt, 202).unwrap();
    let mut after: OutboxItem =
        serde_json::from_value(serde_json::to_value(before).unwrap()).unwrap();
    after.recover_expired(300).unwrap();
    assert!(matches!(
        after.state,
        DeliveryState::Unknown {
            reason: DeliveryFailure::WorkerLost,
            ..
        }
    ));
    assert_eq!(
        after.claim("third".into(), 301, 400, None),
        Err(OutboxError::UnknownOutcome)
    );
    assert_eq!(
        after.complete(&attempt, DeliveryOutcome::Confirmed(receipt()), 302),
        Err(OutboxError::Conflict)
    );
}

#[test]
fn authority_checks_can_defer_claim_but_cannot_requeue_sending() {
    let mut item = items("Acknowledgement", false).remove(0);
    let attempt = item.claim("worker".into(), 101, 1000, None).unwrap();
    item.defer_before_send(
        "worker",
        &attempt,
        DeliveryFailure::ConnectUnavailable,
        500,
        102,
    )
    .unwrap();
    assert!(matches!(
        item.state,
        DeliveryState::RetryWait {
            retry_at_ms: 500,
            ..
        }
    ));
    assert_eq!(
        item.claim("worker".into(), 499, 1000, None),
        Err(OutboxError::Conflict)
    );
    let next = item.claim("worker".into(), 500, 1000, None).unwrap();
    item.begin_send("worker", &next, 501).unwrap();
    assert_eq!(
        item.defer_before_send(
            "worker",
            &next,
            DeliveryFailure::ConnectUnavailable,
            900,
            502
        ),
        Err(OutboxError::Conflict)
    );
    assert!(matches!(item.state, DeliveryState::Sending { .. }));
    assert_eq!(item.attempts[0].credential, key());
    assert_eq!(item.attempts[1].credential, key());
}

#[test]
fn expired_original_authority_becomes_terminal_before_network_io() {
    let mut item = items("Late reply", true).remove(0);
    assert_eq!(item.reject_expired(9999), Err(OutboxError::Conflict));
    item.reject_expired(10000).unwrap();
    assert_eq!(
        item.state,
        DeliveryState::Rejected {
            reason: DeliveryFailure::CredentialExpired
        }
    );
    assert!(item.attempts.is_empty());
}

#[test]
fn repeated_pre_send_outages_end_with_truthful_terminal_receipt() {
    let mut item = items("Reply", false).remove(0);
    for number in 0..16 {
        let now = 101 + number * 3;
        let attempt = item.claim("worker".into(), now, now + 1000, None).unwrap();
        item.defer_before_send(
            "worker",
            &attempt,
            DeliveryFailure::ConnectUnavailable,
            now + 3,
            now + 1,
        )
        .unwrap();
    }
    assert_eq!(item.attempts.len(), 16);
    assert_eq!(
        item.state,
        DeliveryState::Rejected {
            reason: DeliveryFailure::ConnectUnavailable
        }
    );
}

#[test]
fn consumed_token_failure_and_timeout_do_not_automatically_retry() {
    for reason in [
        DeliveryFailure::ProviderFailure,
        DeliveryFailure::ResponseLost,
    ] {
        let mut item = items("final", true).remove(0);
        send(&mut item, DeliveryOutcome::Unknown(reason));
        assert!(matches!(item.state, DeliveryState::Unknown { .. }));
        assert_eq!(
            item.claim("again".into(), 105, 200, None),
            Err(OutboxError::UnknownOutcome)
        );
    }
}

#[test]
fn positive_no_send_evidence_retains_retry_after_and_token_expiry() {
    let mut item = items("final", true).remove(0);
    send(
        &mut item,
        DeliveryOutcome::RetryableNotSent {
            reason: DeliveryFailure::RateLimited,
            retry_at_ms: 500,
            evidence: NotSentEvidence::VerifiedRejectionBeforeTokenConsumption,
        },
    );
    assert_eq!(
        item.claim("early".into(), 499, 1000, None),
        Err(OutboxError::Conflict)
    );
    item.claim("due".into(), 500, 1000, None).unwrap();
    let mut expired = items("final", true).remove(0);
    send(
        &mut expired,
        DeliveryOutcome::RetryableNotSent {
            reason: DeliveryFailure::ConnectUnavailable,
            retry_at_ms: 20_000,
            evidence: NotSentEvidence::NoRequestBytesWritten,
        },
    );
    assert_eq!(
        expired.state,
        DeliveryState::Rejected {
            reason: DeliveryFailure::CredentialExpired
        }
    );
}

#[test]
fn invalid_success_receipt_is_unknown_and_terminal_rejection_stays_distinct() {
    let mut unknown = items("final", false).remove(0);
    send(
        &mut unknown,
        DeliveryOutcome::Confirmed(DeliveryReceipt {
            nyxid_message_id: String::new(),
            platform_message_id: None,
        }),
    );
    assert!(matches!(
        unknown.state,
        DeliveryState::Unknown {
            reason: DeliveryFailure::ReceiptInvalid,
            ..
        }
    ));
    let mut rejected = items("final", false).remove(0);
    send(
        &mut rejected,
        DeliveryOutcome::DefinitiveRejection(DeliveryFailure::BotInactive),
    );
    assert_eq!(
        rejected.state,
        DeliveryState::Rejected {
            reason: DeliveryFailure::BotInactive
        }
    );
}

#[test]
fn original_routing_is_preserved_and_suspend_or_emergency_refuses_it() {
    let source = source();
    let mut bridge = bridge();
    let mut newer = bridge.generations[0].clone();
    newer.config_revision = 2;
    newer.previous_work = Some(AcceptedWorkPolicy::DrainOriginal);
    newer.config.connector.key_id = "replacement-key".into();
    newer.config.display_name = "New generation".into();
    bridge.generations.push(newer);
    bridge.revision = 2;
    bridge.config_revision = 2;
    source.require_bridge(&bridge).unwrap();
    assert_eq!(source.config().connector.key_id, "key");
    bridge.generations[1].previous_work = Some(AcceptedWorkPolicy::SuspendOriginal);
    assert_eq!(source.require_bridge(&bridge), Err(OutboxError::Suspended));
    bridge.generations[1].previous_work = Some(AcceptedWorkPolicy::DrainOriginal);
    bridge.emergency.outbound_stopped = true;
    assert_eq!(source.require_bridge(&bridge), Err(OutboxError::Suspended));
}

#[test]
fn original_interaction_deadline_is_not_extended_by_retained_key() {
    let mut source = source();
    source.route_expires_at_ms = Some(150);
    assert_eq!(
        key().require_source(&source, 150),
        Err(OutboxError::Expired)
    );
    assert_eq!(
        token().require_source(&source, 150),
        Err(OutboxError::Expired)
    );
}

#[test]
fn tool_request_identity_survives_turn_change_and_conflicts_on_changed_text() {
    let first = intent("First reply", "stable-request");
    let mut later_scope = scope();
    later_scope.turn_sequence = 2;
    later_scope.source.command_id = crate::ids::AgentCommandId("later-command".into());
    let repeated = ChannelReplyToolIntent::prepare(
        later_scope,
        ChannelReplyToolRequest {
            request_key: "stable-request".into(),
            text: "First reply".into(),
        },
    )
    .unwrap();
    assert_eq!(first.id(), repeated.id());
    assert_eq!(first.request_sha256(), repeated.request_sha256());
    let changed = intent("Different reply", "stable-request");
    assert_eq!(first.id(), changed.id());
    assert_ne!(first.request_sha256(), changed.request_sha256());
}

#[test]
fn multiple_explicit_calls_in_one_active_turn_have_distinct_ordered_items() {
    let first = intent("I am checking that now", "ack");
    let second = intent("Here is what I found so far", "progress");
    assert_eq!(
        first.scope().turn_sequence(),
        second.scope().turn_sequence()
    );
    assert_ne!(first.id(), second.id());
    let mut first_items = OutboxItem::from_tool_intent(&first, key(), 1, None, 100).unwrap();
    let mut second_items =
        OutboxItem::from_tool_intent(&second, key(), 2, Some(first_items[0].id.clone()), 100)
            .unwrap();
    assert_eq!(
        second_items[0].claim("worker".into(), 101, 1000, Some(&first_items[0])),
        Err(OutboxError::PredecessorPending)
    );
    send(&mut first_items[0], DeliveryOutcome::Confirmed(receipt()));
    second_items[0]
        .claim("worker".into(), 110, 1000, Some(&first_items[0]))
        .unwrap();
    assert_eq!(second_items[0].text(), "Here is what I found so far");
}

#[test]
fn tool_request_rejects_destination_or_credential_injection_and_scope_expiry() {
    for field in [
        "chat_id",
        "bot_id",
        "thread_id",
        "key",
        "command_id",
        "reply_token",
    ] {
        let mut body = json!({"request_key":"message","text":"Hello"});
        body[field] = json!("forged");
        assert!(serde_json::from_value::<ChannelReplyToolRequest>(body).is_err());
    }
    let mut expired = scope();
    expired.deadline = tokio::time::Instant::now();
    assert!(matches!(
        ChannelReplyToolIntent::prepare(
            expired,
            ChannelReplyToolRequest {
                request_key: "message".into(),
                text: "Hello".into()
            }
        ),
        Err(OutboxError::Expired)
    ));
}

#[test]
fn escaping_keeps_utf16_bounds_and_breaks_markup_and_mass_mentions() {
    for platform in [
        BridgePlatform::Telegram,
        BridgePlatform::Discord,
        BridgePlatform::Lark,
    ] {
        let text = format!(
            "@everyone <@123> <at user_id=\"all\">x</at> [click](evil) _x_ `code` {}",
            "😀".repeat(5000)
        );
        let parts = super::text::chunks(&platform, &text).unwrap();
        let limit = if platform == BridgePlatform::Telegram {
            4000
        } else {
            1900
        };
        assert!(parts.iter().all(|p| p.encode_utf16().count() <= limit));
        let full = parts.join("");
        assert!(!full.contains("@everyone"));
        assert!(!full.contains("<@"));
        assert!(!full.contains("<at "));
        assert_eq!(full.chars().filter(|c| *c == '😀').count(), 5000);
    }
}

#[test]
fn rate_limit_metadata_never_makes_an_unconfirmed_send_retryable() {
    let mut item = items("Progress", false).remove(0);
    send(
        &mut item,
        DeliveryOutcome::RateLimitedUnknown {
            not_before_ms: 10_000,
        },
    );
    assert!(matches!(
        item.state,
        DeliveryState::Unknown {
            reason: DeliveryFailure::RateLimited,
            ..
        }
    ));
    assert_eq!(
        item.claim("later-worker".into(), 10_001, 11_000, None),
        Err(OutboxError::UnknownOutcome)
    );
    assert_eq!(item.attempts.len(), 1);
}
