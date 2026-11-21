use super::*;
use crate::media::body::Overflow;
use crate::{
    channels::resolver::ResolvedChannel,
    native_lark::{message, test_support as native},
    records::RecordClient,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use cmaeg_core::{
    activity::{Activity, ActivityKind},
    channel::{ChannelDefinition, LarkRegion, Secret, Source},
    seal::{seal_blob, ChannelPointer, EventRefPayload, MasterKeyRing},
};
use serde_json::{json, Value};
use std::sync::Mutex;

pub const BYTES: &[u8] = b"native Lark file content";
pub const TOKEN: &str = "fixture-native-resource-token";
type Hook = Option<Box<dyn FnOnce() + Send>>;
#[derive(Default)]
pub struct Mock {
    pub calls: Mutex<Vec<(String, String)>>,
    pub bot: Mutex<String>,
    pub status: Mutex<u16>,
    pub body: Mutex<Vec<u8>>,
    pub token_hook: Mutex<Hook>,
    pub media_hook: Mutex<Hook>,
}
impl crate::native_lark::Transport for Mock {
    fn exchange(
        &self,
        method: reqwest::Method,
        url: Url,
        headers: reqwest::header::HeaderMap,
        bytes: Vec<u8>,
    ) -> BoxFuture<'_, Result<crate::net::exchange::HttpReply, crate::net::exchange::TransportError>>
    {
        self.calls.lock().unwrap().push((
            url.to_string(),
            headers
                .get("authorization")
                .map(|v| v.to_str().unwrap().into())
                .unwrap_or_default(),
        ));
        let value = if url.path() == "/open-apis/auth/v3/tenant_access_token/internal" {
            assert_eq!(method, reqwest::Method::POST);
            assert_eq!(
                serde_json::from_slice::<Value>(&bytes).unwrap()["app_id"],
                native::identity().app_id
            );
            if let Some(hook) = self.token_hook.lock().unwrap().take() {
                hook();
            }
            json!({"code":0,"tenant_access_token":TOKEN,"expire":7200})
        } else {
            assert_eq!(url.path(), "/open-apis/bot/v3/info");
            assert_eq!(method, reqwest::Method::GET);
            json!({"code":0,"bot":{"open_id":*self.bot.lock().unwrap()}})
        };
        Box::pin(async move {
            Ok(crate::net::exchange::HttpReply::new(
                reqwest::StatusCode::OK,
                serde_json::to_vec(&value).unwrap(),
            ))
        })
    }
}
impl Transport for Mock {
    fn get<'a>(
        &'a self,
        url: &'a Url,
        token: &'a Secret,
        _: Instant,
    ) -> BoxFuture<'a, Result<Response, FetchError>> {
        self.calls
            .lock()
            .unwrap()
            .push((url.to_string(), format!("Bearer {}", token.as_str())));
        if let Some(hook) = self.media_hook.lock().unwrap().take() {
            hook();
        }
        let status = *self.status.lock().unwrap();
        let body = self.body.lock().unwrap().clone();
        Box::pin(async move {
            Ok(Response::from(
                axum::http::Response::builder()
                    .status(status)
                    .header("content-length", body.len())
                    .body(reqwest::Body::from(body))
                    .unwrap(),
            ))
        })
    }
}
pub struct Fixture {
    pub source: Arc<NativeLarkFileSource>,
    pub mock: Arc<Mock>,
    pub channel: Arc<ResolvedChannel>,
    pub definition: ChannelDefinition,
    pub activity: Activity,
    pub event: EventRefPayload,
    pub resolver: ChannelResolver,
    pub keys: Arc<MasterKeyRing>,
    pub stored: Arc<Mutex<Value>>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
pub fn record(definition: &ChannelDefinition, keys: &MasterKeyRing, version: u64) -> Value {
    let blob = seal_blob(keys, definition.to_blob_json().to_string().as_bytes()).unwrap();
    json!({"record_id":definition.record_id,"blob":STANDARD.encode(blob),"version":version})
}
impl Fixture {
    pub async fn new(region: LarkRegion) -> Self {
        let keys = Arc::new(crate::ingress::routing_plan::fixture::keys());
        let c = native::credentials();
        let mut source = native::source();
        let Source::NativeLark(ref mut s) = source else {
            panic!()
        };
        s.identity.region = region;
        let mut value = crate::channels::routing_fixture::routed();
        value["sources"] = json!([source]);
        value["secrets"]["native_lark"] = json!({"app_secret":c.app_secret.as_str(),"encrypt_key":c.encrypt_key.as_str(),"verification_token":c.verification_token.as_str()});
        let mut definition = crate::channels::routing_fixture::definition(value);
        definition.record_id = uuid::Uuid::new_v4().to_string().parse().unwrap();
        definition.validate().unwrap();
        let stored = Arc::new(Mutex::new(record(&definition, &keys, 1)));
        let state = stored.clone();
        let router = axum::Router::new().fallback(move || {
            let value = state.lock().unwrap().clone();
            async move { axum::Json(value) }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let records = Arc::new(RecordClient::new(&base, "fixture-record-key").unwrap());
        let pointer = ChannelPointer {
            v: 1,
            record_id: definition.record_id.clone(),
            record_url: records
                .record_url(definition.record_id.as_str())
                .unwrap()
                .to_string(),
        };
        let id = pointer.seal_id(&keys).unwrap();
        let resolver = ChannelResolver::new(records, keys.clone(), Default::default());
        let channel = resolver.resolve(id.as_str()).await.unwrap();
        let Source::NativeLark(s) = &definition.sources[0] else {
            panic!()
        };
        let mut raw = native::text("om_media", "", crate::ingress::routing_plan::fixture::NOW);
        raw["event"]["message"]["message_type"] = "file".into();
        raw["event"]["message"]["content"] =
            json!({"file_key":"00000000-1111-2222-3333-44444444444g","file_name":"notes.txt"})
                .to_string()
                .into();
        let msg = message::extract(s, &raw, crate::ingress::routing_plan::fixture::NOW)
            .unwrap()
            .unwrap();
        let launch = message::launch(
            channel.clone(),
            s,
            &msg,
            None,
            &keys,
            std::time::Instant::now(),
            crate::ingress::routing_plan::fixture::NOW,
        )
        .unwrap();
        let event = launch
            .event_ref
            .open(&keys, crate::ingress::routing_plan::fixture::NOW)
            .unwrap()
            .payload;
        let mock = Arc::new(Mock::default());
        *mock.bot.lock().unwrap() = native::identity().bot_open_id;
        *mock.status.lock().unwrap() = 200;
        *mock.body.lock().unwrap() = BYTES.to_vec();
        let source = Arc::new(NativeLarkFileSource {
            client: Client::production().unwrap().with_transport(mock.clone()),
            resolver: resolver.clone(),
            transport: mock.clone(),
        });
        Self {
            source,
            mock,
            channel,
            definition,
            activity: launch.activity,
            event,
            resolver,
            keys,
            stored,
            task,
        }
    }
    pub fn store(&self) {
        *self.stored.lock().unwrap() = record(&self.definition, &self.keys, 2);
    }
    pub fn request(&self) -> FetchRequest<'_> {
        let ActivityKind::Message { attachments, .. } = &self.activity.kind else {
            panic!()
        };
        FetchRequest {
            channel: &self.channel,
            event: &self.event,
            activity: &self.activity,
            locator: attachments[0].source.as_ref(),
            declared_size: None,
            credential: crate::media::source::SourceCredential::ChannelKey,
            cap: 1024,
            overflow: Overflow::Reject,
            deadline: Instant::now() + Duration::from_secs(5),
        }
    }
    pub async fn fetch(&self) -> Result<Vec<u8>, FetchError> {
        self.source
            .fetch(self.request())
            .await
            .map(|b| b.bytes.to_vec())
    }
    pub fn rotate_hook(&self) -> Box<dyn FnOnce() + Send> {
        let mut changed = self.definition.clone();
        changed.secrets.native_lark.as_mut().unwrap().app_secret =
            Secret::new("fixture-rotated-app-secret");
        let value = record(&changed, &self.keys, 2);
        let stored = self.stored.clone();
        Box::new(move || *stored.lock().unwrap() = value)
    }
}
