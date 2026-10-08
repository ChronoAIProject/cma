use super::{
    fanout_support::*,
    routing_fixture::{self, REVIEWER},
};
use cmaeg::{
    ingress::launch::{LaunchOutcome, TurnLauncher},
    turns::{
        render_input,
        render_media::{Audience, RenderMedia},
    },
};
use cmaeg_core::{
    activity::{Activity, ActivityKind, SourceKind},
    channel::{ChannelDefinition, Platform},
    ids::ConsumerId,
    routing::{resolve, Resolution, RouteInput},
};
use serde_json::json;
use std::{collections::BTreeSet, sync::Arc, time::Duration};

fn definition() -> ChannelDefinition {
    let mut value = routing_fixture::routed();
    value["routing"]["default_consumer"]["name"] = "main".into();
    value["sources"] = json!([
        routing_fixture::relay("lark", routing_fixture::ROUTE),
        routing_fixture::relay("telegram", "route-telegram")
    ]);
    routing_fixture::definition(value)
}
fn activity(text: &str) -> Activity {
    let mut activity: Activity = serde_json::from_str(include_str!(
        "../../../cmaeg-core/tests/fixtures/activity.json"
    ))
    .unwrap();
    activity.source = SourceKind::NyxidRelay {
        platform: Platform::Telegram,
        route_id: "route-telegram".into(),
    };
    activity.kind = ActivityKind::Message {
        text: text.into(),
        mentions_bot: true,
        attachments: vec![],
        reply_to: None,
        album_id: None,
    };
    activity
}
fn routed(mut activity: Activity, def: &ChannelDefinition) -> Activity {
    let input = RouteInput {
        activity: &activity,
        chat_source: def.source_of(&activity.source),
        card_consumer: None,
        published: None,
        non_human_providers: &BTreeSet::new(),
        attributes_signed: true,
    };
    let Resolution::Routed(decision) = resolve(def, &input, true).unwrap() else {
        panic!()
    };
    activity.routing = Some(decision.routing);
    activity.validate().unwrap();
    activity
}
fn text(
    activity: &Activity,
    definition: &ChannelDefinition,
    consumer: Option<&ConsumerId>,
) -> String {
    render_input::render(
        activity,
        definition,
        &RenderMedia::empty(None),
        Audience {
            tools: false,
            downloads: false,
        },
        consumer,
    )[0]["content"][0]["text"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[test]
fn routed_addressing_uses_its_source_and_retains_utf8_remainder() {
    let def = definition();
    let reviewer = ConsumerId::parse(REVIEWER).unwrap();
    for raw in [
        "@telegram_bot @main @reviewer 你好",
        "@telegram_bot /to main,reviewer 你好",
    ] {
        let activity = routed(activity(raw), &def);
        assert_eq!(
            text(&activity, &def, Some(&reviewer)),
            "[Routed to @main and @reviewer; you are @reviewer]\n你好"
        );
        assert_eq!(
            text(&activity, &def, None),
            "[Routed to @main and @reviewer; you are @main]\n你好"
        );
    }
    let own_bot = routed(activity("@telegram_bot hi"), &def);
    assert_eq!(own_bot.routing.as_ref().unwrap().addressing_bytes, 0);
    assert_eq!(text(&own_bot, &def, None), "hi");
    let mut renamed = def.clone();
    renamed.routing.as_mut().unwrap().consumers[0].name =
        cmaeg_core::ids::ConsumerName::parse("renamed").unwrap();
    let activity = routed(activity("/to main,reviewer x"), &def);
    assert!(text(&activity, &renamed, Some(&reviewer)).contains("you are @reviewer"));
}

#[test]
fn bare_consumer_mention_renders_like_a_bare_legacy_bot_address() {
    let def = definition();
    let reviewer = ConsumerId::parse(REVIEWER).unwrap();
    let routed = routed(activity("@reviewer"), &def);
    assert_eq!(
        text(&routed, &def, Some(&reviewer)),
        text(&activity("@telegram_bot"), &def, None)
    );
    assert_eq!(text(&routed, &def, Some(&reviewer)), "");
}

#[test]
fn routing_selects_its_own_bot_even_with_the_legacy_source_layout() {
    let mut value = routing_fixture::routed();
    value["sources"] = json!([
        routing_fixture::telegram(),
        routing_fixture::relay("telegram", "route-telegram")
    ]);
    let def = routing_fixture::definition(value);
    assert!(!def.has_extended_sources());
    let legacy = activity("@telegram_bot hi @golden_bot");
    let resolved = routed(legacy.clone(), &def);
    assert_eq!(resolved.routing.as_ref().unwrap().addressing_bytes, 0);
    assert_eq!(text(&resolved, &def, None), "hi @golden_bot");
    assert_eq!(text(&legacy, &def, None), "@telegram_bot hi");
}

#[test]
fn only_resolved_to_commands_remove_target_arguments() {
    let def = definition();
    let mut command = activity("");
    command.kind = ActivityKind::Command {
        name: "to".into(),
        args: "reviewer résumé".into(),
    };
    let resolved = routed(command.clone(), &def);
    assert_eq!(
        text(&resolved, &def, Some(&ConsumerId::parse(REVIEWER).unwrap())),
        "résumé"
    );
    assert_eq!(text(&command, &def, None), "/to reviewer résumé");
    command.kind = ActivityKind::Command {
        name: "help".into(),
        args: "reviewer".into(),
    };
    assert_eq!(text(&routed(command, &def), &def, None), "/help reviewer");
}

#[tokio::test]
async fn providers_receive_consumer_metadata_only_for_routed_turns() {
    let mut h = Harness::new().await;
    assert!(matches!(
        h.base
            .runner
            .launch(h.base.request("legacy-metadata"))
            .await,
        LaunchOutcome::Accepted { .. }
    ));
    let outcome = h
        .wrap(Arc::new(h.base.runner.clone()), Duration::from_secs(2))
        .launch_routed(h.request(&["default", REVIEWER]), None)
        .await;
    assert!(outcome
        .consumers
        .iter()
        .all(|c| matches!(c.outcome, LaunchOutcome::Accepted { .. })));
    let requests = h.base.fake.requests();
    let bodies = requests
        .iter()
        .filter(|r| r.method == "POST")
        .map(|r| serde_json::from_str::<serde_json::Value>(&r.body).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(bodies.len(), 3);
    assert!(bodies[0]["metadata"].get("cmaeg.consumer").is_none());
    let consumers = bodies[1..]
        .iter()
        .map(|b| b["metadata"]["cmaeg.consumer"].as_str().unwrap())
        .collect::<BTreeSet<_>>();
    assert_eq!(consumers, BTreeSet::from(["default", REVIEWER]));
    while !h.base.tracker.is_empty() {
        tokio::select! { _ = h.base.outcomes.recv() => {}, _ = tokio::time::sleep(Duration::from_millis(5)) => {} }
    }
}
