//! Golden bytes and digests of activities stored before attachments could carry
//! locators or media facts.
//!
//! A sealed event reference binds `activity_sha256` over the canonical activity,
//! reply targets and interaction cards re-parse stored activities for 24 hours,
//! and CMA and NyxBot keep `event_context` verbatim. A build that serialized any
//! of these shapes differently would make every outstanding reference fail
//! verification, so the values below were captured once from the commit before
//! the first optional attachment field was added and must never be regenerated.
use cmaeg_core::{activity::Activity, seal::event_ref::activity_digest};

struct Golden {
    name: &'static str,
    stored: &'static str,
    digest: &'static str,
}

const GOLDEN: [Golden; 5] = [
    Golden {
        name: "message without attachments",
        stored: r#"{"schema_version":1,"event_id":"telegram:123456:1001","kind":{"type":"message","text":"hello","attachments":[],"mentions_bot":false},"actor":{"id":"42","display_name":"Ada","kind":"human"},"conversation":{"id":"-100123","kind":"group","thread_id":"7"},"occurred_at":"2026-09-22T00:00:00Z","source":{"type":"telegram","platform":"telegram","bot_id":"123456"}}"#,
        digest: "a0b1787eb63bc58ef2a104b6e5668a483e0474337041aedbdcdd8715e8d7031a",
    },
    Golden {
        name: "message with one legacy attachment",
        stored: r#"{"schema_version":1,"event_id":"msg-1","kind":{"type":"message","text":"see attached","attachments":[{"reference":"telegram:AgADBAADr6cxG","kind":"image","filename":"photo.jpg","mime_type":"image/jpeg","size_bytes":20480}],"mentions_bot":true,"reply_to":"msg-0"},"actor":{"id":"42","display_name":"Ada","kind":"human"},"conversation":{"id":"-100123","kind":"group","thread_id":"7"},"occurred_at":"2026-09-22T00:00:00Z","source":{"type":"nyxid_relay","platform":"telegram","route_id":"route-1"}}"#,
        digest: "087946265b53ac8b90247b4144c5569dbd16d88a19a6584f6af5c996ae08c037",
    },
    Golden {
        name: "interaction",
        stored: r#"{"schema_version":1,"event_id":"telegram-callback:abc","kind":{"type":"interaction","block_id":"choice","action_id":"yes","values":{"label":"Proceed"}},"actor":{"id":"42","display_name":"Ada","kind":"human"},"conversation":{"id":"-100123","kind":"group","thread_id":"7"},"occurred_at":"2026-09-22T00:00:00Z","source":{"type":"nyxid_relay","platform":"telegram","route_id":"route-1"}}"#,
        digest: "8d22a6a9ae60cb5c7a57066172b317830c20fab7c1011419e6081a8338261363",
    },
    Golden {
        name: "command",
        stored: r#"{"schema_version":1,"event_id":"telegram:123456:1002","kind":{"type":"command","name":"help","args":"me"},"actor":{"id":"42","display_name":"Ada","kind":"human"},"conversation":{"id":"-100123","kind":"group","thread_id":"7"},"occurred_at":"2026-09-22T00:00:00Z","source":{"type":"telegram","platform":"telegram","bot_id":"123456"}}"#,
        digest: "bf44b62e3a29cb9faa66dff823eeb278f1db89766a4d7cec5a5fc95689902b1b",
    },
    Golden {
        name: "custom event",
        stored: r#"{"schema_version":1,"event_id":"event-1","kind":{"type":"event","ce":{"specversion":"1.0","id":"event-1","source":"urn:test:source","type":"com.example.test","time":"2026-09-22T00:00:00Z","subject":"order-9","datacontenttype":"application/json","data":{"value":1},"traceid":"abc123"}},"actor":{"id":"urn:test:source","kind":"system"},"conversation":{"id":"urn:test:source","kind":"custom"},"occurred_at":"2026-09-22T00:00:00Z","source":{"type":"custom"}}"#,
        digest: "a8f5093399cc41353ffb365560dcaf3e99550657f0a78706d0f0bd3af73db3c2",
    },
];

#[test]
fn stored_activities_keep_their_exact_bytes_and_digests() {
    for golden in GOLDEN {
        let activity: Activity = serde_json::from_str(golden.stored).unwrap_or_else(|error| {
            panic!("{}: stored activity no longer parses: {error}", golden.name)
        });
        activity.validate().unwrap_or_else(|error| {
            panic!(
                "{}: stored activity no longer validates: {error}",
                golden.name
            )
        });
        assert_eq!(
            serde_json::to_string(&activity).unwrap(),
            golden.stored,
            "{}: serialized bytes changed",
            golden.name
        );
        assert_eq!(
            activity_digest(&activity),
            golden.digest,
            "{}: activity digest changed",
            golden.name
        );
    }
}

#[test]
fn a_changed_activity_does_not_match_its_golden_digest() {
    // Guards the guard: the digest must actually depend on the stored content.
    let mut activity: Activity = serde_json::from_str(GOLDEN[0].stored).unwrap();
    activity.occurred_at = "2026-09-22T00:00:01Z".into();
    assert_ne!(activity_digest(&activity), GOLDEN[0].digest);
}
