use super::*;
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use std::{collections::BTreeMap, sync::Mutex};

pub(super) struct Remote {
    pub records: Mutex<BTreeMap<String, Value>>,
    pub provider_keys: Mutex<BTreeMap<String, String>>,
    pub profiles: Mutex<Vec<Value>>,
    pub refusal: Mutex<Option<String>>,
}
pub(super) struct Server {
    pub base: String,
    pub remote: Arc<Remote>,
    pub identity: super::nyxid_fixture::Identity,
    task: tokio::task::JoinHandle<()>,
}
impl Server {
    pub async fn start() -> Self {
        let remote = Arc::new(Remote {
            records: Mutex::default(),
            provider_keys: Mutex::default(),
            profiles: Mutex::default(),
            refusal: Mutex::default(),
        });
        let app = Router::new()
            .route(
                "/api/v1/users/me",
                get(|| async { Json(json!({"id":"owner"})) }),
            )
            .route(
                "/api/v1/gateway/channel-records/{id}",
                put(save).get(read).delete(delete),
            )
            .route("/api/v1/cmaeg/bindings/{id}", put(bind).delete(unbind))
            .route(
                "/api/v1/cmaeg/bindings/{id}/conversations/{conversation}",
                put(conversation),
            )
            .route("/api/v2/responses", post(respond))
            .with_state(remote.clone());
        let identity = Arc::default();
        let app = app.merge(super::nyxid_fixture::routes(Arc::clone(&identity)));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let issuer = base.clone();
        let app = app.route("/oauth/introspect",post(move |axum::Form(form): axum::Form<BTreeMap<String,String>>| {
            let issuer=issuer.clone();
            async move {
                let active=form.get("token").is_some_and(|v|v=="owner-token");
                Json(json!({"active":active,"sub":"owner","iss":issuer,"token_type":"access","exp":4102444800_u64,"permissions":["cma-eg:admin"]}))
            }
        }));
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            base,
            remote,
            identity,
            task,
        }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn save(
    State(state): State<Arc<Remote>>,
    Path(id): Path<String>,
    Json(mut body): Json<Value>,
) -> Response {
    let mut records = state.records.lock().unwrap();
    let previous = records.get(&id).map(|r| r["version"].as_u64().unwrap());
    if body["expected_version"].as_u64() != previous {
        return StatusCode::CONFLICT.into_response();
    }
    assert!(
        STANDARD
            .decode(body["blob"].as_str().unwrap())
            .unwrap()
            .len()
            < 65_536
    );
    let version = previous.unwrap_or(0) + 1;
    body["version"] = version.into();
    body["record_id"] = id.clone().into();
    body["created_at"] = "now".into();
    body["updated_at"] = "now".into();
    records.insert(id.clone(), body);
    Json(json!({"record_id":id,"version":version,"updated_at":"now"})).into_response()
}
async fn read(State(state): State<Arc<Remote>>, Path(id): Path<String>) -> Response {
    state
        .records
        .lock()
        .unwrap()
        .get(&id)
        .cloned()
        .map(|r| Json(r).into_response())
        .unwrap_or_else(|| StatusCode::NOT_FOUND.into_response())
}
async fn delete(
    State(state): State<Arc<Remote>>,
    Path(id): Path<String>,
    Query(query): Query<BTreeMap<String, u64>>,
) -> Response {
    let mut records = state.records.lock().unwrap();
    let Some(record) = records.get(&id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if query.get("expected_version").copied() != record["version"].as_u64() {
        return StatusCode::CONFLICT.into_response();
    }
    records.remove(&id);
    StatusCode::NO_CONTENT.into_response()
}
async fn bind(
    State(state): State<Arc<Remote>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    assert_eq!(headers["authorization"], "Bearer fixture-service");
    assert_eq!(body["schema_version"], 1);
    assert_eq!(body["tools"]["nyxid_service_slug"], "cmaeg");
    let manager = layer2_core::Caller::new(
        SubjectId::new("gateway-manager"),
        layer2_core::Authorization::new([
            layer2_core::Capability::ManageGatewayRecords,
            layer2_core::Capability::ManageProviderBindings,
        ]),
    );
    // Exercise the actual negotiation for each request. Runtime allocation and
    // credential custody remain fake; provider_keys below retains their effects.
    let plan = super::provider_fixture::service()
        .put(
            &manager,
            &id,
            body["profile"].clone(),
            body["profile_digest"].as_str().unwrap(),
            &ProvidedCredential::new(body["agent_key"].as_str().unwrap().into()).unwrap(),
        )
        .await
        .unwrap();
    state
        .provider_keys
        .lock()
        .unwrap()
        .insert(id.clone(), body["agent_key"].as_str().unwrap().to_owned());
    state.profiles.lock().unwrap().push(body["profile"].clone());
    let mut refused = serde_json::to_value(plan.refused).unwrap();
    if let Some(field) = state.refusal.lock().unwrap().clone() {
        refused
            .as_array_mut()
            .unwrap()
            .push(json!({"field":field,"reason":"unsupported field"}));
    }
    Json(json!({"binding_id":id,"level":"l2","honoured":plan.honoured,"refused":refused}))
        .into_response()
}
async fn unbind(State(state): State<Arc<Remote>>, Path(id): Path<String>) -> StatusCode {
    state.provider_keys.lock().unwrap().remove(&id);
    StatusCode::NO_CONTENT
}

async fn conversation(headers: HeaderMap) -> StatusCode {
    assert_eq!(headers["authorization"], "Bearer fixture-service");
    StatusCode::OK
}
async fn respond(
    State(state): State<Arc<Remote>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    assert!(body.get("event_context").is_none_or(Value::is_null));
    assert!(!headers["idempotency-key"].is_empty());
    let key = headers["authorization"]
        .to_str()
        .unwrap()
        .strip_prefix("Bearer ")
        .unwrap();
    assert!(state
        .provider_keys
        .lock()
        .unwrap()
        .values()
        .any(|k| k == key));
    let events = [
        json!({"type":"response.created","response":{"id":"response-fixture"}}),
        json!({"type":"response.output_text.delta","item_id":"answer","delta":"Ready"}),
        json!({"type":"response.completed","response":{"id":"response-fixture","output":[]}}),
    ];
    let stream = events
        .into_iter()
        .map(|e| format!("event: {}\ndata: {e}\n\n", e["type"].as_str().unwrap()))
        .collect::<String>();
    (
        [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
        stream,
    )
        .into_response()
}

#[path = "../../../../cmaeg/tests/support/database.rs"]
mod database;

pub(super) struct Gateway {
    database: database::DatabaseFixture,
    pub base: String,
    pub child: tokio::process::Child,
    master: String,
}
impl Gateway {
    pub async fn start(server: &Server) -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        let master = format!(
            "1:{}",
            STANDARD.encode(
                [
                    uuid::Uuid::new_v4().as_bytes().as_slice(),
                    uuid::Uuid::new_v4().as_bytes().as_slice()
                ]
                .concat()
            )
        );
        let binary = Self::binary();
        let database = database::DatabaseFixture::new(&binary).await;
        let mut command = Self::command(server, &base, &master);
        database.configure(&mut command);
        let mut gateway = Self {
            database,
            base: base.clone(),
            child: command
                .spawn()
                .expect("build cmaeg before running Bot interoperability tests"),
            master,
        };
        gateway.ready().await;
        gateway
    }
    fn binary() -> std::path::PathBuf {
        std::env::var_os("CMAEG_TEST_BINARY")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                let exe = std::env::current_exe().unwrap();
                exe.parent().unwrap().parent().unwrap().join("cmaeg")
            })
    }
    fn command(server: &Server, base: &str, master: &str) -> tokio::process::Command {
        let mut cmd = tokio::process::Command::new(Self::binary());
        for (key, _) in std::env::vars().filter(|(k, _)| k.starts_with("CMAEG_")) {
            cmd.env_remove(key);
        }
        cmd.kill_on_drop(true).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())
            .env("CMAEG_LISTEN", base.strip_prefix("http://").unwrap()).env("CMAEG_METRICS_LISTEN", "127.0.0.1:0")
            .env("CMAEG_PUBLIC_URL", base).env("CMAEG_CMA_BASE_URL", &server.base).env("CMAEG_NYXID_BASE_URL", &server.base)
            .env("CMAEG_CMA_SERVICE_TOKEN", "fixture-service").env("CMAEG_MASTER_KEYS", master)
            .env("CMAEG_PROVIDERS", json!([{"slug":"cma_codex","base_url":server.base,"kind":"responses_http",
                "paths":{"agent_card":"/.well-known/agent-card.json","binding":"/api/v1/cmaeg/bindings/{binding_id}",
                    "conversation":"/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}","responses":"/api/v2/responses"},"max_inflight":8}]).to_string());
        cmd
    }
    async fn ready(&mut self) {
        for _ in 0..100 {
            if reqwest::get(format!("{}/readyz", self.base))
                .await
                .is_ok_and(|r| r.status().is_success())
            {
                return;
            }
            assert!(
                self.child.try_wait().unwrap().is_none(),
                "gateway exited before readiness"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("gateway did not become ready");
    }
    pub async fn restart(&mut self, server: &Server) {
        self.child.kill().await.unwrap();
        self.child.wait().await.unwrap();
        let mut command = Self::command(server, &self.base, &self.master);
        self.database.configure(&mut command);
        self.child = command.spawn().unwrap();
        self.ready().await;
    }
}

pub(super) async fn bot() -> Bot {
    struct Profiles;
    #[async_trait::async_trait]
    impl BotProfileResolver for Profiles {
        async fn resolve(
            &self,
            _: &SubjectId,
            reference: &layer2_core::profiles::AgentProfileRef,
        ) -> Result<BotProfiles, BotError> {
            use layer2_core::profiles::*;
            let agent = json!({"model":"fixture-model","thinking_effort":"x_high","permission_level":"guarded","skills":[],"system_prompt":"Reply to the event"});
            let sandbox = json!({"cpu_millis":1500,"memory_mib":2048,"nyxid_services":["ornn-api"],"agent_provider":"codex","model_access":"chronoai_managed",
                "max_workspaces":1,"max_agents_per_workspace":1,"workspace_profiles":[],"software_install_commands":["true"],"environment_variables":[]});
            let workspace = json!({"name":"bot-workspace","location":{"type":"directory","name":"bot-workspace"},"setup_commands":["git status"],"agent_profiles":[]});
            Ok(serde_json::from_value(json!({"resolved_model":"fixture-model",
                "agent":{"profile_id":reference.id,"revision":reference.revision,"digest":content_digest(&agent).unwrap(),"content":agent,"published_at":layer2_core::now_rfc3339()},
                "sandbox":{"profile_id":"sbp_fixture","revision":1,"digest":content_digest(&sandbox).unwrap(),"content":sandbox,"published_at":layer2_core::now_rfc3339()},
                "workspace":{"profile_id":"wkp_fixture","revision":1,"digest":content_digest(&workspace).unwrap(),"content":workspace,"published_at":layer2_core::now_rfc3339()}})).unwrap())
        }
    }
    let service = BotService::new(
        Arc::new(InMemoryBotStore::default()),
        Arc::new(Profiles),
        "cma_codex".into(),
    );
    let caller = layer2_core::Caller::new(
        SubjectId::new("owner"),
        layer2_core::Authorization::new([layer2_core::Capability::ManageOwnBots]),
    );
    service.create(&caller, "create", serde_json::from_value(json!({"name":"Gateway interoperability", "external_bot_id":"external-bot", "profile":{"id":"agp_fixture","revision":1},
        "admission":{"type":"scoped","senders":{"type":"open"},"chats":{"type":"open"},"groups":"deny"},"reply":{"auto_final":true,"ack_after_ms":0}})).unwrap()).await.unwrap()
}
