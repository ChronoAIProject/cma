//! An event's routing on its page and in the Activity tab: the decision,
//! the trace with rules tried and consumers skipped, routing refusals and
//! the kill switch's shadow decision, from what the routing ingress
//! reports to the console observer.
use cmaeg::{
    console::{
        mask,
        observer::{ConsoleObserver, Observation},
    },
    turns::observer::ActivityObserver,
};
use cmaeg_core::{
    activity::Activity,
    channel::ChannelDefinition,
    ids::RecordId,
    routing::{explain, resolve, Resolution, Trace},
};
use serde_json::json;

use super::{
    pages_support::{record, World},
    policy::NYX,
    resolver_fixture::{event, input, lark, message},
    routed_support::{routed_with, secrets},
};

/// The routed console fixture at record `index`, with `edit` applied,
/// stored in `world`.
fn store(
    world: &World,
    index: usize,
    edit: impl FnOnce(&mut serde_json::Value),
) -> (RecordId, ChannelDefinition) {
    let record = record(index);
    let definition = routed_with(&record, "alice", "alice-routed", edit);
    world.store(definition.clone(), 1);
    (record, definition)
}

/// Apply what `report` told a console observer to `world`'s memory.
async fn report(world: &World, report: impl FnOnce(&ConsoleObserver)) {
    let (observer, queue) = ConsoleObserver::new();
    report(&observer);
    drop(observer);
    let mut observations: Vec<Observation> = Vec::new();
    queue
        .drain(|observation| observations.push(observation))
        .await;
    for observation in observations {
        world.memory.apply(observation);
    }
}

/// `activity` routed on `def` with routing on, and the trace.
fn routed(def: &ChannelDefinition, mut activity: Activity) -> (Activity, Trace) {
    let route = input(def, &activity);
    let Ok(Resolution::Routed(decision)) = resolve(def, &route, true) else {
        panic!("expected a routed decision");
    };
    activity.routing = Some(decision.routing);
    (activity, decision.trace)
}

async fn page(world: &World, record: &RecordId, sid: &str) -> String {
    let path = format!(
        "/console/channels/{record}/events/{}",
        mask::event_key("evt-1")
    );
    let response = world.harness.get_as(&path, sid).await;
    assert_eq!(response.status(), 200, "{path}");
    response.text().await.unwrap()
}

fn assert_clean(page: &str) {
    for raw in [
        "please review",
        "chat-1",
        "user-1",
        "https://ci.example/build",
    ] {
        assert!(!page.contains(raw), "the page shows {raw}");
    }
    for secret in secrets() {
        assert!(!page.contains(&secret), "the page shows {secret}");
    }
}

#[tokio::test]
async fn an_event_page_shows_the_decision_the_rules_tried_and_skipped_consumers() {
    let world = World::start().await;
    // ci gives every com.example.* event to reviewer and the pending nyx.
    let (target, def) = store(&world, 1, |value| {
        value["routing"]["rules"][2] = json!({"id": "ci",
            "when": {"event_types": ["com.example.*"]},
            "targets": [super::policy::REVIEWER, NYX]});
    });
    let (activity, trace) = routed(&def, event("com.example.build", json!({})));
    report(&world, |observer| {
        observer.routed(&target, &activity, &trace);
        observer.admitted(&target, &activity);
    })
    .await;
    let (sid, _) = world.harness.sign_in("alice", "/console").await;
    let page = page(&world, &target, &sid).await;
    for fact in [
        "<dt>Resolution</dt><dd>rule</dd>",
        "<dt>Rule</dt><dd><code>ci</code></dd>",
        "<dt>Targets</dt><dd>@reviewer <code>csm_00000000000a</code> (primary)</dd>",
        "<dt>Source slot</dt><dd><code>custom</code></dd>",
        "<li>decided by the rules</li>",
        "<li>rule <code>ops-chat</code>: did not match (failed on",
        "<li>rule <code>slack-only</code>: inactive, skipped</li>",
        "<li>rule <code>ci</code>: matched</li>",
        "<h3>Skipped consumers</h3><ul><li>@nyx <code>csm_00000000000b</code>: pending</li></ul>",
    ] {
        assert!(page.contains(fact), "{fact}");
    }
    assert_clean(&page);
    let activity = world
        .harness
        .get_as(&format!("/console/channels/{target}?tab=activity"), &sid)
        .await
        .text()
        .await
        .unwrap();
    assert!(activity.contains("<td>rule <code>ci</code></td>"));
    assert_eq!(activity.matches("<tr><td>").count(), 1, "one row per event");
    world.close().await;
}

#[tokio::test]
async fn a_routing_refusal_shows_its_reason_and_the_shadow_decision() {
    let world = World::start().await;
    let (target, def) = store(&world, 1, |_| {});
    // Under the kill switch a mention of reviewer would leak the event to a
    // consumer other than default, so it is refused.
    let activity = message(lark(), "@reviewer please review");
    let route = input(&def, &activity);
    let error = resolve(&def, &route, false).unwrap_err();
    let trace = explain(&def, &route, false);
    report(&world, |observer| {
        observer.route_refused(&target, &activity, &error.to_string(), &trace);
    })
    .await;
    let (sid, _) = world.harness.sign_in("alice", "/console").await;
    let page = page(&world, &target, &sid).await;
    for fact in [
        "<p>Refused by routing: <code>kill_switch_closed</code>.</p>",
        "<li>decided by the kill switch: multi-consumer routing is off</li>",
        "<li>addressing by mention naming 1 consumer(s)</li>",
        "<li>shadow decision under the kill switch: mention → refused</li>",
    ] {
        assert!(page.contains(fact), "{fact}");
    }
    assert!(!page.contains("<dt>Resolution</dt>"));
    assert_clean(&page);
    let refused = world
        .harness
        .get_as(
            &format!("/console/channels/{target}?tab=activity&outcome=refused"),
            &sid,
        )
        .await
        .text()
        .await
        .unwrap();
    assert!(refused.contains("refused: kill_switch_closed"));
    world.close().await;
}

#[tokio::test]
async fn a_legacy_launch_under_the_kill_switch_shows_its_shadow_decision() {
    let world = World::start().await;
    let (target, def) = store(&world, 1, |_| {});
    let activity = message(lark(), "please review");
    let route = input(&def, &activity);
    assert!(matches!(
        resolve(&def, &route, false),
        Ok(Resolution::Legacy)
    ));
    let trace = explain(&def, &route, false);
    report(&world, |observer| {
        observer.routed(&target, &activity, &trace);
        observer.admitted(&target, &activity);
    })
    .await;
    let (sid, _) = world.harness.sign_in("alice", "/console").await;
    let page = page(&world, &target, &sid).await;
    assert!(page.contains("Routing kept this event on the legacy path"));
    assert!(page.contains(
        "<li>shadow decision under the kill switch: default targets → legacy path to the default consumer</li>"
    ));
    assert_clean(&page);
    world.close().await;
}

#[tokio::test]
async fn an_unreported_event_on_a_routed_channel_says_no_decision_was_recorded() {
    let world = World::start().await;
    let (target, _) = store(&world, 1, |_| {});
    let activity = message(lark(), "please review");
    report(&world, |observer| observer.admitted(&target, &activity)).await;
    let (sid, _) = world.harness.sign_in("alice", "/console").await;
    let page = page(&world, &target, &sid).await;
    assert!(page.contains("No routing decision was recorded for this event"));
    assert!(!page.contains("<h3>Trace</h3>"));
    world.close().await;
}
