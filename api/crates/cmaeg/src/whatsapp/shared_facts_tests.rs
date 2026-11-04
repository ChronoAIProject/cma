use super::*;
use serde_json::json;

pub(crate) fn contact(id: &str) -> Value {
    json!({"from":"447700900123","id":id,"timestamp":"1800000000","type":"contacts",
        "contacts":[{"name":{"formatted_name":"Alice 李"},
            "phones":[{"phone":"+1 (650) 555-0123","wa_id":"999999","type":"WORK"}],
            "urls":[{"url":"http://127.0.0.1/do-not-fetch"}]}]})
}

pub(crate) fn place(id: &str) -> Value {
    json!({"from":"447700900123","id":id,"timestamp":"1800000000","type":"location",
        "location":{"latitude":1.3521,"longitude":103.8198,"name":"Meeting point",
            "address":"1 Example Road","url":"http://127.0.0.1/do-not-fetch"}})
}

#[test]
fn shared_facts_preserve_bounded_unicode_names_display_phones_and_coordinates_only() {
    let raw = contact("wamid.contact");
    assert_eq!(
        extract(&raw, "contacts").unwrap(),
        Content::Text {
            text: "Shared contacts:\n1. Name: \"Alice 李\"; Phone: \"+1 (650) 555-0123\"".into()
        }
    );
    let raw = place("wamid.place");
    assert_eq!(
        extract(&raw, "location").unwrap(),
        Content::Text { text: "Shared location:\nLatitude: 1.3521\nLongitude: 103.8198\nName: \"Meeting point\"\nAddress: \"1 Example Road\"".into() }
    );
    for latitude in [-90, 0, 90] {
        for longitude in [-180, 0, 180] {
            assert!(extract(
                &json!({"location":{"latitude":latitude,"longitude":longitude}}),
                "location"
            )
            .is_ok());
        }
    }
    assert!(extract(
        &json!({"contacts":[{"name":{"formatted_name":"Name only"}}]}),
        "contacts"
    )
    .is_ok());
    let contact = json!({"name":{"formatted_name":"x".repeat(256)},"phones":vec![json!({"phone":"1".repeat(80)});5]});
    assert!(extract(&json!({"contacts":vec![contact;10]}), "contacts").is_ok());
    assert!(extract(&json!({"location":{"latitude":90,"longitude":180,"name":"x".repeat(256),"address":"x".repeat(1024)}}), "location").is_ok());
}

#[test]
fn malformed_shared_facts_never_become_partial_messages() {
    for bad in [
        Value::Null,
        json!(true),
        json!("1"),
        json!([]),
        json!({}),
        json!(90.0001),
    ] {
        let mut raw = place("wamid.bad");
        raw["location"]["latitude"] = bad;
        assert_eq!(extract(&raw, "location"), Err(Error::Message));
    }
    for bad in [json!(-180.0001), json!(180.0001), json!(1e300)] {
        let mut raw = place("wamid.bad");
        raw["location"]["longitude"] = bad;
        assert_eq!(extract(&raw, "location"), Err(Error::Message));
    }
    for bad in [
        Value::Null,
        json!([]),
        json!(""),
        json!("  "),
        json!("A\nB"),
        json!("A\u{202e}B"),
        json!("x".repeat(257)),
    ] {
        let mut raw = contact("wamid.bad");
        raw["contacts"][0]["name"]["formatted_name"] = bad;
        assert_eq!(extract(&raw, "contacts"), Err(Error::Message));
    }
    for bad in [
        Value::Null,
        json!(123),
        json!("---"),
        json!("https://phone/123"),
        json!("123\n456"),
        json!("1".repeat(81)),
    ] {
        let mut raw = contact("wamid.bad");
        raw["contacts"][0]["phones"][0]["phone"] = bad;
        assert_eq!(extract(&raw, "contacts"), Err(Error::Message));
    }
    let mut raw = contact("wamid.bad");
    raw["contacts"][0]["phones"][0]
        .as_object_mut()
        .unwrap()
        .remove("phone");
    assert_eq!(extract(&raw, "contacts"), Err(Error::Message));
    for bad in [json!(""), json!(false), json!("x".repeat(1025))] {
        let mut raw = place("wamid.bad");
        raw["location"]["address"] = bad;
        assert_eq!(extract(&raw, "location"), Err(Error::Message));
    }
}

#[test]
fn contact_counts_and_total_rendered_bytes_are_bounded_without_truncation() {
    let contact = contact("wamid.count")["contacts"][0].clone();
    for contacts in [
        json!([]),
        json!({}),
        json!([{}]),
        json!(vec![contact.clone(); 11]),
    ] {
        assert_eq!(
            extract(&json!({"contacts":contacts}), "contacts"),
            Err(Error::Message)
        );
    }
    let mut raw = json!({"contacts":[contact]});
    for phones in [
        Value::Null,
        json!({}),
        json!(["123"]),
        json!(vec![json!({"phone":"123"}); 6]),
    ] {
        raw["contacts"][0]["phones"] = phones;
        assert_eq!(extract(&raw, "contacts"), Err(Error::Message));
    }
    let contact = json!({"name":{"formatted_name":"\\".repeat(256)},"phones":vec![json!({"phone":"1".repeat(80)});5]});
    assert_eq!(
        extract(&json!({"contacts":vec![contact;10]}), "contacts"),
        Err(Error::Message)
    );
    let raw = json!({"contacts":[{"name":{"formatted_name":"Alice\"; Phone: \"999"}}]});
    let Content::Text { text } = extract(&raw, "contacts").unwrap() else {
        panic!()
    };
    assert_eq!(
        text,
        "Shared contacts:\n1. Name: \"Alice\\\"; Phone: \\\"999\""
    );
}
