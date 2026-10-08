//! Source attach and detach from the console, through the record-keyed
//! seam: forms, the one-time reveal of a generated secret, refusals that
//! never echo a secret, confirmation, and no write while unavailable.
use std::sync::atomic::Ordering;

use cmaeg::console::source_changes::SourceAvailability;
use cmaeg_core::{channel::Source, ids::RecordId};
use tracing_subscriber::{layer::SubscriberExt, Layer};

use super::{
    pages_support::{definition, record, World},
    support::{csrf_for, Captured},
};

const ATTACH: &str = "/console/channels/{record_id}/sources";
const DETACH: &str = "/console/channels/{record_id}/sources/{source_id}/detach";

/// Alice's channel with only a Lark relay and no known channel id.
fn relay_only(world: &World) -> RecordId {
    let record = record(1);
    let mut stored = definition(&record, "alice", "alice-orders");
    stored
        .sources
        .retain(|source| matches!(source, Source::NyxidRelay { .. }));
    stored.creation = None;
    world.store(stored, 1);
    record
}

async fn sources_page(world: &World, record: &RecordId, sid: &str) -> String {
    let path = format!("/console/channels/{record}?tab=sources");
    let response = world.harness.get_as(&path, sid).await;
    assert_eq!(response.status(), 200);
    response.text().await.unwrap()
}

#[tokio::test]
async fn while_source_changes_are_unavailable_nothing_is_offered_or_written() {
    for availability in [None, Some(SourceAvailability::Disabled)] {
        let world = match availability {
            None => World::start().await,
            Some(availability) => World::with_sources(availability).await,
        };
        let orders = relay_only(&world);
        let (sid, _) = world.harness.sign_in("alice", "/console").await;
        let page = sources_page(&world, &orders, &sid).await;
        assert!(!page.contains("Attach a source") && !page.contains(">Detach<"));
        let (status, text) = match availability {
            None => (501, "not available in this gateway release yet"),
            Some(_) => (400, "disabled by the operator"),
        };
        assert!(page.contains(text));
        let attach = world.csrf(&sid, ATTACH, &orders);
        let detach = world.csrf(&sid, DETACH, &orders);
        let detach_path = format!("/console/channels/{orders}/sources/relay_lark/detach");
        for response in [
            world
                .harness
                .post(
                    &format!("/console/channels/{orders}/sources"),
                    Some(&sid),
                    None,
                    &[
                        ("csrf", &attach),
                        ("expected_version", "1"),
                        ("type", "telegram"),
                        ("bot_id", "7000000002"),
                    ],
                )
                .await,
            world.harness.get_as(&detach_path, &sid).await,
            world
                .harness
                .post(
                    &detach_path,
                    Some(&sid),
                    None,
                    &[("csrf", &detach), ("expected_version", "1")],
                )
                .await,
        ] {
            assert_eq!(response.status(), status);
            assert!(response.text().await.unwrap().contains(text));
        }
        if availability.is_some() {
            assert_eq!(world.changes().calls.load(Ordering::SeqCst), 0);
        }
        assert_eq!(world.reader.channels.lock().unwrap()[&orders].1, 1);
        world.close().await;
    }
}

#[tokio::test]
async fn an_attach_redirects_to_sources_and_a_refreshed_post_conflicts() {
    let world = World::with_sources(SourceAvailability::Available).await;
    let orders = relay_only(&world);
    let (sid, _) = world.harness.sign_in("alice", "/console").await;
    let page = sources_page(&world, &orders, &sid).await;
    let action = format!("/console/channels/{orders}/sources");
    assert!(page.contains("Attach a Telegram bot"));
    assert!(page.contains(&format!(
        "href=\"/console/channels/{orders}/sources/relay_lark/detach\">Detach</a>"
    )));
    let csrf = csrf_for(&page, &action);
    let fields = [
        ("csrf", csrf.as_str()),
        ("expected_version", "1"),
        ("type", "telegram"),
        ("bot_id", "7000000002"),
        ("bot_username", "second_bot"),
        ("admission", "all"),
        ("telegram_bot_token", "7000000002:typed-token-SECRET"),
    ];
    let response = world.harness.post(&action, Some(&sid), None, &fields).await;
    assert_eq!(response.status(), 303);
    assert_eq!(
        response.headers()["location"],
        format!("/console/channels/{orders}?tab=sources").as_str()
    );
    let after = sources_page(&world, &orders, &sid).await;
    assert!(after.contains("@second_bot · id 7000000002"));
    assert!(after.contains("name=\"expected_version\" value=\"2\""));
    // A browser refresh re-posts the stale version.
    let refreshed = world.harness.post(&action, Some(&sid), None, &fields).await;
    assert_eq!(refreshed.status(), 409);
    let body = refreshed.text().await.unwrap();
    assert!(body.contains("This channel changed since the page was loaded."));
    assert!(!body.contains("typed-token-SECRET"));
    assert_eq!(world.changes().calls.load(Ordering::SeqCst), 2);
    world.close().await;
}

#[tokio::test]
async fn a_generated_secret_is_revealed_once_and_never_leaks() {
    let logs = Captured::default();
    let subscriber = tracing_subscriber::registry().with(
        tracing_subscriber::fmt::layer()
            .with_ansi(false)
            .with_writer(logs.clone())
            .with_filter(cmaeg::log_layer_filter(Some("trace"))),
    );
    let _guard = tracing::subscriber::set_default(subscriber);
    let world = World::with_sources(SourceAvailability::Available).await;
    let orders = relay_only(&world);
    let (sid, _) = world.harness.sign_in("alice", "/console").await;
    let action = format!("/console/channels/{orders}/sources");
    let csrf = csrf_for(&sources_page(&world, &orders, &sid).await, &action);
    let fields = [
        ("csrf", csrf.as_str()),
        ("expected_version", "1"),
        ("type", "custom"),
        ("auth", "hmac"),
        ("key_id", "generated-key"),
        ("generate_secret", "on"),
        ("partition_custom_key", "event_subject"),
    ];
    let response = world.harness.post(&action, Some(&sid), None, &fields).await;
    assert_eq!(response.status(), 200);
    assert!(response.headers().get("location").is_none());
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert!(response.headers()["content-security-policy"]
        .to_str()
        .unwrap()
        .starts_with("default-src 'none'"));
    let reveal = response.text().await.unwrap();
    let secrets = world.changes().secrets.lock().unwrap().clone();
    assert_eq!(secrets.len(), 1, "the seam received one generated secret");
    let secret = &secrets[0];
    assert_eq!(secret.len(), 44);
    let (before, inside) = reveal.split_once("<details class=\"reveal\">").unwrap();
    let (summary, rest) = inside.split_once("</summary>").unwrap();
    let (content, after) = rest.split_once("</details>").unwrap();
    assert!(!before.contains(secret.as_str()) && !after.contains(secret.as_str()));
    assert!(!summary.contains(secret.as_str()));
    assert!(summary.contains(&format!("…{}", &secret[40..])));
    assert_eq!(content.matches(secret.as_str()).count(), 1);
    assert!(!reveal.to_lowercase().contains("<script"));

    // A refresh re-posts the stale version: 409, and no second secret.
    let refreshed = world.harness.post(&action, Some(&sid), None, &fields).await;
    assert_eq!(refreshed.status(), 409);
    let refreshed = refreshed.text().await.unwrap();
    assert!(!refreshed.contains(secret.as_str()));
    let page = sources_page(&world, &orders, &sid).await;
    assert!(!page.contains(secret.as_str()));
    assert!(page.contains("<code>generated-key</code>"));
    let logged = logs.text();
    assert!(logged.contains("console_action"), "the log capture is live");
    assert!(logged.contains("outcome=\"ok\"") && logged.contains("source_id=\"custom\""));
    assert!(
        !logged.contains(secret.as_str()),
        "a log line holds the secret"
    );
    world.close().await;
}

#[tokio::test]
async fn refused_attaches_re_render_without_any_submitted_secret() {
    let world = World::with_sources(SourceAvailability::Available).await;
    let orders = relay_only(&world);
    let (sid, _) = world.harness.sign_in("alice", "/console").await;
    let action = format!("/console/channels/{orders}/sources");
    let csrf = csrf_for(&sources_page(&world, &orders, &sid).await, &action);
    for (fields, status, message) in [
        (
            vec![
                ("type", "telegram"),
                ("bot_id", "refuse"),
                ("telegram_bot_token", "typed-bot-token-SECRET"),
                ("telegram_webhook_secret", "typed-webhook-SECRET"),
            ],
            400,
            "Refused at /source/bot_id: must be a Telegram bot id",
        ),
        (
            vec![
                ("type", "nyxid_relay"),
                ("issuer", "https://nyxid.example"),
                ("key_id", "typed-relay-key"),
                ("platform", "carrier-pigeon"),
                ("channel_key", "typed-channel-key-SECRET"),
            ],
            400,
            "Choose a platform.",
        ),
        (
            vec![
                ("type", "custom"),
                ("auth", "hmac"),
                ("key_id", "typed-key"),
                ("hmac_secret", "typed-hmac-SECRET"),
                ("schema", "{not json"),
            ],
            400,
            "The event schema must be JSON.",
        ),
    ] {
        let mut posted = vec![("csrf", csrf.as_str()), ("expected_version", "1")];
        posted.extend(fields.iter().copied());
        let response = world.harness.post(&action, Some(&sid), None, &posted).await;
        assert_eq!(response.status(), status, "{message}");
        let body = response.text().await.unwrap();
        assert!(body.contains(message), "{message}");
        assert!(!body.contains("SECRET"), "{message}: a secret was echoed");
        // Non-secret fields are kept for the owner to correct.
        let kept = fields
            .iter()
            .find(|(name, _)| ["bot_id", "key_id"].contains(name))
            .unwrap()
            .1;
        assert!(body.contains(&format!("value=\"{kept}\"")), "{message}");
    }
    // Only the first reached management; the others were refused while
    // parsing, before any call.
    assert_eq!(world.changes().calls.load(Ordering::SeqCst), 1);
    world.close().await;
}

#[tokio::test]
async fn a_detach_is_confirmed_then_made_by_record_without_a_channel_id() {
    let world = World::with_sources(SourceAvailability::Available).await;
    let orders = world.add(1, "alice", "alice-orders");
    world
        .reader
        .channels
        .lock()
        .unwrap()
        .get_mut(&orders)
        .unwrap()
        .0
        .creation = None;
    let (sid, _) = world.harness.sign_in("alice", "/console").await;
    let confirm = format!("/console/channels/{orders}/sources/custom/detach");
    let page = world.harness.get_as(&confirm, &sid).await;
    assert_eq!(page.status(), 200);
    let page = page.text().await.unwrap();
    assert!(
        !page.contains("cmaeg1.ch."),
        "no channel id is known or needed"
    );
    for fact in [
        "Detach <code>custom</code>?",
        "Blocking references",
        "Rules that become inactive",
        "name=\"expected_version\" value=\"1\"",
    ] {
        assert!(page.contains(fact), "{fact}");
    }
    let csrf = csrf_for(&page, &confirm);
    *world.changes().default_reply.lock().unwrap() = Some("relay_lark".into());
    let lark = format!("/console/channels/{orders}/sources/relay_lark/detach");
    let refused = world
        .harness
        .post(
            &lark,
            Some(&sid),
            None,
            &[("csrf", &csrf), ("expected_version", "1")],
        )
        .await;
    assert_eq!(refused.status(), 409);
    assert!(refused
        .text()
        .await
        .unwrap()
        .contains("default reply source"));

    let response = world
        .harness
        .post(
            &confirm,
            Some(&sid),
            None,
            &[("csrf", &csrf), ("expected_version", "1")],
        )
        .await;
    assert_eq!(response.status(), 303);
    assert_eq!(
        response.headers()["location"],
        format!("/console/channels/{orders}?tab=sources").as_str()
    );
    assert_eq!(
        world.changes().detached.lock().unwrap().as_slice(),
        [(
            orders.clone(),
            cmaeg_core::channel::SourceId::parse("custom").unwrap(),
            1
        )]
    );
    let after = sources_page(&world, &orders, &sid).await;
    assert!(!after.contains("<code>custom</code>"));
    // The detached source is gone: its confirmation is 404, and a stale
    // detach of another source conflicts.
    assert_eq!(world.harness.get_as(&confirm, &sid).await.status(), 404);
    let stale = world
        .harness
        .post(
            &format!("/console/channels/{orders}/sources/telegram/detach"),
            Some(&sid),
            None,
            &[("csrf", &csrf), ("expected_version", "1")],
        )
        .await;
    assert_eq!(stale.status(), 409);
    world.close().await;
}

#[tokio::test]
async fn another_owners_channel_is_404_for_every_source_change() {
    let world = World::with_sources(SourceAvailability::Available).await;
    let billing = world.add(3, "bob", "bob-billing");
    let (sid, _) = world.harness.sign_in("alice", "/console").await;
    let attach = world.csrf(&sid, ATTACH, &billing);
    let detach = world.csrf(&sid, DETACH, &billing);
    let detach_path = format!("/console/channels/{billing}/sources/custom/detach");
    for response in [
        world
            .harness
            .post(
                &format!("/console/channels/{billing}/sources"),
                Some(&sid),
                None,
                &[
                    ("csrf", &attach),
                    ("expected_version", "1"),
                    ("type", "telegram"),
                    ("bot_id", "7"),
                ],
            )
            .await,
        world.harness.get_as(&detach_path, &sid).await,
        world
            .harness
            .post(
                &detach_path,
                Some(&sid),
                None,
                &[("csrf", &detach), ("expected_version", "1")],
            )
            .await,
    ] {
        assert_eq!(response.status(), 404);
        assert!(!response.text().await.unwrap().contains("bob-billing"));
    }
    assert_eq!(world.changes().calls.load(Ordering::SeqCst), 0);
    world.close().await;
}

#[tokio::test]
async fn only_attached_source_slots_can_be_detached() {
    let world = World::with_sources(SourceAvailability::Available).await;
    let orders = relay_only(&world);
    let (sid, _) = world.harness.sign_in("alice", "/console").await;
    let sources = sources_page(&world, &orders, &sid).await;
    assert!(sources.contains(&format!(
        "href=\"/console/channels/{orders}/sources/relay_lark/detach\">Detach</a>"
    )));
    let attached = format!("/console/channels/{orders}/sources/relay_lark/detach");
    assert_eq!(world.harness.get_as(&attached, &sid).await.status(), 200);
    // A transport name, a spelling outside the slot grammar and a slot the
    // channel does not hold name no source of it.
    for missing in ["nyxid_relay", "relay_LARK", "relay_slack", "telegram"] {
        let path = format!("/console/channels/{orders}/sources/{missing}/detach");
        assert_eq!(
            world.harness.get_as(&path, &sid).await.status(),
            404,
            "{path}"
        );
    }
    assert_eq!(world.changes().calls.load(Ordering::SeqCst), 0);
    world.close().await;
}
