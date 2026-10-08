//! Service serialization and SDK examples agree without changing legacy bytes.
use cmaeg::{
    http::{
        callbacks::CallbackResponse,
        consumer_admission::{AdmissionStatus, ConsumerAdmission},
        events::{AcceptedStatus, EventAccepted},
    },
    ingress::admission::Refusal,
};
use cmaeg_core::ids::{ConsumerId, ConsumerName, ConversationId, EventId};
use serde_json::{json, Value};

fn examples() -> Value {
    serde_json::from_str(include_str!("fixtures/sdk-contract.json")).unwrap()
}
fn consumers() -> Vec<ConsumerAdmission> {
    [
        ("default", "main", "conv_main", AdmissionStatus::Accepted),
        (
            "csm_000000000001",
            "helper",
            "conv_helper",
            AdmissionStatus::Deferred,
        ),
        (
            "csm_000000000002",
            "reviewer",
            "conv_reviewer",
            AdmissionStatus::Refused,
        ),
    ]
    .into_iter()
    .map(|(id, name, conversation, status)| ConsumerAdmission {
        consumer_id: ConsumerId::parse(id).unwrap(),
        name: ConsumerName::parse(name).unwrap(),
        conversation_id: ConversationId::new(conversation).unwrap(),
        status,
        code: (status == AdmissionStatus::Refused).then(|| "provider_rejected".into()),
        provider_code: (status == AdmissionStatus::Refused).then(|| "policy_denied".into()),
    })
    .collect()
}

#[test]
fn routed_responses_preserve_each_targets_status_and_optional_refusal_codes() {
    let expected = json!({"status":"accepted","event_id":"routed-event","conversation_id":"conv_main",
    "consumers":[
        {"consumer_id":"default","name":"main","conversation_id":"conv_main","status":"accepted"},
        {"consumer_id":"csm_000000000001","name":"helper","conversation_id":"conv_helper","status":"deferred"},
        {"consumer_id":"csm_000000000002","name":"reviewer","conversation_id":"conv_reviewer","status":"refused","code":"provider_rejected","provider_code":"policy_denied"}
    ]});
    let event = EventAccepted {
        status: AcceptedStatus::Accepted,
        event_id: EventId::new("routed-event").unwrap(),
        conversation_id: ConversationId::new("conv_main").unwrap(),
        consumers: Some(consumers()),
    };
    let callback = CallbackResponse::Accepted {
        event_id: "routed-event".into(),
        conversation_id: "conv_main".into(),
        consumers: Some(consumers()),
    };
    assert_eq!(serde_json::to_value(event).unwrap(), expected);
    assert_eq!(serde_json::to_value(callback).unwrap(), expected);
    assert_eq!(expected["consumers"][0].get("code"), None);
    assert_eq!(expected["consumers"][1].get("provider_code"), None);
    assert_eq!(expected["consumers"][2]["provider_code"], "policy_denied");
}

#[test]
fn legacy_acceptance_omits_consumers_instead_of_serializing_null() {
    let event = EventAccepted {
        status: AcceptedStatus::Accepted,
        event_id: EventId::new("legacy-event").unwrap(),
        conversation_id: ConversationId::new("conv_main").unwrap(),
        consumers: None,
    };
    let callback = CallbackResponse::Accepted {
        event_id: "legacy-event".into(),
        conversation_id: "conv_main".into(),
        consumers: None,
    };
    let expected =
        json!({"status":"accepted","event_id":"legacy-event","conversation_id":"conv_main"});
    assert_eq!(serde_json::to_value(event).unwrap(), expected);
    assert_eq!(serde_json::to_value(callback).unwrap(), expected);
}

#[test]
fn routing_refusals_match_the_shared_closed_enum_fixtures() {
    let examples = examples();
    for (reason, wire) in [
        (Refusal::TargetUnknown, "target_unknown"),
        (Refusal::TargetNotMentionable, "target_not_mentionable"),
        (Refusal::TooManyTargets, "too_many_targets"),
        (Refusal::ConsumerUnavailable, "consumer_unavailable"),
    ] {
        let value = serde_json::to_value(CallbackResponse::Refused { reason }).unwrap();
        assert_eq!(value, json!({"status":"refused","reason":wire}));
        assert!(examples["CallbackResponse"]
            .as_array()
            .unwrap()
            .contains(&value));
        assert!(examples["Refusal"]
            .as_array()
            .unwrap()
            .contains(&json!(wire)));
    }
}

#[test]
fn maximal_schema_example_matches_service_serialization() {
    // The shared corpus requires every optional property in each example.
    // This proves its shape; mixed admission behavior is exercised above.
    let refused = consumers().pop().unwrap();
    assert_eq!(
        serde_json::to_value(&refused).unwrap(),
        examples()["ConsumerAdmission"][0]
    );
    let value = EventAccepted {
        status: AcceptedStatus::Accepted,
        event_id: EventId::new("routed-event").unwrap(),
        conversation_id: ConversationId::new("conv_main").unwrap(),
        consumers: Some(vec![refused]),
    };
    assert_eq!(
        serde_json::to_value(value).unwrap(),
        examples()["EventAccepted"][1]
    );
}
