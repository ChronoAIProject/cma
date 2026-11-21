//! Amendment R6 §R6.4 on the channel pages: every page and post derives
//! authority through REST's creator cache, so a demotion or a deactivation
//! stops reads and source changes within 60 s, with no write.
use std::{sync::atomic::Ordering, time::Duration};

use cmaeg::console::source_changes::SourceAvailability;

use super::pages_support::{channel_id, World, EVENT};

const ATTACH: &str = "/console/channels/{record_id}/sources";
const DETACH: &str = "/console/channels/{record_id}/sources/{source_id}/detach";
const LINK: &str = "/console/channels/{record_id}/link";

#[tokio::test]
async fn a_demotion_stops_every_organization_page_and_post_within_60_seconds() {
    let world = World::with_sources(SourceAvailability::Available).await;
    let personal = world.add(1, "alice", "alice-orders");
    let organization = world.add(4, "org-a", "org-operations");
    world.traffic(&organization, "https://org.example/feed");
    let oauth = &world.harness.nyxid.oauth;
    oauth.grant("org-a", "Org A", "alice", "admin");
    let (sid, _) = world.harness.sign_in("alice", "/console").await;
    assert_eq!(
        world.harness.switch_owner(&sid, "org-a").await.status(),
        303
    );
    let org_pages = [
        format!("/console/channels/{organization}?tab=overview"),
        format!("/console/channels/{organization}?tab=activity"),
        format!("/console/channels/{organization}?tab=producers"),
        format!("/console/channels/{organization}/events/{EVENT}"),
        format!("/console/channels/{organization}/sources/custom/detach"),
    ];
    for page in &org_pages {
        assert_eq!(
            world.harness.get_as(page, &sid).await.status(),
            200,
            "{page}"
        );
    }
    let attach = world.csrf(&sid, ATTACH, &organization);
    let detach = world.csrf(&sid, DETACH, &organization);
    let link = world.csrf(&sid, LINK, &organization);
    let pasted = channel_id(&organization).to_string();
    let posts = [
        (
            format!("/console/channels/{organization}/sources"),
            vec![
                ("csrf", attach.as_str()),
                ("expected_version", "1"),
                ("type", "telegram"),
                ("bot_id", "7000000009"),
            ],
        ),
        (
            format!("/console/channels/{organization}/sources/custom/detach"),
            vec![("csrf", detach.as_str()), ("expected_version", "1")],
        ),
        (
            format!("/console/channels/{organization}/link"),
            vec![("csrf", link.as_str()), ("channel_id", pasted.as_str())],
        ),
    ];

    oauth.grant("org-a", "Org A", "alice", "member");
    // REST's cache answers for up to 60 s, exactly as it does for REST.
    world.harness.advance(Duration::from_secs(59)).await;
    assert_eq!(
        world.harness.get_as(&org_pages[0], &sid).await.status(),
        200
    );
    world.harness.advance(Duration::from_secs(2)).await;

    // The first post after the demotion goes home, where the flash explains.
    for (index, (path, fields)) in posts.iter().enumerate() {
        let response = world.harness.post(path, Some(&sid), None, fields).await;
        if index == 0 {
            assert_eq!(response.status(), 303, "{path}");
            assert_eq!(response.headers()["location"], "/console");
        } else {
            // Now authorized as Personal: the record is not theirs.
            assert_eq!(response.status(), 404, "{path}");
        }
    }
    for page in &org_pages {
        let response = world.harness.get_as(page, &sid).await;
        assert_eq!(response.status(), 404, "{page}");
        assert!(!response.text().await.unwrap().contains("org-operations"));
    }
    let list = world
        .harness
        .get_as("/console", &sid)
        .await
        .text()
        .await
        .unwrap();
    assert!(list.contains("Channels of your personal account."));
    assert!(list.contains("alice-orders") && !list.contains("org-operations"));
    // Personal pages keep working throughout.
    let own = world
        .harness
        .get_as(&format!("/console/channels/{personal}?tab=sources"), &sid)
        .await;
    assert_eq!(own.status(), 200);
    assert_eq!(
        world.changes().calls.load(Ordering::SeqCst),
        0,
        "no CMA write"
    );
    assert_eq!(world.reader.channels.lock().unwrap()[&organization].1, 1);
    world.close().await;
}

#[tokio::test]
async fn a_deactivated_owners_attach_ends_the_session_without_a_write() {
    let world = World::with_sources(SourceAvailability::Available).await;
    let personal = world.add(1, "alice", "alice-orders");
    let (sid, _) = world.harness.sign_in("alice", "/console").await;
    let attach = world.csrf(&sid, ATTACH, &personal);
    world
        .harness
        .nyxid
        .oauth
        .deactivated
        .lock()
        .unwrap()
        .insert("alice".into());
    world.harness.advance(Duration::from_secs(61)).await;
    let response = world
        .harness
        .post(
            &format!("/console/channels/{personal}/sources"),
            Some(&sid),
            None,
            &[
                ("csrf", attach.as_str()),
                ("expected_version", "1"),
                ("type", "telegram"),
                ("bot_id", "7000000009"),
            ],
        )
        .await;
    assert_eq!(response.status(), 303);
    assert_eq!(
        response.headers()["location"],
        "/console/signed-out?reason=expired"
    );
    assert!(response.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .starts_with("__Host-cmaeg-console=;"));
    assert!(world.harness.console.sessions().find(&sid).is_none());
    assert_eq!(
        world.changes().calls.load(Ordering::SeqCst),
        0,
        "no CMA write"
    );
    world.close().await;
}
