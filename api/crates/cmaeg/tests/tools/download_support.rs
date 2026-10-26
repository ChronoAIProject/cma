//! Media-link helpers: autonomous bindings on each source, links sealed at
//! runtime exactly as minting seals them, and channel redefinition.
use super::{relay_support::now, support::App};
use cmaeg_core::{
    activity::AttachmentSource,
    channel::ChannelDefinition,
    seal::{media_link, seal_blob, MediaLinkPayload},
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub fn relay(index: u8) -> AttachmentSource {
    AttachmentSource::NyxidRelay { index }
}

/// The relay channel with an autonomous binding.
pub fn autonomous(value: &mut Value) {
    value["profile"]["permission"]["action_mode"] = "autonomous".into();
}

/// [`autonomous`], also receiving natively from Telegram bot 77.
pub fn autonomous_native(value: &mut Value) {
    autonomous(value);
    value["sources"]
        .as_array_mut()
        .unwrap()
        .push(json!({"type":"telegram","bot_id":"77","bot_username":"fixture_bot","admission":{"type":"all"}}));
    value["secrets"]["telegram_bot_token"] = "77:fixture".into();
    value["secrets"]["telegram_webhook_secret"] = "native-webhook-fixture".into();
}

/// [`autonomous`], also receiving custom events whose attachments may come
/// from `https://files.example`.
pub fn autonomous_custom(value: &mut Value) {
    autonomous(value);
    value["sources"].as_array_mut().unwrap().push(json!({"type":"custom","auth":{"type":"hmac","key_id":"key-1"},"attachments":{"origins":["https://files.example"]}}));
    value["partition"]["key"] = "fixed".into();
}

/// A link to attachment `index` of the app's event for the app's binding and
/// agent key, issued at `issued_at` for `ttl` seconds: the token and its
/// payload.
pub fn link_at(app: &App, index: u8, issued_at: i64, ttl: i64) -> (String, MediaLinkPayload) {
    let keys = &app.state.keys;
    let event = app.event_ref.open(keys, now()).unwrap().payload;
    let hash: [u8; 32] = Sha256::digest(app.key.as_bytes()).into();
    let payload = MediaLinkPayload::new(
        "cma_codex".parse().unwrap(),
        "binding-1".parse().unwrap(),
        &hash,
        event,
        index,
        issued_at,
        ttl,
    )
    .unwrap();
    let token = media_link::seal(keys, &payload).unwrap();
    (token.expose_header_value().to_owned(), payload)
}

/// A fresh two-minute link to attachment `index`.
pub fn link(app: &App, index: u8) -> String {
    link_at(app, index, now(), 120).0
}

/// GET the download route with `token` as the bearer.
pub async fn download(app: &App, token: &str) -> reqwest::Response {
    app.fixture
        .client
        .get(format!("{}/v1/media/content", app.base))
        .bearer_auth(token)
        .send()
        .await
        .unwrap()
}

/// GET `{base}/v1/media/content{query}` with one `Authorization` header per
/// entry of `authorization`.
pub async fn download_with(app: &App, query: &str, authorization: &[String]) -> reqwest::Response {
    let mut request = app
        .fixture
        .client
        .get(format!("{}/v1/media/content{query}", app.base));
    for value in authorization {
        request = request.header("authorization", value);
    }
    request.send().await.unwrap()
}

/// Store `edit`ed as the channel's definition and drop the cached one, as
/// an owner's update does.
pub async fn redefine(app: &App, edit: impl FnOnce(&mut ChannelDefinition)) {
    let current = app
        .state
        .resolver
        .resolve(app.fixture.id.as_str())
        .await
        .unwrap();
    let mut definition = current.definition.clone();
    edit(&mut definition);
    definition.profile_digest = definition.profile.digest();
    definition.validate().unwrap();
    *app.fixture.backend.blob.lock().unwrap() = seal_blob(
        &app.state.keys,
        definition.to_blob_json().to_string().as_bytes(),
    )
    .unwrap();
    app.state.resolver.invalidate(app.fixture.id.as_str());
}

/// Assert the exact hardened head of a served original and return its body.
pub async fn served(response: reqwest::Response, name: &str, length: Option<usize>) -> Vec<u8> {
    assert_eq!(response.status(), 200);
    let headers = response.headers();
    assert_eq!(headers["content-type"], "application/octet-stream");
    assert_eq!(
        headers["content-disposition"],
        format!("attachment; filename=\"{name}\"").as_str()
    );
    assert_eq!(headers["x-content-type-options"], "nosniff");
    assert_eq!(
        headers["content-security-policy"],
        "default-src 'none'; sandbox"
    );
    assert_eq!(headers["cache-control"], "no-store");
    assert_eq!(headers["referrer-policy"], "no-referrer");
    let mut names: Vec<_> = headers.keys().map(|name| name.as_str()).collect();
    names.sort_unstable();
    let mut expected = vec![
        "cache-control",
        "cmaeg-protocol-version",
        "content-disposition",
        "content-security-policy",
        "content-type",
        "date",
        "referrer-policy",
        "x-content-type-options",
        "x-request-id",
    ];
    match length {
        Some(length) => {
            assert_eq!(headers["content-length"], length.to_string().as_str());
            expected.push("content-length");
        }
        None => expected.push("transfer-encoding"),
    }
    expected.sort_unstable();
    assert_eq!(names, expected);
    response.bytes().await.unwrap().to_vec()
}
