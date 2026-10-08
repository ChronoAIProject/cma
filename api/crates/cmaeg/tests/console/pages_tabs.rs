//! A channel's detail tabs, rendered only from gateway memory and only for
//! the record's owner.
use cmaeg::console::observer::Observation;

use super::pages_support::{definition, record, World, AGENT_KEY_HASH, SECRETS, TABS};

async fn tab(world: &World, record: &str, tab: &str, sid: &str) -> String {
    let path = format!("/console/channels/{record}?tab={tab}");
    let response = world.harness.get_as(&path, sid).await;
    assert_eq!(response.status(), 200, "{path}");
    response.text().await.unwrap()
}

#[tokio::test]
async fn every_tab_renders_the_owners_channel_from_memory() {
    let world = World::start().await;
    let orders = world.add(1, "alice", "alice-orders");
    world.traffic(&orders, "https://sensors.example/line-1");
    let (sid, _) = world.harness.sign_in("alice", "/console").await;
    let record = orders.to_string();

    let overview = tab(&world, &record, "overview", &sid).await;
    assert!(overview.contains("<h1>alice-orders</h1>"));
    assert!(overview.contains(&format!("cmaeg1.ch.fixture-{record}")));
    let base = &world.harness.base;
    for endpoint in [
        format!("{base}/v1/callbacks/cmaeg1.ch.fixture-{record}"),
        format!("{base}/v1/channels/cmaeg1.ch.fixture-{record}/events"),
        format!("{base}/v1/telegram/cmaeg1.ch.fixture-{record}"),
    ] {
        assert!(overview.contains(&endpoint), "{endpoint}");
    }
    for fact in [
        "<dt>Partition key</dt><dd>per_event</dd>",
        "<dt>Telegram token</dt><dd>set</dd>",
        "<dt>Telegram webhook secret</dt><dd>set</dd>",
        "<dt>Channel key</dt><dd>set</dd>",
        "<dt>HMAC secrets</dt><dd>set for hmac-key-1</dd>",
        "<dt>Owner kind</dt><dd>human</dd>",
        "<dt>Version</dt><dd>1</dd>",
        "12.0 KiB of about 48 KiB",
        "Source changes are not available in this gateway release yet.",
    ] {
        assert!(overview.contains(fact), "{fact}");
    }
    assert!(overview.contains("sha256:…12345678"));

    let sources = tab(&world, &record, "sources", &sid).await;
    for fact in [
        "<code>telegram</code>",
        "<code>relay_lark</code>",
        "<code>custom</code>",
        "@lark_fixture_bot · id ou_fixturebot",
        "@fixture_bot · id 7000000001",
        "senders allowlist of 1; chats open; groups on mention",
        "NyxID relay token from https://nyxid.example",
        "<code>relay-key-1</code>",
        "<code>hmac-key-1</code>",
        "attachments at /attachments from 1 origin(s)",
        "<td>2</td>",
    ] {
        assert!(sources.contains(fact), "{fact}");
    }

    let consumers = tab(&world, &record, "consumers", &sid).await;
    for fact in [
        "cma_codex",
        "binding-1",
        "<td>L1</td>",
        "profile",
        "@default",
    ] {
        assert!(consumers.contains(fact), "{fact}");
    }
    let routing = tab(&world, &record, "routing", &sid).await;
    assert!(routing.contains("<dt>Model</dt><dd>legacy</dd>"));
    assert!(routing.contains("every admitted event goes to its default consumer"));

    let activity = tab(&world, &record, "activity", &sid).await;
    for fact in [
        "0123456789abcdef",
        "…5678",
        "…7890",
        "1 image, 2 file",
        "accepted · after 120 ms · answer delivered · to origin",
        "failure notice failed (chat_not_found) · to default",
        "refused: schema_mismatch",
    ] {
        assert!(activity.contains(fact), "{fact}");
    }
    let only_refusals = tab(&world, &record, "activity&outcome=refused", &sid).await;
    assert!(only_refusals.contains("schema_mismatch"));
    assert!(!only_refusals.contains("0123456789abcdef"));
    let only_custom = tab(&world, &record, "activity&source=custom", &sid).await;
    assert!(!only_custom.contains("answer delivered"));
    // Admitted events carry the slot id the Sources tab and REST use.
    assert!(activity.contains("<option value=\"relay_lark\">relay_lark</option>"));
    let only_lark = tab(&world, &record, "activity&source=relay_lark", &sid).await;
    assert!(only_lark.contains("answer delivered"));
    assert!(!only_lark.contains("schema_mismatch"));

    let producers = tab(&world, &record, "producers", &sid).await;
    for fact in [
        "https://sensors.example/line-1",
        "hmac:hmac-key-1",
        "com.example.order",
        "identity is advisory",
    ] {
        assert!(producers.contains(fact), "{fact}");
    }
    world.close().await;
}

#[tokio::test]
async fn another_owners_or_an_unknown_record_is_404_on_every_tab() {
    let world = World::start().await;
    let billing = world.add(3, "bob", "bob-billing");
    world.traffic(&billing, "https://bob.example/feed");
    let (sid, _) = world.harness.sign_in("alice", "/console").await;
    for target in [
        billing.to_string(),
        record(99).to_string(),
        "not-a-record".to_owned(),
    ] {
        for name in TABS {
            let path = format!("/console/channels/{target}?tab={name}");
            let response = world.harness.get_as(&path, &sid).await;
            assert_eq!(response.status(), 404, "{path}");
            let body = response.text().await.unwrap();
            assert!(body.contains("There is nothing at this address."));
            for leaked in ["bob-billing", "bob.example", "…5678"] {
                assert!(!body.contains(leaked), "{path} leaked {leaked}");
            }
        }
    }
    world.close().await;
}

#[tokio::test]
async fn a_record_missing_from_the_index_is_read_once_by_record() {
    let world = World::start().await;
    let late = record(7);
    // In CMA but not yet indexed: the page reads it once, with the owner
    // check, and indexes it.
    world.reader.channels.lock().unwrap().insert(
        late.clone(),
        (definition(&late, "alice", "late-channel"), 4),
    );
    let foreign = record(8);
    world.reader.channels.lock().unwrap().insert(
        foreign.clone(),
        (definition(&foreign, "bob", "bob-late"), 1),
    );
    let (sid, _) = world.harness.sign_in("alice", "/console").await;
    let page = tab(&world, late.as_str(), "overview", &sid).await;
    assert!(page.contains("late-channel"));
    assert!(page.contains("<dt>Version</dt><dd>4</dd>"));
    assert_eq!(world.reads(), 1);
    tab(&world, late.as_str(), "sources", &sid).await;
    assert_eq!(world.reads(), 1, "the indexed summary is reused");
    // Another owner's unindexed record is read once, then remembered as a
    // miss.
    for _ in 0..3 {
        let path = format!("/console/channels/{foreign}");
        assert_eq!(world.harness.get_as(&path, &sid).await.status(), 404);
    }
    assert_eq!(world.reads(), 2);
    world.close().await;
}

#[tokio::test]
async fn refresh_renders_read_no_cma_while_the_version_is_unchanged() {
    let world = World::start().await;
    let orders = world.add(1, "alice", "alice-orders");
    let (sid, _) = world.harness.sign_in("alice", "/console").await;
    let record = orders.to_string();
    let refreshing = tab(&world, &record, "activity&refresh=10", &sid).await;
    assert!(refreshing.contains("<meta http-equiv=\"refresh\" content=\"10\">"));
    let reads = world.reads();
    for _ in 0..5 {
        tab(&world, &record, "activity&refresh=10", &sid).await;
    }
    assert_eq!(world.reads(), reads, "no CMA read while the version holds");
    for ignored in ["7", "5", "abc", "600"] {
        let page = tab(
            &world,
            &record,
            &format!("activity&refresh={ignored}"),
            &sid,
        )
        .await;
        assert!(!page.contains("http-equiv"), "refresh={ignored}");
    }
    assert!(!tab(&world, &record, "overview&refresh=10", &sid)
        .await
        .contains("http-equiv"));
    // A new version is read once, then reused again.
    world.store(definition(&orders, "alice", "alice-orders-v2"), 2);
    assert!(tab(&world, &record, "activity&refresh=30", &sid)
        .await
        .contains("alice-orders-v2"));
    assert_eq!(world.reads(), reads + 1);
    tab(&world, &record, "activity&refresh=30", &sid).await;
    assert_eq!(world.reads(), reads + 1);
    world.close().await;
}

#[tokio::test]
async fn tabs_show_no_secret_full_hash_or_sender_id_and_carry_the_headers() {
    let world = World::start().await;
    let orders = world.add(1, "alice", "alice-orders");
    world.traffic(&orders, "https://sensors.example/line-1");
    world.memory.apply(Observation::ChannelId {
        record: orders.clone(),
        id: super::pages_support::channel_id(&orders),
    });
    let (sid, _) = world.harness.sign_in("alice", "/console").await;
    for name in TABS {
        let path = format!("/console/channels/{orders}?tab={name}");
        let response = world.harness.get_as(&path, &sid).await;
        assert_eq!(response.status(), 200);
        let headers = response.headers().clone();
        assert!(headers["content-security-policy"]
            .to_str()
            .unwrap()
            .starts_with("default-src 'none'; style-src 'sha256-"));
        assert_eq!(headers["cache-control"], "no-store");
        assert_eq!(headers["x-frame-options"], "DENY");
        let body = response.text().await.unwrap();
        assert!(body.contains("<meta name=\"robots\" content=\"noindex\">"));
        assert!(!body.to_lowercase().contains("<script"), "{name}");
        for secret in SECRETS
            .iter()
            .chain([&AGENT_KEY_HASH, &"ou_sender_fixture_1"])
        {
            assert!(!body.contains(secret), "{name} shows {secret}");
        }
    }
    world.close().await;
}
