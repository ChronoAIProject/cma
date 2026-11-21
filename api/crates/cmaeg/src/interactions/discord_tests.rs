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
    let result = render(&message(multi), "30d4d6fe46f64e2db382123456789abc", 1).unwrap();
    assert_eq!(result[2]["components"][0]["min_values"], 0);
    assert_eq!(result[2]["components"][0]["max_values"], 1);
}

#[test]
fn native_discord_multiselect_keeps_neutral_bounds_and_private_option_ids() {
    let options: Vec<_> = (0..20)
        .map(|i| json!({"id":format!("private-{i}"),"label":format!("Option {i}"),"selected":i<2}))
        .collect();
    let value = json!({"blocks":[{"type":"choice","id":"scope","prompt":"Choose","multi":true,"options":options}]});
    let markup = render(
        &message(value.clone()),
        "30d4d6fe46f64e2db382123456789abc",
        1,
    )
    .unwrap();
    let select = &markup[0]["components"][0];
    assert_eq!(select["min_values"], 0);
    assert_eq!(select["max_values"], 20);
    assert_eq!(select["options"][19]["value"], "19");
    assert_eq!(select["options"][1]["default"], true);
    assert!(!markup.to_string().contains("private-"));
    let mut excessive = value.clone();
    excessive["blocks"][0]["options"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":"extra","label":"Extra"}));
    assert!(render(&message(excessive), "30d4d6fe46f64e2db382123456789abc", 1).is_err());
    let mut single = value;
    single["blocks"][0]["multi"] = false.into();
    assert!(render(&message(single), "30d4d6fe46f64e2db382123456789abc", 1).is_err());
}

#[test]
fn historical_click_serialization_is_unchanged_and_empty_selection_is_explicit() {
    let historical = r#"{"event_id":"event","callback_id":"callback","action":0,"occurred_at":"2026-10-27T00:00:00Z"}"#;
    let mut click: super::card::Click = serde_json::from_str(historical).unwrap();
    assert_eq!(serde_json::to_string(&click).unwrap(), historical);
    click.selection = Some(vec![]);
    assert_eq!(
        serde_json::to_value(&click).unwrap()["selection"],
        json!([])
    );
}
