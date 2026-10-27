use super::*;
use serde_json::json;

fn envelope(statuses: Vec<Value>) -> Vec<u8> {
    serde_json::to_vec(&json!({"object":"whatsapp_business_account","entry":[{
        "id":"23456","changes":[{"field":"messages","value":{
            "messaging_product":"whatsapp","metadata":{"phone_number_id":"34567"},
            "statuses":statuses}}]}]}))
    .unwrap()
}
fn status(kind: &str) -> Value {
    json!({"id":"wamid.reply","status":kind,"timestamp":"1800000000",
        "recipient_id":"447700900123"})
}

#[test]
fn official_statuses_keep_private_group_and_participant_facts_separate() {
    let mut group = status("read");
    group["recipient_type"] = "group".into();
    group["recipient_id"] = "1234567890123456".into();
    group["recipient_participant_id"] = "447700900999".into();
    let result = extract(
        &super::super::tests::source(),
        &envelope(vec![status("sent"), group]),
    )
    .unwrap();
    assert_eq!(result.len(), 2);
    assert!(!result[0].group);
    assert_eq!(result[0].kind, Kind::Sent);
    assert!(result[1].group);
    assert_eq!(result[1].recipient, "1234567890123456");
    assert_eq!(result[1].participant.as_deref(), Some("447700900999"));
    for kind in ["sent", "delivered", "read", "played", "failed"] {
        assert_eq!(
            extract(
                &super::super::tests::source(),
                &envelope(vec![status(kind)])
            )
            .unwrap()[0]
                .kind
                .as_str(),
            kind
        );
    }
}

#[test]
fn statuses_require_exact_source_and_valid_identity_without_copying_provider_error_text() {
    let source = super::super::tests::source();
    let mut body: Value = serde_json::from_slice(&envelope(vec![status("failed")])).unwrap();
    body["entry"][0]["changes"][0]["value"]["statuses"][0]["errors"] = json!([
        {"code":131049,"message":"private provider details","href":"https://untrusted.invalid"}]);
    let extracted = extract(&source, &serde_json::to_vec(&body).unwrap()).unwrap();
    assert!(!format!("{extracted:?}").contains("private provider details"));
    body["entry"][0]["changes"][0]["value"]["metadata"]["phone_number_id"] = "99999".into();
    assert!(extract(&source, &serde_json::to_vec(&body).unwrap())
        .unwrap()
        .is_empty());
    for (field, value) in [
        ("id", json!("../message")),
        ("recipient_id", json!("https://other.invalid")),
        ("timestamp", json!(0)),
        ("timestamp", json!("-1")),
        ("recipient_type", json!("broadcast")),
        ("recipient_participant_id", json!("447700900999")),
    ] {
        let mut raw = status("read");
        raw[field] = value;
        assert!(extract(&source, &envelope(vec![raw])).is_err(), "{field}");
    }
}

#[test]
fn callback_duplicates_and_unknown_types_are_bounded() {
    let source = super::super::tests::source();
    assert_eq!(
        extract(&source, &envelope(vec![status("read"); 100]))
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        extract(&source, &envelope(vec![status("read"); 101])),
        Err(Error::TooMany)
    );
    assert!(extract(&source, &envelope(vec![status("future_status")]))
        .unwrap()
        .is_empty());
    let mut malformed: Value = serde_json::from_slice(&envelope(vec![])).unwrap();
    malformed["entry"][0]["changes"][0]["value"]["statuses"] = json!({});
    assert!(extract(&source, &serde_json::to_vec(&malformed).unwrap()).is_err());
}
