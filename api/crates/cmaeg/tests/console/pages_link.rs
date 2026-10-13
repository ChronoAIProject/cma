//! Paste-link: an owner links a channel id the console has not seen, after
//! the gateway checks that it opens to this record. Nothing is re-sealed.
use super::{
    pages_support::{channel_id, definition, record, World},
    support::csrf_for,
};

const LINK_ERROR_OTHER: &str = "This channel id belongs to another channel.";

#[tokio::test]
async fn a_pasted_channel_id_is_linked_only_when_it_opens_to_the_record() {
    let world = World::start().await;
    let unseen = record(1);
    let mut stored = definition(&unseen, "alice", "unseen-channel");
    stored.creation = None;
    world.store(stored, 1);
    let sibling = world.add(2, "alice", "alice-sibling");
    let foreign = world.add(3, "bob", "bob-billing");
    let (sid, _) = world.harness.sign_in("alice", "/console").await;
    let overview_path = format!("/console/channels/{unseen}?tab=overview");
    let action = format!("/console/channels/{unseen}/link");
    let page = world
        .harness
        .get_as(&overview_path, &sid)
        .await
        .text()
        .await
        .unwrap();
    assert!(page.contains("Not observed since restart; paste to link."));
    assert!(!page.contains("Endpoints"));
    let csrf = csrf_for(&page, &action);

    for (pasted, message) in [
        (channel_id(&sibling).to_string(), LINK_ERROR_OTHER),
        (channel_id(&foreign).to_string(), LINK_ERROR_OTHER),
        (
            "cmaeg1.ch.forged-<script>alert(1)</script>".to_owned(),
            "This is not a channel id of this gateway.",
        ),
        (
            "plain words".to_owned(),
            "This is not a channel id of this gateway.",
        ),
    ] {
        let response = world
            .harness
            .post(
                &action,
                Some(&sid),
                None,
                &[("csrf", &csrf), ("channel_id", &pasted)],
            )
            .await;
        assert_eq!(response.status(), 400, "{pasted}");
        let body = response.text().await.unwrap();
        assert!(body.contains(message), "{pasted}");
        assert!(!body.contains(&pasted), "the pasted value is echoed");
    }

    let own = channel_id(&unseen).to_string();
    let response = world
        .harness
        .post(
            &action,
            Some(&sid),
            None,
            &[("csrf", &csrf), ("channel_id", &own)],
        )
        .await;
    assert_eq!(response.status(), 303);
    assert_eq!(response.headers()["location"], overview_path.as_str());
    let linked = world
        .harness
        .get_as(&overview_path, &sid)
        .await
        .text()
        .await
        .unwrap();
    assert!(linked.contains(&format!("<code>{own}</code>")));
    assert!(linked.contains("Endpoints"));
    // Every channel id the page shows is the one that was pasted.
    assert_eq!(
        linked.matches("cmaeg1.ch.").count(),
        linked.matches(&own).count()
    );
    world.close().await;
}

#[tokio::test]
async fn linking_another_owners_record_is_404_without_a_read_of_its_id() {
    let world = World::start().await;
    let foreign = world.add(3, "bob", "bob-billing");
    let own = world.add(1, "alice", "alice-orders");
    let (sid, _) = world.harness.sign_in("alice", "/console").await;
    let key = world
        .harness
        .console
        .sessions()
        .find(&sid)
        .unwrap()
        .meta()
        .csrf_key;
    let csrf =
        super::support::csrf_token(&key, "/console/channels/{record_id}/link", foreign.as_str());
    let response = world
        .harness
        .post(
            &format!("/console/channels/{foreign}/link"),
            Some(&sid),
            None,
            &[
                ("csrf", &csrf),
                ("channel_id", &channel_id(&foreign).to_string()),
            ],
        )
        .await;
    assert_eq!(response.status(), 404);
    // A token minted for one record is refused for another.
    let response = world
        .harness
        .post(
            &format!("/console/channels/{own}/link"),
            Some(&sid),
            None,
            &[
                ("csrf", &csrf),
                ("channel_id", &channel_id(&own).to_string()),
            ],
        )
        .await;
    assert_eq!(response.status(), 403);
    world.close().await;
}
