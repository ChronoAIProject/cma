use super::{
    normalization_tests::{channel, raw},
    normalize::{normalize, Normalized},
    rest_fixture::Fixture,
};
use crate::media::{
    body::Overflow,
    source::{
        discord::{attachment_url, DiscordFileSource},
        url::UrlTransport,
        FetchRequest, MediaSource, SourceCredential,
    },
    FetchError,
};
use cmaeg_core::{
    activity::ActivityKind,
    channel::AdmissionPolicy,
    seal::{EventRefPayload, ReplyAddress},
};
use futures_util::future::BoxFuture;
use serde_json::json;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::time::Instant;
struct Download {
    origin: String,
    calls: Mutex<Vec<String>>,
}
impl UrlTransport for Download {
    fn get<'a>(
        &'a self,
        url: &'a reqwest::Url,
        _: Instant,
    ) -> BoxFuture<'a, Result<reqwest::Response, FetchError>> {
        self.calls.lock().unwrap().push(url.to_string());
        Box::pin(async move {
            reqwest::Client::new()
                .get(&self.origin)
                .send()
                .await
                .map_err(|_| FetchError::Upstream)
        })
    }
}
#[test]
fn cdn_url_must_bind_official_origin_exact_channel_and_attachment() {
    assert!(attachment_url(
        "https://cdn.discordapp.com/attachments/99/500/image.png?ex=123&hm=abc",
        "99",
        "500"
    )
    .is_ok());
    for url in [
        "https://attacker.example/attachments/99/500/image.png",
        "http://cdn.discordapp.com/attachments/99/500/image.png",
        "https://cdn.discordapp.com@attacker.example/attachments/99/500/image.png",
        "https://cdn.discordapp.com/attachments/98/500/image.png",
        "https://cdn.discordapp.com/attachments/99/501/image.png",
        "https://cdn.discordapp.com:8443/attachments/99/500/image.png",
    ] {
        assert_eq!(
            attachment_url(url, "99", "500").unwrap_err(),
            FetchError::Rejected
        );
    }
}
#[tokio::test]
async fn native_media_refreshes_exact_metadata_keeps_token_off_cdn_and_honors_revocation_and_caps()
{
    let f = Fixture::new().await;
    let download = Fixture::new().await;
    *download.memory.response.lock().unwrap() = vec![1, 2, 3, 4];
    let url = "https://cdn.discordapp.com/attachments/99/500/photo.png?ex=123&hm=private";
    f.respond(
        axum::http::StatusCode::OK,
        json!({"id":"700","channel_id":"99","attachments":[{"id":"500","size":4,"url":url}]}),
    );
    let transport = Arc::new(Download {
        origin: download.base.clone(),
        calls: Mutex::new(vec![]),
    });
    let source = DiscordFileSource::new(f.api.clone()).with_download(transport.clone());
    let mut event = raw();
    event["attachments"] = json!([{"id":"500","filename":"photo.png","size":4}]);
    let Normalized::Activity(activity) = normalize(&event, "77", &AdmissionPolicy::All {}).unwrap()
    else {
        panic!()
    };
    let ActivityKind::Message { attachments, .. } = &activity.kind else {
        panic!()
    };
    let current = channel();
    let payload = EventRefPayload {
        telegram_admission: None,
        turn_trace: None,
        v: 1,
        activity_sha256: Some(cmaeg_core::seal::event_ref::activity_digest(&activity)),
        channel_id: current.id.clone(),
        record_id: current.pointer.record_id.clone(),
        event_id: activity.event_id.clone(),
        partition_key: "discord:99".into(),
        conversation_id: "conversation-1".parse().unwrap(),
        expires_at: i64::MAX,
        reply: ReplyAddress::Discord {
            bot_id: "77".into(),
            channel_id: "99".into(),
            message_id: "700".into(),
            thread_id: None,
        },
        consumer: None,
        lineage: None,
    };
    let request = FetchRequest {
        channel: &current,
        event: &payload,
        activity: &activity,
        locator: attachments[0].source.as_ref(),
        declared_size: Some(4),
        credential: SourceCredential::ChannelKey,
        cap: 10,
        overflow: Overflow::Reject,
        deadline: Instant::now() + Duration::from_secs(5),
    };
    assert_eq!(
        &source.fetch(request).await.unwrap().bytes[..],
        &[1, 2, 3, 4]
    );
    assert_eq!(transport.calls.lock().unwrap().as_slice(), [url]);
    assert!(download.memory.calls.lock().unwrap()[0]
        .authorization
        .is_none());
    assert_eq!(
        f.memory.calls.lock().unwrap()[0].path,
        "/api/v10/channels/99/messages/700"
    );
    assert_eq!(
        source
            .fetch(FetchRequest { cap: 3, ..request })
            .await
            .unwrap_err(),
        FetchError::TooLarge
    );
    let mut revoked = channel();
    Arc::get_mut(&mut revoked)
        .unwrap()
        .definition
        .secrets
        .discord_bot_token = None;
    assert_eq!(
        source
            .fetch(FetchRequest {
                channel: &revoked,
                ..request
            })
            .await
            .unwrap_err(),
        FetchError::NotRetrievable
    );
    assert_eq!(f.memory.calls.lock().unwrap().len(), 2);
    f.respond(axum::http::StatusCode::OK,json!({"id":"700","channel_id":"99","attachments":[{"id":"500","url":"https://attacker.example/attachments/99/500/photo.png"}]}));
    assert_eq!(
        source.fetch(request).await.unwrap_err(),
        FetchError::Rejected
    );
    assert_eq!(transport.calls.lock().unwrap().len(), 1);
}
