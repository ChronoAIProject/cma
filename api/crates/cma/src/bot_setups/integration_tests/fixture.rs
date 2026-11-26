use super::*;
use crate::auth::{
    actor_links::*, persistence::*, profile::Profile, test_resources as resources, AuthMode,
};
use crate::sandbox::CredentialMaterial;
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use layer2_core::sandbox::SandboxCredentialAuthority;
use layer2_core::{
    profiles::{content_digest, AgentProvider, ProfileCapacityBounds},
    resource_access::ResourceAccessService,
    sandbox::{agent_io::*, binding::*, store::*, AgentStatus, InMemorySandboxStore},
    SubjectId,
};
use std::{
    collections::HashMap,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    time::SystemTime,
};

pub(super) const KEY: &str = "binding-fixture-private-credential";
pub(super) const USER_TOKEN: &str = "visitor-fixture-private-credential";
pub(super) const EVENT: &str = "cmaeg1.ev.fixture";
pub(super) const BINDING: &str = "binding-continuation";
pub(super) const TARGET: &str = "cmaeg1.rt.fixture";

pub(super) fn epoch() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

#[derive(Default)]
pub(super) struct Peer {
    pub permissions: Vec<String>,
    pub activity: Mutex<Value>,
    pub mint_calls: AtomicUsize,
    pub gateway_calls: AtomicUsize,
    pub reply_calls: AtomicUsize,
    pub effects: Mutex<HashMap<String, Value>>,
    pub lose_reply_once: AtomicBool,
    pub polled: Mutex<Vec<AdmittedAgentCommand>>,
}
async fn context(State(peer): State<Arc<Peer>>, headers: HeaderMap) -> Json<Value> {
    assert_eq!(headers["authorization"], format!("Bearer {KEY}"));
    peer.gateway_calls.fetch_add(1, Ordering::SeqCst);
    Json(
        json!({"channel_id":"channel-a","provider_binding_id":BINDING,"conversation_id":"chat-a","event_id":"event-a",
        "source":{"type":"nyxid_relay","platform":"telegram"},"activity_verified":true,"expires_at":epoch()+600,"activity":*peer.activity.lock().unwrap()}),
    )
}
async fn mint(State(peer): State<Arc<Peer>>, headers: HeaderMap) -> Json<Value> {
    assert_eq!(headers["authorization"], format!("Bearer {KEY}"));
    peer.mint_calls.fetch_add(1, Ordering::SeqCst);
    Json(json!({"reply_target_ref":TARGET,"expires_at":epoch()+86400}))
}
async fn reply(
    State(peer): State<Arc<Peer>>,
    Path(target): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> axum::response::Response {
    assert_eq!(headers["authorization"], format!("Bearer {KEY}"));
    assert_eq!(
        target, TARGET,
        "reply must use the attested original destination"
    );
    peer.reply_calls.fetch_add(1, Ordering::SeqCst);
    let key = headers["idempotency-key"].to_str().unwrap().to_owned();
    let mut effects = peer.effects.lock().unwrap();
    if let Some(previous) = effects.get(&key) {
        assert_eq!(previous, &body);
    } else {
        effects.insert(key, body);
    }
    drop(effects);
    if peer.lose_reply_once.swap(false, Ordering::SeqCst) {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    Json(json!({"message_ref":"fixture-message","delivery":{"outcome":"confirmed"},"rendering":{}}))
        .into_response()
}

pub(super) struct Fixture {
    pub authority: Arc<SetupAuthority>,
    pub store: Arc<InMemorySandboxStore>,
    pub auth_store: Arc<MemoryAuthStore>,
    pub commands: Arc<AgentCommandService>,
    pub binding: ProviderBinding,
    pub conversation: BindingConversation,
    pub source: ExternalEventContext,
    pub actor: ExternalActor,
    pub peer: Arc<Peer>,
    pub audit: Arc<ports::Audit>,
    server: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}
impl Fixture {
    pub async fn new() -> Self {
        Self::with_permissions(&["cma:*"]).await
    }

    pub async fn with_permissions(permissions: &[&str]) -> Self {
        let store = Arc::new(InMemorySandboxStore::default());
        let profile = json!({"schema_version":1,"metadata":{"name":"Father","owner":{"kind":"human","subject":"father-owner"},"provider":"registered"},
            "model":{"access":"managed","service":AgentProvider::default().connection_services().managed,"api_type":"responses","model":"model-a","thinking_effort":"medium"},
            "permission":{"action_mode":"guarded"},"context":{},"environment":{"size":{"cpu_cores":1,"memory_gib":2},"workspace":{"type":"directory","name":"project"}}});
        let mapped = map_profile(
            profile.clone(),
            &format!("sha256:{}", content_digest(&profile).unwrap()),
            AgentProvider::default(),
            "responses",
            ProfileCapacityBounds {
                min_cpu_millis: 250,
                max_cpu_millis: 8000,
                min_memory_mib: 512,
                max_memory_mib: 16384,
            },
        )
        .unwrap();
        let mut binding = ProviderBinding::new(
            BINDING.into(),
            SubjectId::new("manager"),
            mapped,
            ProvidedCredential::new(KEY.into()).unwrap().sha256(),
            layer2_core::now_rfc3339(),
        )
        .unwrap();
        store.save_binding(None, binding.clone()).await.unwrap();
        binding.state = BindingState::Ready;
        binding.work = BindingWork::Idle;
        binding
            .advance_revision(layer2_core::now_rfc3339())
            .unwrap();
        store.save_binding(Some(1), binding.clone()).await.unwrap();
        let mut conversation = BindingConversation::new(
            &binding,
            "provider-sender-a".into(),
            layer2_core::now_rfc3339(),
        )
        .unwrap();
        store
            .save_binding_conversation(binding.revision, None, conversation.clone())
            .await
            .unwrap();
        conversation.state = BindingConversationState::Ready;
        conversation.revision += 1;
        store
            .save_binding_conversation(binding.revision, Some(1), conversation.clone())
            .await
            .unwrap();
        let sandbox = resources::sandbox(binding.sandbox_id.as_str(), "father-owner");
        let workspace = resources::workspace(
            binding.workspace_id.as_str(),
            binding.sandbox_id.as_str(),
            "father-owner",
        );
        let worktree = resources::worktree(
            conversation.worktree_id.as_str(),
            binding.workspace_id.as_str(),
            binding.sandbox_id.as_str(),
            conversation.agent_id.as_str(),
            "father-owner",
        );
        let mut agent = resources::agent(
            conversation.agent_id.as_str(),
            conversation.worktree_id.as_str(),
            binding.workspace_id.as_str(),
            binding.sandbox_id.as_str(),
            "father-owner",
            AgentStatus::Ready,
        );
        agent.conversation.conversation_id = Some("native-fixture-conversation".into());
        agent.conversation.runtime_generation = Some(1);
        store.create_sandbox(sandbox).await.unwrap();
        store.create_workspace(workspace).await.unwrap();
        store.create_worktree(worktree).await.unwrap();
        store.create_agent(agent).await.unwrap();
        let activity = json!({"actor":{"kind":"human","id":"sender-a"},"conversation":{"kind":"private","id":"chat-a"},
            "source":{"type":"nyxid_relay","platform":"telegram"},"event_id":"event-a","occurred_at":layer2_core::now_rfc3339(),"kind":{"type":"message","text":"Please help create a bot"}});
        let peer = Arc::new(Peer {
            permissions: permissions
                .iter()
                .map(|value| (*value).to_owned())
                .collect(),
            activity: Mutex::new(activity.clone()),
            ..Default::default()
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let metadata = json!({"authorization_endpoint":format!("{base}/authorize"),"token_endpoint":format!("{base}/token"),"userinfo_endpoint":format!("{base}/userinfo"),"introspection_endpoint":format!("{base}/introspect")});
        let app = Router::new()
            .route(
                "/.well-known/openid-configuration",
                get(move || {
                    let value = metadata.clone();
                    async move { Json(value) }
                }),
            )
            .route(
                "/userinfo",
                get(|State(peer): State<Arc<Peer>>| async move {
                    Json(json!({"sub":"visitor","name":"Visitor","permissions":peer.permissions.clone()}))
                }),
            )
            .route(
                "/introspect",
                post(|State(peer): State<Arc<Peer>>| async move {
                    Json(json!({"active":true,"sub":"visitor","permissions":peer.permissions.clone()}))
                }),
            )
            .route("/api/v1/orgs", get(|headers:HeaderMap| async move {
                assert_eq!(headers["authorization"], format!("Bearer {USER_TOKEN}"));
                Json(json!({"orgs":[{"id":"visitor-org","your_role":"admin","display_name":"Visitor organization"}]}))
            }))
            .route("/api/v1/orgs/visitor-org/authorization", get(|headers:HeaderMap| async move {
                assert_eq!(headers["authorization"], format!("Bearer {USER_TOKEN}"));
                Json(json!({"id":"visitor-org","your_role":"admin"}))
            }))
            .route("/v1/events/{event}/context", get(context))
            .route("/v1/events/{event}/reply-target", post(mint))
            .route("/v1/reply-targets/{target}/messages", post(reply))
            .with_state(peer.clone());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let auth_store = Arc::new(MemoryAuthStore::default());
        let auth = AuthState::with_store(
            AuthMode::Provider {
                base_url: base.clone(),
                client_id: "fixture".into(),
                client_secret: "fixture-client".into(),
                public_url: "https://cma.example.test".into(),
            },
            None,
            auth_store.clone(),
        )
        .with_binding_store(store.clone());
        let bindings = BindingAccessService::new(store.clone());
        let access = Arc::new(
            ResourceAccessService::new(store.clone(), auth.clone()).with_bindings(bindings),
        );
        let commands = AgentCommandService::new(store.clone(), access, auth.clone());
        let source = ExternalEventContext {
            provider_conversation_id: Some(conversation.conversation_id.clone()),
            source_id: "channel-a".into(),
            conversation_id: "chat-a".into(),
            event_id: "event-a".into(),
            reference: EVENT.into(),
            payload: json!({"activity":activity}),
        };
        let mut command = AgentCommand::message("Please help create a bot");
        let AgentCommand::Message { input, .. } = &mut command else {
            unreachable!()
        };
        input.content.push(AgentContent::ExternalEvent {
            context: source.clone(),
        });
        let request = auth.binding_request(KEY.into()).await.unwrap().unwrap();
        let context = auth.resource_request_context(&request);
        commands
            .admit(
                &context,
                KEY,
                CommandSubmissionSource::Api,
                CommandSubmission {
                    agent_id: conversation.agent_id.clone(),
                    command,
                    idempotency_key: "source-event-a".into(),
                    expected_agent_revision: None,
                    requested_model: None,
                },
            )
            .await
            .unwrap();
        let material = CredentialMaterial {
            github_route: None,
            sandbox_id: binding.sandbox_id.clone(),
            owner: binding.owner.clone(),
            source: binding.credential_source(),
            value: SecretValue::new(KEY.into()),
            authority: SandboxCredentialAuthority {
                generation: 1,
                provider_key_id: format!("provided-{}", binding.key_sha256),
                provider_state_version: 1,
                issued_at: layer2_core::now_rfc3339(),
                expires_at: layer2_core::rfc3339_at(SystemTime::now() + Duration::from_secs(3600)),
                grant_fingerprint: "a".repeat(64),
            },
        };
        let audit = Arc::new(ports::Audit::default());
        let authority = Arc::new(SetupAuthority {
            auth,
            store: store.clone(),
            custody: Arc::new(ports::Custody(material)),
            activity: audit.clone(),
            contexts: Arc::new(ports::Contexts::default()),
            http: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
            gateway: base,
            binding: BINDING.into(),
        });
        let actor = ExternalActor {
            binding_id: BINDING.into(),
            channel_id: "channel-a".into(),
            conversation_id: "chat-a".into(),
            actor_id: "sender-a".into(),
            platform: "telegram".into(),
        };
        Self {
            authority,
            store,
            auth_store,
            commands,
            binding,
            conversation,
            source,
            actor,
            peer,
            audit,
            server,
        }
    }

    pub async fn activate(&self) -> ActorWakeupNotification {
        self.authority
            .verified_event(BINDING, KEY, EVENT, None)
            .await
            .unwrap();
        let identity = self
            .authority
            .auth
            .ensure_actor_link(&authorization::actor(&self.actor))
            .await
            .unwrap();
        let state = SubjectAuthState::new(
            SubjectId::new("visitor"),
            crate::permissions::capabilities_from_permissions(&self.peer.permissions),
            Profile::default(),
            Some(DurableSubjectTokens {
                access_token: SecretValue::new(USER_TOKEN.into()),
                refresh_token: Some(SecretValue::new(
                    "visitor-fixture-refresh-credential".into(),
                )),
                expires_at: SystemTime::now() + Duration::from_secs(3600),
            }),
        );
        self.auth_store.put_subject(state.clone()).await.unwrap();
        // Browser consent/PKCE has its own real HTTP tests. Start this integration
        // at its committed row, then exercise real grant checks and command IO.
        let mut row = self
            .auth_store
            .actor_link_by_id(&identity.link_id)
            .await
            .unwrap()
            .unwrap();
        let revision = row.revision;
        row.subject = Some(state.subject);
        row.incarnation = Some(state.incarnation);
        row.active = true;
        row.browser_consent = Some(ActorBrowserConsent {
            attempt_digest: "b".repeat(64),
            browser_digest: "c".repeat(64),
            csrf_token: "d".repeat(32),
            review_revision: revision,
            approved_at: Some(epoch()),
        });
        row.wakeup = Some(ActorWakeup {
            notification_id: "e".repeat(64),
            state: ActorWakeupState::Pending { retry_at: 0 },
        });
        row.revision += 1;
        self.auth_store
            .replace_actor_link(Some(revision), row)
            .await
            .unwrap();
        self.authority
            .auth
            .claim_actor_wakeup(&identity.link_id, &"e".repeat(64), "fixture-worker", 120)
            .await
            .unwrap()
            .unwrap()
    }
    pub fn worker(&self) -> Arc<ActorContinuationWorker> {
        let store = self.store.clone();
        let agent = self.conversation.agent_id.clone();
        let peer = self.peer.clone();
        let resources = Router::new().route("/api/v2/responses/{id}", get(move |Path(id):Path<String>| {let store=store.clone(); let agent=agent.clone(); let peer=peer.clone(); async move {
            let command = store.get_response_command(&layer2_core::ids::AgentResponseId(id)).await.unwrap().unwrap();
            assert_eq!(command.receipt.envelope.agent_id, agent);
            peer.polled.lock().unwrap().push(command);
            Json(json!({"status":"completed","output":[{"type":"message","content":[{"type":"output_text","text":"Choose personal or organization."}]}]}))
        }}));
        ActorContinuationWorker::with_authority(
            self.authority.clone(),
            self.commands.clone(),
            resources,
        )
    }
}
