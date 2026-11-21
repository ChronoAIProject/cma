//! Attachment tools use the calling consumer's media audience: what the
//! reviewer's own binding may do decides, never the default's.
use super::{
    consumer_support::{add_reviewer, call, issue, reviewer, reviewer_key},
    download_support::{autonomous, download, relay},
    media_support::{relayed, MediaSetup},
    routing_fixture,
    support::App,
};
use cmaeg_core::{activity::AttachmentKind, ids::ConsumerId, seal::EventRef};
use reqwest::Method;
use serde_json::Value;

fn photo() -> Vec<u8> {
    (0..16 * 1024).map(|index| (index % 241) as u8).collect()
}

/// A routed app over one served photo, with the channel shaped by `edit`.
async fn app(edit: fn(&mut Value)) -> App {
    let setup = MediaSetup::new(vec![relayed(0, AttachmentKind::Image)]).edit(edit);
    setup.fakes.source.serve(&relay(0), Ok(photo()));
    App::with_media(setup).await
}

/// An autonomous default consumer and a guarded reviewer.
fn autonomous_default(value: &mut Value) {
    add_reviewer(value);
    autonomous(value);
}

/// A guarded default consumer and an autonomous reviewer.
fn autonomous_reviewer(value: &mut Value) {
    add_reviewer(value);
    let consumer = &mut value["routing"]["consumers"][0];
    consumer["profile"]["permission"]["action_mode"] = "autonomous".into();
    consumer["profile_digest"] = routing_fixture::digest_of(&consumer["profile"]).into();
}

/// The download view of attachment 0 under `reference` with `key`.
async fn download_view(app: &App, reference: &EventRef, key: &str) -> Value {
    let path = format!(
        "/v1/events/{}/attachments/0?view=download",
        reference.as_str()
    );
    let response = call(app, Method::GET, &path, key, None).await;
    assert_eq!(response.status(), 200);
    response.json().await.unwrap()
}

/// The `inbound_media` the context reports under `reference` with `key`.
async fn inbound_media(app: &App, reference: &EventRef, key: &str) -> Value {
    let path = format!("/v1/events/{}/context", reference.as_str());
    let response = call(app, Method::GET, &path, key, None).await;
    assert_eq!(response.status(), 200);
    response.json::<Value>().await.unwrap()["capabilities"]["inbound_media"].clone()
}

#[tokio::test]
async fn a_guarded_reviewer_gets_its_own_audience_not_the_autonomous_defaults() {
    let app = app(autonomous_default).await;
    let theirs = issue(&app, &reviewer(), &reviewer_key(), None).await;
    let view = download_view(&app, &theirs, &reviewer_key()).await;
    assert_eq!(
        (view["status"].as_str(), view["reason"].as_str()),
        (Some("unavailable"), Some("download_not_permitted")),
        "{view}"
    );
    assert_eq!(
        inbound_media(&app, &theirs, &reviewer_key()).await["downloads"],
        false
    );
    let ours = issue(&app, &ConsumerId::default_consumer(), &app.key, None).await;
    assert_eq!(
        download_view(&app, &ours, &app.key).await["status"],
        "ready"
    );
    assert_eq!(
        inbound_media(&app, &ours, &app.key).await["downloads"],
        true
    );
}

#[tokio::test]
async fn an_autonomous_reviewer_mints_and_redeems_a_link_for_its_own_binding() {
    let app = app(autonomous_reviewer).await;
    let ours = issue(&app, &ConsumerId::default_consumer(), &app.key, None).await;
    let refused = download_view(&app, &ours, &app.key).await;
    assert_eq!(refused["reason"], "download_not_permitted", "{refused}");
    let theirs = issue(&app, &reviewer(), &reviewer_key(), None).await;
    let view = download_view(&app, &theirs, &reviewer_key()).await;
    assert_eq!(view["status"], "ready", "{view}");
    let curl = view["download"]["curl"].as_str().unwrap();
    let start = curl.find("Bearer ").unwrap() + "Bearer ".len();
    let token = &curl[start..start + curl[start..].find('\'').unwrap()];
    let response = download(&app, token).await;
    assert_eq!(response.status(), 200);
    assert_eq!(response.bytes().await.unwrap(), photo());
}
