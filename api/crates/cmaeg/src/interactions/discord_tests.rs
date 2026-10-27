use super::discord::*;
use cmaeg_core::message::OutboundMessage;
use serde_json::{json, Value};
fn message(value: Value) -> OutboundMessage {
    serde_json::from_value(value).unwrap()
}
#[test]
fn native_discord_rows_and_single_selects_have_exact_bounds_and_opaque_values() {
    let options: Vec<_> = (0..8)
        .map(|i| json!({"id":format!("secret-{i}"),"label":format!("Choice {i}")}))
        .collect();
    let value = message(
        json!({"blocks":[{"type":"buttons","id":"buttons","options":options},{"type":"choice","id":"scope","prompt":"Choose","multi":false,"options":[{"id":"secret-select","label":"Selected"}]}]}),
    );
    let result = render(&value, "30d4d6fe-46f6-4e2d-b382-123456789abc", 1).unwrap();
    assert_eq!(result.as_array().unwrap().len(), 3);
    assert_eq!(result[0]["components"].as_array().unwrap().len(), 5);
    assert_eq!(result[1]["components"].as_array().unwrap().len(), 3);
    assert_eq!(result[2]["components"][0]["type"], 3);
    assert_eq!(result[2]["components"][0]["options"][0]["value"], "8");
    assert!(!result.to_string().contains("secret-"));
    let blocks: Vec<_> = (0..6).map(|i| json!({"type":"choice","id":format!("c{i}"),"prompt":"Choose","multi":false,"options":[{"id":"x","label":"X"}]})).collect();
    assert!(render(
        &message(json!({"blocks":blocks})),
        "30d4d6fe46f64e2db382123456789abc",
        1
    )
    .is_err());
    let mut multi = serde_json::to_value(value).unwrap();
    multi["blocks"][1]["multi"] = true.into();
    assert!(render(&message(multi), "30d4d6fe46f64e2db382123456789abc", 1).is_err());
}
