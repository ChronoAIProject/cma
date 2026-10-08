//! The console pages of a routed channel: consumers, the routing policy,
//! and how each event was routed, all from gateway memory.
use super::{
    pages_support::World,
    routed_support::{self, secrets},
};

async fn tab(world: &World, record: &str, tab: &str, sid: &str) -> String {
    let path = format!("/console/channels/{record}?tab={tab}");
    let response = world.harness.get_as(&path, sid).await;
    assert_eq!(response.status(), 200, "{path}");
    response.text().await.unwrap()
}

fn assert_no_secret(page: &str, name: &str) {
    for secret in secrets() {
        assert!(!page.contains(&secret), "{name} shows {secret}");
    }
}

#[tokio::test]
async fn the_consumers_tab_shows_what_routing_reads_of_each_consumer() {
    let world = World::start().await;
    let routed = routed_support::add(&world, 1, "alice", "alice-routed");
    let (sid, _) = world.harness.sign_in("alice", "/console").await;
    let page = tab(&world, routed.as_str(), "consumers", &sid).await;
    let rows: Vec<&str> = page.split("<tr>").skip(2).collect();
    assert_eq!(rows.len(), 3, "default, reviewer and nyx");
    let (lead, reviewer, nyx) = (rows[0], rows[1], rows[2]);
    for fact in [
        "Lead",
        "@lead",
        "<code>default</code>",
        "mentionable",
        "answers (provider default)",
        "automatic final answer on · to origin",
        "to any routable consumer",
    ] {
        assert!(lead.contains(fact), "default: {fact}");
    }
    for fact in [
        "@reviewer",
        "<code>csm_00000000000a</code>",
        "binding-reviewer",
        "answers (provider default)",
        "to <code>default</code>",
    ] {
        assert!(reviewer.contains(fact), "reviewer: {fact}");
    }
    assert!(!reviewer.contains("Pending"));
    for fact in [
        "@nyx",
        "<span class=\"badge\">Pending</span>",
        "not mentionable",
        "answers (set on the consumer)",
        "<td>off</td>",
    ] {
        assert!(nyx.contains(fact), "nyx: {fact}");
    }
    assert_no_secret(&page, "consumers");
    world.close().await;
}

#[tokio::test]
async fn a_consumers_capability_follows_the_configured_provider_set() {
    let world = World::start().await;
    let routed = routed_support::add(&world, 1, "alice", "alice-routed");
    let (sid, _) = world.harness.sign_in("alice", "/console").await;
    // Only cma_codex answers non-human actors by default; nyx's own
    // setting is the only way an nyxbot consumer answers them.
    let unset = routed_support::routed_with(&routed, "alice", "alice-routed", |value| {
        value["routing"]["consumers"][1]
            .as_object_mut()
            .unwrap()
            .remove("non_human");
    });
    world.store(unset, 2);
    let page = tab(&world, routed.as_str(), "consumers", &sid).await;
    let nyx = page.split("<tr>").nth(4).unwrap();
    assert!(nyx.contains("ignores (provider default)"), "{nyx}");
    world.close().await;
}

#[tokio::test]
async fn the_routing_tab_shows_the_policy_in_order_with_inactive_rules() {
    let world = World::start().await;
    let routed = routed_support::add(&world, 1, "alice", "alice-routed");
    let (sid, _) = world.harness.sign_in("alice", "/console").await;
    let page = tab(&world, routed.as_str(), "routing", &sid).await;
    for fact in [
        "<dt>Model</dt><dd>routed</dd>",
        "<dt>Default targets</dt><dd>@lead <code>default</code></dd>",
        "<dt>Default consumer receives every event</dt><dd>no</dd>",
        "<dt>Most targets per event</dt><dd>3</dd>",
        "<dt>Reply labels</dt><dd>always</dd>",
        "<dt>Default reply destination</dt><dd>route …8e9f of relay_lark</dd>",
        "at most 3 hops; 5 per consumer and 20 per channel a minute",
        "Mentions override rules, unless the consumer is not mentionable.",
        "<dt>Leading @name mentions</dt><dd>on</dd>",
        "<dt>cmaegtargets event field</dt><dd>on</dd>",
        "<td>1</td><td><code>ops-chat</code></td><td><code>relay_lark</code></td><td>kinds message; 1 chat(s)</td>",
        "@reviewer <code>csm_00000000000a</code>, @lead <code>default</code>",
        "<td>2</td><td><code>slack-only</code> <span class=\"badge\">inactive</span></td>",
        "<td>3</td><td><code>ci</code></td><td>any</td><td>event types com.example.ci.*; event sources https://ci.example/build</td>",
        // Multi-consumer routing is off in this release's composition.
        "events this channel's routing does not give to the default consumer are refused",
    ] {
        assert!(page.contains(fact), "{fact}");
    }
    assert_eq!(page.matches("badge\">inactive").count(), 1);
    assert_no_secret(&page, "routing");
    let overview = tab(&world, routed.as_str(), "overview", &sid).await;
    assert!(overview.contains("Multi-consumer routing is disabled by the operator"));
    world.close().await;
}

#[tokio::test]
async fn the_channel_list_counts_rules_and_marks_pending_consumers() {
    let world = World::start().await;
    routed_support::add(&world, 1, "alice", "alice-routed");
    world.add(2, "alice", "alice-legacy");
    let (sid, _) = world.harness.sign_in("alice", "/console").await;
    let list = world
        .harness
        .get_as("/console", &sid)
        .await
        .text()
        .await
        .unwrap();
    let routed = list
        .split("<tr>")
        .find(|row| row.contains("alice-routed"))
        .unwrap();
    assert!(routed.contains("<td>3 rules</td>"), "{routed}");
    assert!(routed.contains("Lead · cma_codex"));
    assert!(routed.contains("nyx · nyxbot <span class=\"badge\">Pending</span>"));
    let legacy = list
        .split("<tr>")
        .find(|row| row.contains("alice-legacy"))
        .unwrap();
    assert!(legacy.contains("<td>legacy</td>"));
    assert!(!legacy.contains("Pending"));
    assert!(list.contains("Multi-consumer routing is disabled by the operator"));
    assert_no_secret(&list, "list");
    world.close().await;
}
