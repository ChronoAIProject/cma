use super::*;
use crate::native_lark::test_support::{envelope, identity};
use serde_json::json;
const NOW: i64 = 1_800_000_000;

// Official SDK db9f52d0a88e3f5e4ed81bb34d71a28e3f5608fa:
// P2MessageReadV1Data + EventMessageReader; see CMAEG_Native_Lark_Read_Status.md.
fn event() -> Value {
    envelope(
        EVENT_TYPE,
        "read_event",
        json!({
            "reader":{"reader_id":{"open_id":"ou_human"},"tenant_key":identity().tenant_key,
                "read_time":(NOW * 1000 + 123).to_string()},
            "message_id_list":["om_first","om_second","om_first"]
        }),
    )
}

#[test]
fn exact_reader_and_message_ids_preserve_milliseconds_and_deduplicate() {
    let read = extract(&identity(), &event(), NOW).unwrap();
    assert_eq!(read.reader, "ou_human");
    assert_eq!(read.occurred_ms, NOW * 1000 + 123);
    assert_eq!(read.message_ids, ["om_first", "om_second"]);
}

#[test]
fn malformed_cross_tenant_and_unbounded_batches_are_refused_wholly() {
    for (pointer, value) in [
        ("/schema", json!("1.0")),
        ("/header/event_type", json!("im.message.receive_v1")),
        ("/header/event_id", json!("")),
        ("/header/app_id", json!("cli_other")),
        ("/header/tenant_key", json!("other")),
        ("/event/reader/tenant_key", json!("other")),
        (
            "/event/reader/reader_id/open_id",
            json!(identity().bot_open_id),
        ),
        (
            "/event/reader/reader_id/open_id",
            json!("user_id_is_not_open_id"),
        ),
        ("/event/reader/reader_id", json!({"user_id":"ou_human"})),
        ("/event/reader/read_time", json!(NOW * 1000)),
        ("/event/reader/read_time", json!(format!("0{}", NOW * 1000))),
        ("/event/reader/read_time", json!(i64::MAX.to_string())),
        (
            "/event/reader/read_time",
            json!(((NOW + 301) * 1000).to_string()),
        ),
        (
            "/event/reader/read_time",
            json!(((NOW - super::super::receipts::RETENTION) * 1000).to_string()),
        ),
        ("/event/message_id_list", json!([])),
        ("/event/message_id_list", json!(["om_valid", "bad/message"])),
        (
            "/event/message_id_list",
            json!(vec!["om_one"; MAX_MESSAGES + 1]),
        ),
    ] {
        let mut raw = event();
        *raw.pointer_mut(pointer).unwrap() = value;
        assert!(extract(&identity(), &raw, NOW).is_err(), "{pointer}");
    }
}
