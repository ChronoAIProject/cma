//! The console's channel pages, rendered from gateway memory for a signed-in
//! session: ownership, paging, masking, escaping and headers.
use super::pages_support::World;

async fn text(world: &World, path: &str, sid: &str) -> String {
    let response = world.harness.get_as(path, sid).await;
    assert_eq!(response.status(), 200, "{path}");
    response.text().await.unwrap()
}

#[tokio::test]
async fn each_owner_sees_only_their_own_channels() {
    let world = World::start().await;
    world.add(1, "alice", "alice-orders");
    world.add(2, "alice", "alice-support");
    world.add(3, "bob", "bob-billing");
    world.add(4, "org-a", "org-operations");
    world
        .harness
        .nyxid
        .oauth
        .grant("org-a", "Org A", "alice", "admin");
    let (sid, _) = world.harness.sign_in("alice", "/console").await;

    let personal = text(&world, "/console", &sid).await;
    assert!(personal.contains("Channels of your personal account."));
    assert!(personal.contains("alice-orders") && personal.contains("alice-support"));
    for other in ["bob-billing", "org-operations"] {
        assert!(!personal.contains(other), "{other} on the personal list");
    }
    assert!(personal.contains("telegram") && personal.contains("relay_lark"));
    assert!(personal.contains("default · cma_codex"));

    assert_eq!(
        world.harness.switch_owner(&sid, "org-a").await.status(),
        303
    );
    let organization = text(&world, "/console", &sid).await;
    assert!(organization.contains("Channels of organization Org A."));
    assert!(organization.contains("org-operations"));
    for other in ["alice-orders", "alice-support", "bob-billing"] {
        assert!(!organization.contains(other), "{other} on the org list");
    }
    world.close().await;
}

#[tokio::test]
async fn the_list_pages_fifty_rows_and_filters_by_name() {
    let world = World::start().await;
    for index in 0..120 {
        world.add(index, "alice", &format!("chan-{index:03}"));
    }
    world.add(500, "bob", "chan-bob");
    let (sid, _) = world.harness.sign_in("alice", "/console").await;

    let first = text(&world, "/console", &sid).await;
    // A page fills at most 50 summaries.
    assert_eq!(world.reads(), 50);
    assert!(first.contains("chan-000") && first.contains("chan-049"));
    assert!(!first.contains("chan-050"));
    assert!(first.contains("Page 1 of 3 · 120 channels"));
    let second = text(&world, "/console?page=1", &sid).await;
    assert!(second.contains("chan-050") && second.contains("chan-099"));
    assert!(!second.contains("chan-049") && !second.contains("chan-100"));
    let third = text(&world, "/console?page=2", &sid).await;
    assert!(third.contains("chan-100") && third.contains("chan-119"));
    assert!(third.contains("Page 3 of 3"));
    assert_eq!(world.reads(), 120);

    // Case-insensitive substring of the name; the other owner never shows.
    let filtered = text(&world, "/console?q=CHAN-01", &sid).await;
    for index in 10..20 {
        assert!(filtered.contains(&format!("chan-{index:03}")));
    }
    for absent in ["chan-009", "chan-020", "chan-bob"] {
        assert!(!filtered.contains(absent), "{absent}");
    }
    assert!(filtered.contains("value=\"CHAN-01\""));
    let none = text(&world, "/console?q=nothing-matches", &sid).await;
    assert!(none.contains("No channel name matches this filter."));
    // A filter longer than 80 characters is cut, not refused.
    let long = "x".repeat(200);
    let cut = text(&world, &format!("/console?q={long}"), &sid).await;
    assert!(cut.contains(&format!("value=\"{}\"", "x".repeat(80))));
    assert!(!cut.contains(&"x".repeat(81)));
    world.close().await;
}
