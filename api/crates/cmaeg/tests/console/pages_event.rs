//! An event's page: its consumers' timeline and causation, served from the
//! activity ring only to the channel's owner.
use super::pages_support::{World, EVENT, FOLLOW_UP};

#[tokio::test]
async fn an_event_page_shows_the_timeline_and_links_its_parent() {
    let world = World::start().await;
    let orders = world.add(1, "alice", "alice-orders");
    world.traffic(&orders, "https://sensors.example/line-1");
    let (sid, _) = world.harness.sign_in("alice", "/console").await;

    let activity = world
        .harness
        .get_as(&format!("/console/channels/{orders}?tab=activity"), &sid)
        .await
        .text()
        .await
        .unwrap();
    let link = format!("/console/channels/{orders}/events/{EVENT}");
    assert!(activity.contains(&format!("href=\"{link}\"")));

    let first = world.harness.get_as(&link, &sid).await;
    assert_eq!(first.status(), 200);
    let first = first.text().await.unwrap();
    for fact in [
        "Event <code>0123456789abcdef</code>",
        "<dd>relay_lark</dd>",
        "<dd>…5678</dd>",
        "<dd>…7890</dd>",
        "no routing policy, so the event went to the default consumer",
        "<li>launch accepted</li>",
        "<li>accepted after 120 ms</li>",
        "<li>answer delivered</li>",
        "<li>destination origin</li>",
    ] {
        assert!(first.contains(fact), "{fact}");
    }
    assert!(!first.contains("Causation"));

    let follow_up = world
        .harness
        .get_as(
            &format!("/console/channels/{orders}/events/{FOLLOW_UP}"),
            &sid,
        )
        .await
        .text()
        .await
        .unwrap();
    assert!(follow_up.contains("<li>failure notice failed (chat_not_found)</li>"));
    assert!(follow_up.contains("<li>destination default</li>"));
    assert!(follow_up.contains("<h2>Causation</h2>"));
    assert!(follow_up.contains(&format!("<a href=\"{link}\"><code>{EVENT}</code></a>")));
    assert!(follow_up.contains("<dt>Hop</dt><dd>1</dd>"));
    world.close().await;
}

#[tokio::test]
async fn events_of_other_owners_unknown_keys_and_malformed_keys_are_404() {
    let world = World::start().await;
    let orders = world.add(1, "alice", "alice-orders");
    let billing = world.add(3, "bob", "bob-billing");
    world.traffic(&billing, "https://bob.example/feed");
    let (sid, _) = world.harness.sign_in("alice", "/console").await;
    for path in [
        format!("/console/channels/{billing}/events/{EVENT}"),
        format!("/console/channels/{orders}/events/{EVENT}"),
        format!("/console/channels/{orders}/events/0123456789ABCDEF"),
        format!("/console/channels/{orders}/events/%3Cscript%3E"),
        format!("/console/channels/not-a-record/events/{EVENT}"),
    ] {
        let response = world.harness.get_as(&path, &sid).await;
        assert_eq!(response.status(), 404, "{path}");
        let body = response.text().await.unwrap();
        assert!(!body.contains("bob") && !body.contains("…5678"), "{path}");
    }
    world.close().await;
}
