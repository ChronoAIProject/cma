//! The real NyxID source against a bounded loopback contract, including current records.
use super::*;
use crate::{
    channels::resolver::ChannelResolver,
    media::source::{MediaSource, SourceCredential},
    records::RecordClient,
    ChannelCacheConfig,
};
use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode, Uri},
    response::IntoResponse,
    Router,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use cmaeg_core::{
    activity::{Activity, ActivityKind, Attachment, AttachmentKind, MediaVariant},
    channel::{ChannelDefinition, Platform},
    ids::ChannelId,
    seal::{seal_blob, ChannelPointer, EventRefPayload, MasterKeyRing},
};
use serde_json::json;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

pub const KEY: &str = "resource-channel-fixture";
pub const MEDIA: &str = "/api/v1/proxy/catalog-one/open-apis/im/v1/messages/om_native/resources/img_resource?type=image&_nyxid_via=connection-one";
pub const BOT: &str = "/api/v1/proxy/catalog-one/open-apis/bot/v3/info?_nyxid_via=connection-one";

#[derive(Clone)]
pub struct Reply {
    pub status: u16,
    pub body: Vec<u8>,
    pub content_type: &'static str,
    pub delay: Duration,
}
impl Reply {
    pub fn json(value: Value) -> Self {
        Self {
            status: 200,
            body: serde_json::to_vec(&value).unwrap(),
            content_type: "application/json",
            delay: Duration::ZERO,
        }
    }
}

#[derive(Clone, Default)]
pub struct Server {
    pub replies: Arc<Mutex<BTreeMap<String, Reply>>>,
    pub requests: Arc<Mutex<Vec<(String, String)>>>,
}
impl Server {
    pub fn set(&self, path: &str, reply: Reply) {
        self.replies.lock().unwrap().insert(path.into(), reply);
    }
    pub fn paths(&self) -> Vec<String> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .map(|(path, _)| path.clone())
            .collect()
    }
}

async fn handle(State(state): State<Server>, uri: Uri, headers: HeaderMap) -> impl IntoResponse {
    let path = uri.path_and_query().unwrap().as_str();
    state.requests.lock().unwrap().push((
        path.into(),
        headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .into(),
    ));
    let reply = state
        .replies
        .lock()
        .unwrap()
        .get(path)
        .cloned()
        .unwrap_or(Reply {
            status: 404,
            body: Vec::new(),
            content_type: "text/plain",
            delay: Duration::ZERO,
        });
    tokio::time::sleep(reply.delay).await;
    (
        StatusCode::from_u16(reply.status).unwrap(),
        [
            ("content-type", reply.content_type),
            ("location", "/redirect-trap"),
        ],
        Bytes::from(reply.body),
    )
}

pub struct Fixture {
    pub server: Server,
    pub source: Arc<nyxid::NyxidSource>,
    pub channel: Arc<ResolvedChannel>,
    pub definition: ChannelDefinition,
    pub keys: Arc<MasterKeyRing>,
    pub resolver: ChannelResolver,
    pub activity: Activity,
    pub event: EventRefPayload,
    pub locator: AttachmentSource,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Fixture {
    pub async fn new(platform: Platform) -> Self {
        let server = Server::default();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let router = Router::new().fallback(handle).with_state(server.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let keys =
            Arc::new(MasterKeyRing::parse(&format!("1:{}", STANDARD.encode([1; 32]))).unwrap());
        let records = Arc::new(RecordClient::new(&base, "record-fixture").unwrap());
        let mut definition =
            crate::channels::definition::fixture::with_agent_key("resource-agent-fixture");
        let binding = super::tests::binding();
        definition.sources = vec![serde_json::from_value(json!({"type":"nyxid_relay","issuer":"https://nyx.example","jwks_url":"https://nyx.example/jwks","key_id":"key-1","route_ids":["route-one"],"platform":platform,"bot_id":"ou_source","lark_resources":binding,"admission":{"type":"all"}})).unwrap()];
        definition.record_id = uuid::Uuid::new_v4().to_string().parse().unwrap();
        definition.secrets.channel_key = Some(Secret::new(KEY));
        definition.partition.key = cmaeg_core::channel::PartitionKey::ConversationAndSender;
        definition.validate().unwrap();
        let pointer = ChannelPointer {
            v: 1,
            record_id: definition.record_id.clone(),
            record_url: records
                .record_url(definition.record_id.as_str())
                .unwrap()
                .to_string(),
        };
        let id: ChannelId = pointer.seal_id(&keys).unwrap();
        let resolver = ChannelResolver::new(records, keys.clone(), ChannelCacheConfig::default());
        let record_path = format!("/api/v1/gateway/channel-records/{}", definition.record_id);
        server.set(&record_path, stored(&definition, &keys, 1));
        let channel = resolver.resolve(id.as_str()).await.unwrap();
        let locator = AttachmentSource::LarkResource {
            message_id: "om_native".into(),
            resource_key: "img_resource".into(),
            resource_kind: cmaeg_core::channel::LarkResourceKind::Image,
            binding,
            bot_id: "ou_source".into(),
            source_revision: revision(
                &definition.sources[0],
                definition.secrets.channel_key.as_ref().unwrap(),
            )
            .unwrap(),
        };
        let mut activity: Activity = serde_json::from_str(include_str!(
            "../../../../cmaeg-core/tests/fixtures/activity.json"
        ))
        .unwrap();
        activity.source = SourceKind::NyxidRelay {
            platform,
            route_id: "route-one".into(),
        };
        activity.kind = ActivityKind::Message {
            text: "post".into(),
            mentions_bot: false,
            reply_to: None,
            album_id: None,
            attachments: vec![Attachment {
                reference: "lark:img_resource".into(),
                kind: AttachmentKind::Image,
                filename: None,
                mime_type: None,
                size_bytes: None,
                source: Some(locator.clone()),
                variant: Some(MediaVariant::Embedded),
                duration_seconds: None,
                width: None,
                height: None,
                thumbnail: None,
                emoji: None,
            }],
        };
        activity.validate().unwrap();
        let event = EventRefPayload {
            v: 1,
            channel_id: id,
            record_id: definition.record_id.clone(),
            event_id: activity.event_id.clone(),
            partition_key: "post".into(),
            conversation_id: activity.conversation.id.parse().unwrap(),
            expires_at: time::OffsetDateTime::now_utc().unix_timestamp() + 1800,
            reply: ReplyAddress::NyxidRelay {
                thread_reply: false,
                message_id: "relay-envelope-different".into(),
                platform,
                route_id: "route-one".into(),
                reply_token: Secret::new("reply-token-must-not-be-used"),
                reply_token_expires_at: i64::MAX,
            },
            consumer: None,
            lineage: None,
            telegram_admission: None,
            activity_sha256: Some(cmaeg_core::seal::event_ref::activity_digest(&activity)),
        };
        let (mut services, mut endpoints, mut catalog) = super::tests::metadata();
        let (slug, origin) = lark_resource_service(platform).unwrap();
        services["services"][0]["slug"] = slug.into();
        endpoints["endpoints"][0]["url"] = origin.into();
        catalog["slug"] = slug.into();
        catalog["base_url"] = origin.into();
        server.set("/api/v1/user-services", Reply::json(services));
        server.set("/api/v1/endpoints", Reply::json(endpoints));
        server.set("/api/v1/services/catalog-one", Reply::json(catalog));
        server.set(
            BOT,
            Reply::json(json!({"code":0,"bot":{"open_id":"ou_source"}})),
        );
        server.set(
            MEDIA,
            Reply {
                status: 200,
                body: b"opaque-image-fixture".to_vec(),
                content_type: "image/png",
                delay: Duration::ZERO,
            },
        );
        Self {
            server,
            source: Arc::new(
                nyxid::NyxidSource::new(&base)
                    .unwrap()
                    .with_resolver(resolver.clone()),
            ),
            resolver,
            channel,
            definition,
            keys,
            activity,
            event,
            locator,
            task,
        }
    }
    pub fn store(&self, version: u64) {
        self.server.set(
            &format!(
                "/api/v1/gateway/channel-records/{}",
                self.definition.record_id
            ),
            stored(&self.definition, &self.keys, version),
        );
    }
    pub fn request(&self) -> FetchRequest<'_> {
        FetchRequest {
            channel: &self.channel,
            event: &self.event,
            activity: &self.activity,
            locator: Some(&self.locator),
            declared_size: None,
            credential: SourceCredential::ReplyToken,
            cap: 1024,
            overflow: Overflow::Reject,
            deadline: Instant::now() + Duration::from_secs(5),
        }
    }
    pub async fn fetch(&self) -> Result<Vec<u8>, FetchError> {
        self.source
            .fetch(self.request())
            .await
            .map(|body| body.bytes.to_vec())
    }
}
fn stored(definition: &ChannelDefinition, keys: &MasterKeyRing, version: u64) -> Reply {
    let blob = seal_blob(keys, definition.to_blob_json().to_string().as_bytes()).unwrap();
    Reply::json(
        json!({"record_id":definition.record_id,"blob":STANDARD.encode(blob),"version":version}),
    )
}
