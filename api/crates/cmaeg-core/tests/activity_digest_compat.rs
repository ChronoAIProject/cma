//! Golden bytes and digests of stored activities, pinned so that a new
//! optional field (attachment locators and media facts, then routing) never
//! changes an activity that does not carry it.
//!
//! A sealed event reference binds `activity_sha256` over the canonical activity,
//! reply targets and interaction cards re-parse stored activities for 24 hours,
//! and CMA and NyxBot keep `event_context` verbatim. A build that serialized any
//! of these shapes differently would make every outstanding reference fail
//! verification, so the `GOLDEN` values below were captured once from the
//! commit before the first optional attachment field was added, the `SOURCES`
//! values from the commit before `Activity.routing` was added, and neither set
//! may ever be regenerated.
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

/// One activity per source and kind, captured on the parent commit of the
/// change that added `Activity.routing`, so that an activity without routing
/// is proven to keep its exact bytes and digest on every source: native
/// Telegram, each NyxID relay platform, custom events with and without
/// attachments, interactions and commands. Like `GOLDEN`, these values are a
/// reviewed contract and are never regenerated to make a test pass.
const SOURCES: [Golden; 10] = [
    Golden {
        name: "native Telegram message with media facts",
        stored: r#"{"schema_version":1,"event_id":"telegram:123456:1003","kind":{"type":"message","text":"look","attachments":[{"reference":"telegram:AQADxyz","kind":"image","mime_type":"image/jpeg","size_bytes":20480,"source":{"type":"telegram","file_id":"AgACAgQAAxkBAAIB","file_unique_id":"AQADxyz"},"variant":"photo","width":1280,"height":720,"thumbnail":{"type":"telegram","file_id":"AAMCAgADGQEAAgE","file_unique_id":"AQADthumb"}}],"mentions_bot":true,"reply_to":"1002","album_id":"13579"},"actor":{"id":"42","display_name":"Ada","kind":"human"},"conversation":{"id":"-100123","kind":"group","thread_id":"7"},"occurred_at":"2026-09-22T00:00:00Z","source":{"type":"telegram","platform":"telegram","bot_id":"123456"}}"#,
        digest: "691c82d358a75643d440d4e05cf2867c044e6854e18631b3dc4b81c3e09dbe78",
    },
    Golden {
        name: "native Telegram interaction",
        stored: r#"{"schema_version":1,"event_id":"telegram-callback:native-1","kind":{"type":"interaction","block_id":"choice","action_id":"no","values":{"selected":["no"]}},"actor":{"id":"42","display_name":"Ada","kind":"human"},"conversation":{"id":"42","kind":"private"},"occurred_at":"2026-09-22T00:00:00Z","source":{"type":"telegram","platform":"telegram","bot_id":"123456"}}"#,
        digest: "ba5e3924b3b35d075804b1ba633fe8cc68602c9785f26396e0cdbb797a6cd031",
    },
    Golden {
        name: "relay telegram message with a relay locator",
        stored: r#"{"schema_version":1,"event_id":"message-2","kind":{"type":"message","text":"voice","attachments":[{"reference":"telegram:item-0","kind":"audio","mime_type":"audio/ogg","source":{"type":"nyxid_relay","index":0},"variant":"voice","duration_seconds":3}],"mentions_bot":false},"actor":{"id":"42","display_name":"Ada","kind":"human"},"conversation":{"id":"42","kind":"private"},"occurred_at":"2026-09-22T00:00:00Z","source":{"type":"nyxid_relay","platform":"telegram","route_id":"route-1"}}"#,
        digest: "a11bd134e860196dbe354bf7afc981110d7172ee360108334bc482cce60bdb05",
    },
    Golden {
        name: "relay lark message",
        stored: r#"{"schema_version":1,"event_id":"om_lark_message","kind":{"type":"message","text":"hello lark","attachments":[],"mentions_bot":true},"actor":{"id":"ou_sender","display_name":"Lin","kind":"human"},"conversation":{"id":"oc_chat","kind":"group","thread_id":"omt_thread"},"occurred_at":"2026-09-22T00:00:00Z","source":{"type":"nyxid_relay","platform":"lark","route_id":"route-lark"}}"#,
        digest: "cd289f94597c1bf6c04a9ac4d013952ea0f32455a3cd7d491feebbb7aea90f8b",
    },
    Golden {
        name: "relay feishu message",
        stored: r#"{"schema_version":1,"event_id":"om_feishu_message","kind":{"type":"message","text":"hello feishu","attachments":[{"reference":"feishu:img_v3_key","kind":"image","source":{"type":"nyxid_relay","index":1},"variant":"image"}],"mentions_bot":false},"actor":{"id":"ou_feishu","kind":"human"},"conversation":{"id":"oc_feishu","kind":"private"},"occurred_at":"2026-09-22T00:00:00Z","source":{"type":"nyxid_relay","platform":"feishu","route_id":"route-feishu"}}"#,
        digest: "9b58a12fc9600d7d4719228efee1edf9023193a8a613da8c84f5ae50fc951abe",
    },
    Golden {
        name: "relay discord message",
        stored: r#"{"schema_version":1,"event_id":"1180000000000000001","kind":{"type":"message","text":"hello discord","attachments":[],"mentions_bot":true},"actor":{"id":"1170000000000000001","display_name":"dee","kind":"human"},"conversation":{"id":"1160000000000000001","kind":"channel"},"occurred_at":"2026-09-22T00:00:00Z","source":{"type":"nyxid_relay","platform":"discord","route_id":"route-discord"}}"#,
        digest: "59087678aef85d9a39a67dbaa04164fa9c5e5e40b76c3b134017059b1102e90d",
    },
    Golden {
        name: "relay slack message",
        stored: r#"{"schema_version":1,"event_id":"1700000000.000100","kind":{"type":"message","text":"hello slack","attachments":[{"reference":"slack:F012345","kind":"file","filename":"notes.txt","mime_type":"text/plain","size_bytes":12,"variant":"file"}],"mentions_bot":false},"actor":{"id":"U012345","kind":"human"},"conversation":{"id":"C012345","kind":"channel","thread_id":"1700000000.000001"},"occurred_at":"2026-09-22T00:00:00Z","source":{"type":"nyxid_relay","platform":"slack","route_id":"route-slack"}}"#,
        digest: "dd2a9313fd5d78018594defb2b00939806db7e9129c0d3f1248ba66da72b5afa",
    },
    Golden {
        name: "relay whatsapp message",
        stored: r#"{"schema_version":1,"event_id":"wamid.HBgLMTIzNDU2Nzg5MBUCABEYEg","kind":{"type":"message","text":"hello whatsapp","attachments":[],"mentions_bot":false},"actor":{"id":"15550001111","display_name":"Wes","kind":"human"},"conversation":{"id":"15550001111","kind":"private"},"occurred_at":"2026-09-22T00:00:00Z","source":{"type":"nyxid_relay","platform":"whatsapp","route_id":"route-whatsapp"}}"#,
        digest: "67e49fe67f6a681120ee46d3b631fe5557dfdabc1ffa1bbb06ce5b453dc442d0",
    },
    Golden {
        name: "relay command",
        stored: r#"{"schema_version":1,"event_id":"om_lark_command","kind":{"type":"command","name":"to","args":"reviewer please check"},"actor":{"id":"ou_sender","kind":"human"},"conversation":{"id":"oc_chat","kind":"group"},"occurred_at":"2026-09-22T00:00:00Z","source":{"type":"nyxid_relay","platform":"lark","route_id":"route-lark"}}"#,
        digest: "9e85fd27a4b3759632e116ac45326365a4801e300f11161c67bf064960543d60",
    },
    Golden {
        name: "custom event with attachments",
        stored: r#"{"schema_version":1,"event_id":"event-2","kind":{"type":"event","ce":{"specversion":"1.0","id":"event-2","source":"urn:test:source","type":"com.example.upload","time":"2026-09-22T00:00:00Z","subject":"order-10","datacontenttype":"application/json","data":{"file":{"url":"https://files.example/report.pdf"},"inline":"aGVsbG8="}},"attachments":[{"reference":"report","kind":"file","filename":"report.pdf","mime_type":"application/pdf","source":{"type":"url","url":"https://files.example/report.pdf"}},{"reference":"inline-0","kind":"other","source":{"type":"inline","pointer":"/inline"}}]},"actor":{"id":"urn:test:source","kind":"system"},"conversation":{"id":"order-10","kind":"custom"},"occurred_at":"2026-09-22T00:00:00Z","source":{"type":"custom"}}"#,
        digest: "ae8f1034f74ae343baee6ffd809f840bb4dd8ab4f020dbb9bdc1e321115b82a5",
    },
];

#[test]
fn stored_activities_keep_their_exact_bytes_and_digests() {
    for golden in GOLDEN.iter().chain(SOURCES.iter()) {
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
