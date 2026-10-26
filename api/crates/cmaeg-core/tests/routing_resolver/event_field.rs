//! Custom event targets from the `cmaegtargets` attribute, in structured and
//! binary mode, with its signed-attribute rule and its bounds.
use cmaeg_core::{
    activity::{Activity, ActivityKind},
    channel::ChannelDefinition,
    cloudevent::{parse_binary, parse_structured},
    routing::{event_field::read, RouteError},
};
use serde_json::{json, Value};

use super::{
    fixture::{channel, event, input, unrouted},
    policy::{hash, NYX, REVIEWER},
};

type Read = Result<Option<Vec<String>>, RouteError>;

fn targets(def: &ChannelDefinition, activity: &Activity, signed: bool) -> Read {
    let mut input = input(def, activity);
    input.attributes_signed = signed;
    read(def, &input).map(|ids| ids.map(|ids| ids.iter().map(|id| id.to_string()).collect()))
}

fn named(value: Value) -> Activity {
    event("build.done", json!({"cmaegtargets": value}))
}

fn routed() -> ChannelDefinition {
    channel(|_| {})
}

fn found(ids: &[&str]) -> Read {
    Ok(Some(ids.iter().map(|id| (*id).to_owned()).collect()))
}

/// The activity `event` builds, carrying `ce` instead.
fn carrying(ce: cmaeg_core::cloudevent::CloudEvent) -> Activity {
    let mut activity = event("build.done", json!({}));
    activity.kind = ActivityKind::Event {
        ce,
        attachments: Vec::new(),
    };
    activity
}

fn binary(targets: &str) -> Activity {
    let headers: Vec<(String, String)> = [
        ("ce-specversion", "1.0"),
        ("ce-id", "ce-1"),
        ("ce-source", "https://ci.example/build"),
        ("ce-type", "build.done"),
        ("ce-cmaegtargets", targets),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_owned(), value.to_owned()))
    .collect();
    carrying(parse_binary(&headers, b"").unwrap())
}

#[test]
fn structured_mode_names_targets_in_order_without_duplicates() {
    let body = json!({"specversion": "1.0", "id": "ce-1", "source": "https://ci.example/build",
        "type": "build.done", "cmaegtargets": " reviewer , main,REVIEWER,nyx"});
    let activity = carrying(parse_structured(body.to_string().as_bytes()).unwrap());
    let read = targets(&routed(), &activity, true);
    assert_eq!(read, found(&[REVIEWER, "default", NYX]));
    let by_id = named(json!(format!("{NYX},default")));
    assert_eq!(targets(&routed(), &by_id, true), found(&[NYX, "default"]));
}

#[test]
fn binary_mode_is_read_only_when_its_attributes_are_signed() {
    // A nyxid_key source authenticates the request itself, so the caller
    // marks its binary attributes signed.
    let keyed = channel(|value| {
        value["sources"][2]["auth"] =
            json!({"type": "nyxid_key", "key_hashes": [hash("producer")]});
    });
    assert_eq!(
        targets(&keyed, &binary("reviewer"), true),
        found(&[REVIEWER])
    );
    // HMAC signs only the body, so binary headers could have been swapped.
    let refused = targets(&routed(), &binary("reviewer"), false);
    assert_eq!(refused, Err(RouteError::TargetsUnsigned));
    // An event that names no targets needs no signature.
    let plain = event("build.done", json!({}));
    assert_eq!(targets(&routed(), &plain, false), Ok(None));
}

#[test]
fn malformed_values_are_refused() {
    let def = routed();
    for value in [json!(7), json!(["reviewer"]), json!(null), json!({"a": 1})] {
        let refused = targets(&def, &named(value.clone()), true);
        assert_eq!(refused, Err(RouteError::InvalidTargets), "{value}");
    }
    for value in [
        "",
        " ",
        "reviewer,,main",
        "reviewer,",
        ",main",
        "reviewer, ,main",
    ] {
        let refused = targets(&def, &named(json!(value)), true);
        assert_eq!(refused, Err(RouteError::InvalidTargets), "{value:?}");
    }
    let long = format!("reviewer,{}", "x".repeat(248));
    assert_eq!(long.len(), 257);
    let refused = targets(&def, &named(json!(long)), true);
    assert_eq!(refused, Err(RouteError::InvalidTargets));
}

#[test]
fn more_tokens_than_max_targets_are_refused() {
    let nine = ["main"; 9].join(",");
    let refused = targets(&routed(), &named(json!(nine)), true);
    assert_eq!(refused, Err(RouteError::TooManyTargets));
    let def = channel(|value| value["routing"]["max_targets"] = 2.into());
    let three = named(json!("reviewer,nyx,main"));
    assert_eq!(targets(&def, &three, true), Err(RouteError::TooManyTargets));
    let two = named(json!("reviewer,nyx"));
    assert_eq!(targets(&def, &two, true), found(&[REVIEWER, NYX]));
}

#[test]
fn the_attribute_is_refused_where_the_channel_turns_it_off() {
    let def = channel(|value| value["routing"]["addressing"] = json!({"event_field": false}));
    for value in [json!("reviewer"), json!(7)] {
        let refused = targets(&def, &named(value), true);
        assert_eq!(refused, Err(RouteError::TargetsNotPermitted));
    }
    assert_eq!(
        targets(&def, &event("build.done", json!({})), true),
        Ok(None)
    );
}

#[test]
fn unknown_and_pending_names_are_unknown() {
    let refused = targets(&routed(), &named(json!("reviewer,ghost")), true);
    assert_eq!(refused, Err(RouteError::TargetUnknown("ghost".into())));
    let pending = channel(|value| value["routing"]["consumers"][1]["pending"] = true.into());
    let refused = targets(&pending, &named(json!("nyx")), true);
    assert_eq!(refused, Err(RouteError::TargetUnknown("nyx".into())));
    let refused = targets(&pending, &named(json!(NYX)), true);
    assert_eq!(refused, Err(RouteError::TargetUnknown(NYX.into())));
}

#[test]
fn mentionable_does_not_restrict_authenticated_producers() {
    let quiet = channel(|value| value["routing"]["consumers"][0]["mentionable"] = false.into());
    let read = targets(&quiet, &named(json!("reviewer")), true);
    assert_eq!(read, found(&[REVIEWER]));
}

#[test]
fn unrouted_channels_and_other_kinds_ignore_the_attribute() {
    assert_eq!(
        targets(&unrouted(), &named(json!("reviewer")), false),
        Ok(None)
    );
    assert_eq!(targets(&unrouted(), &named(json!(7)), true), Ok(None));
    let message = super::fixture::message(super::fixture::telegram(), "cmaegtargets: reviewer");
    assert_eq!(targets(&routed(), &message, true), Ok(None));
}
