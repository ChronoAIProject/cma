//! Every console page against a channel whose owner-controlled strings are
//! markup and whose record holds every kind of secret: everything is
//! escaped, nothing secret or re-sealed is rendered, and no page carries a
//! script.
use cmaeg::console::{
    observer::{ActivitySeed, Observation},
    source_changes::SourceAvailability,
};
use cmaeg_core::channel::{CustomAuth, Source};
use time::OffsetDateTime;

use super::pages_support::{channel_id, definition, record, World, AGENT_KEY_HASH, SECRETS, TABS};

const SCRIPT: &str = "<script>alert(1)</script>";
const IMG: &str = "\"><img src=x>";
const JS: &str = "javascript:alert(1)";

/// Every page of `world`'s fixture channel, as `(path, status, body)`.
async fn every_page(world: &World, record: &str, sid: &str) -> Vec<(String, u16, String)> {
    let mut paths = vec![
        "/console".to_owned(),
        format!("/console?q={}", "%3Cscript%3E"),
    ];
    paths.extend(
        TABS.iter()
            .map(|tab| format!("/console/channels/{record}?tab={tab}")),
    );
    paths.push(format!(
        "/console/channels/{record}/events/0123456789abcdef"
    ));
    paths.push(format!("/console/channels/{record}/sources/custom/detach"));
    let mut pages = Vec::new();
    for path in paths {
        let response = world.harness.get_as(&path, sid).await;
        let status = response.status().as_u16();
        pages.push((path, status, response.text().await.unwrap()));
    }
    pages
}

#[tokio::test]
async fn every_owner_controlled_string_is_escaped_and_no_secret_is_rendered() {
    let world = World::with_sources(SourceAvailability::Available).await;
    let target = record(1);
    let mut stored = definition(&target, "alice", SCRIPT);
    for source in &mut stored.sources {
        match source {
            Source::Telegram { bot_username, .. } => *bot_username = Some(IMG.into()),
            Source::NyxidRelay {
                issuer,
                bot_username,
                key_id,
                ..
            } => {
                *issuer = JS.into();
                *bot_username = Some(SCRIPT.into());
                *key_id = IMG.into();
            }
            Source::Custom { auth, .. } => {
                *auth = CustomAuth::Hmac {
                    key_id: SCRIPT.into(),
                }
            }
        }
    }
    stored.secrets.hmac_secrets =
        vec![(SCRIPT.into(), cmaeg_core::channel::Secret::new(SECRETS[1]))];
    world.store(stored, 1);
    world.traffic(&target, &format!("{JS}//{SCRIPT}"));
    world.memory.apply(Observation::Routed {
        record: target.clone(),
        entry_seed: ActivitySeed {
            at: OffsetDateTime::now_utc(),
            event_key: "00000000000000ff".into(),
            source: SCRIPT.into(),
            kind: "message",
            actor_masked: IMG.into(),
            chat_masked: SCRIPT.into(),
            attachments: [0; 4],
        },
    });
    let (sid, _) = world.harness.sign_in("alice", "/console").await;
    let pages = every_page(&world, target.as_str(), &sid).await;
    let shown_id = channel_id(&target).to_string();
    for (path, status, body) in &pages {
        assert_eq!(*status, 200, "{path}");
        let lower = body.to_lowercase();
        assert!(!lower.contains("<script"), "{path} carries a script");
        assert!(!lower.contains("<img"), "{path} carries an image tag");
        assert!(
            !lower.contains("href=\"javascript:"),
            "{path} links a script URL"
        );
        for secret in SECRETS
            .iter()
            .chain([&AGENT_KEY_HASH, &"ou_sender_fixture_1"])
        {
            assert!(!body.contains(secret), "{path} shows {secret}");
        }
        // The only channel id ever rendered is the one the record retained.
        assert_eq!(
            body.matches("cmaeg1.ch.").count(),
            body.matches(&shown_id).count(),
            "{path} renders a channel id it was not given"
        );
    }
    let all: String = pages.iter().map(|(_, _, body)| body.as_str()).collect();
    for escaped in [
        "&lt;script&gt;alert(1)&lt;/script&gt;",
        "&quot;&gt;&lt;img src=x&gt;",
        "javascript:alert(1)//&lt;script&gt;",
    ] {
        assert!(all.contains(escaped), "{escaped} is not shown escaped");
    }
    world.close().await;
}
