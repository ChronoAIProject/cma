//! Owner scoping: the switcher offers Personal and admin organizations, and
//! every switch is verified through REST's creator cache before it is
//! stored.
use super::support::{csrf_for, Harness};
use cmaeg::console::session::{Flash, OwnerSelection};
use std::{sync::atomic::Ordering, time::Duration};

fn selected(harness: &Harness, sid: &str) -> OwnerSelection {
    harness
        .console
        .sessions()
        .find(sid)
        .unwrap()
        .meta()
        .selected
        .clone()
}

#[tokio::test]
async fn the_switcher_offers_admin_organizations_and_disables_the_rest() {
    let harness = Harness::enabled().await;
    let oauth = &harness.nyxid.oauth;
    oauth.grant("org-a", "Org A", "alice", "admin");
    oauth.grant("org-b", "Org B", "alice", "member");
    let (sid, _) = harness.sign_in("alice", "/console").await;
    let page = harness.get_as("/console", &sid).await.text().await.unwrap();
    assert!(page.contains("Signed in as …lice"));
    assert!(page.contains("<option value=\"personal\" selected>Personal</option>"));
    assert!(page.contains("<option value=\"org-a\">Org A</option>"));
    assert!(page.contains("<option value=\"org-b\" disabled>Org B (Admin role required)</option>"));
    assert!(page.contains("No channels to show yet."));
    // The list is fetched at most once a minute per session.
    harness.get_as("/console", &sid).await;
    assert_eq!(oauth.org_list_calls.load(Ordering::SeqCst), 1);
    harness.clock.advance(Duration::from_secs(61));
    harness.get_as("/console", &sid).await;
    assert_eq!(oauth.org_list_calls.load(Ordering::SeqCst), 2);
    harness.close().await;
}

#[tokio::test]
async fn an_owner_switch_is_verified_before_it_is_stored() {
    let harness = Harness::enabled().await;
    let oauth = &harness.nyxid.oauth;
    oauth.grant("org-a", "Org A", "alice", "admin");
    oauth.grant("org-b", "Org B", "alice", "member");
    let (sid, _) = harness.sign_in("alice", "/console").await;
    let before = oauth.authorization_calls.load(Ordering::SeqCst);
    let response = harness.switch_owner(&sid, "org-a").await;
    assert_eq!(response.status(), 303);
    assert_eq!(response.headers()["location"], "/console");
    assert_eq!(oauth.authorization_calls.load(Ordering::SeqCst), before + 1);
    assert_eq!(
        selected(&harness, &sid),
        OwnerSelection::Organization("org-a".into())
    );
    let page = harness.get_as("/console", &sid).await.text().await.unwrap();
    assert!(page.contains("Channels of organization Org A."));
    // The organization's entry in REST's cache answered that page.
    assert_eq!(oauth.authorization_calls.load(Ordering::SeqCst), before + 1);
    // A member-only organization is refused; the selection stays.
    let refused = harness.switch_owner(&sid, "org-b").await;
    assert_eq!(refused.headers()["location"], "/console");
    assert_eq!(
        selected(&harness, &sid),
        OwnerSelection::Organization("org-a".into())
    );
    let page = harness.get_as("/console", &sid).await.text().await.unwrap();
    assert!(page.contains("That owner could not be selected."));
    // A malformed choice is refused the same way.
    harness.switch_owner(&sid, "bad owner").await;
    assert_eq!(
        harness.console.sessions().find(&sid).unwrap().meta().flash,
        Some(Flash::OwnerRefused)
    );
    assert_eq!(harness.switch_owner(&sid, "personal").await.status(), 303);
    assert_eq!(selected(&harness, &sid), OwnerSelection::Personal);
    harness.close().await;
}

#[tokio::test]
async fn a_demoted_admin_drops_to_personal_and_cannot_reselect_once_the_cache_expires() {
    let harness = Harness::enabled().await;
    let oauth = &harness.nyxid.oauth;
    oauth.grant("org-a", "Org A", "alice", "admin");
    let (sid, _) = harness.sign_in("alice", "/console").await;
    harness.switch_owner(&sid, "org-a").await;
    let page = harness.get_as("/console", &sid).await.text().await.unwrap();
    assert!(page.contains("Channels of organization Org A."));
    let csrf = csrf_for(&page, "/console/owner");
    oauth.grant("org-a", "Org A", "alice", "member");
    // Past REST's 60 s cache, re-selecting the organization makes no change:
    // the request's own authority finds the demotion first.
    harness.advance(Duration::from_secs(61)).await;
    let response = harness
        .post(
            "/console/owner",
            Some(&sid),
            None,
            &[("csrf", &csrf), ("owner", "org-a")],
        )
        .await;
    assert_eq!(response.headers()["location"], "/console");
    assert_eq!(selected(&harness, &sid), OwnerSelection::Personal);
    let page = harness.get_as("/console", &sid).await.text().await.unwrap();
    assert!(
        page.contains("Your admin access to Org A has ended"),
        "{page}"
    );
    assert!(page.contains("Channels of your personal account."));
    // From Personal, the switch itself now refuses the organization.
    let refused = harness.switch_owner(&sid, "org-a").await;
    assert_eq!(refused.headers()["location"], "/console");
    assert_eq!(selected(&harness, &sid), OwnerSelection::Personal);
    let page = harness.get_as("/console", &sid).await.text().await.unwrap();
    assert!(page.contains("That owner could not be selected."));
    harness.close().await;
}

#[tokio::test]
async fn a_post_during_a_token_endpoint_outage_gets_the_503_page() {
    let harness = Harness::enabled().await;
    let (sid, _) = harness.sign_in("alice", "/console").await;
    let page = harness.get_as("/console", &sid).await.text().await.unwrap();
    let csrf = csrf_for(&page, "/console/owner");
    harness.clock.advance(Duration::from_secs(880));
    harness
        .nyxid
        .oauth
        .token_status
        .store(503, Ordering::SeqCst);
    let response = harness
        .post(
            "/console/owner",
            Some(&sid),
            None,
            &[("csrf", &csrf), ("owner", "personal")],
        )
        .await;
    assert_eq!(response.status(), 503);
    assert!(response.headers().get("location").is_none());
    assert!(response
        .text()
        .await
        .unwrap()
        .contains("Console unavailable"));
    assert_eq!(harness.console.sessions().len(), 1);
    harness.close().await;
}
