mod gateway_reset_tests {
    use super::*;
    use axum::{
        extract::Request, http::StatusCode, response::IntoResponse, routing::post, Json, Router,
    };
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    use cmaeg_core::{
        channel::ChannelDefinition,
        ids::{compact_partition_key, ChannelId, ConsumerId, ConversationId, RecordId},
        profile::{canonical_json, EventWorkerProfile},
        seal::{seal_blob, MasterKeyRing},
    };
    use layer2_core::{
        profiles::{store::InMemoryProfileStore, ProfileCapacityBounds},
        sandbox::*,
        Authorization, Capability, CoreError, RuntimeCapabilities, RuntimeCapability, SandboxId,
        ServiceId, SubjectId,
    };
    use serde_json::{json, Value};
    use std::{
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        },
        time::Duration,
    };
    use tokio::{
        process::{Child, Command},
        sync::RwLock,
    };
    use tower::ServiceExt;

    const CONSUMER: &str = "csm_00000000000a";

    struct NoCustody;
    #[async_trait::async_trait]
    impl BindingCredentialCustody for NoCustody {
        async fn retain(
            &self,
            _: &SandboxId,
            _: &SubjectId,
            _: &[ServiceId],
            _: &ProvidedCredential,
        ) -> Result<(), CoreError> {
            panic!("reset must not create or replace credentials")
        }
    }
    fn routes(store: MongoSandboxStore) -> [Router; 2] {
        let store = Arc::new(store);
        let profiles = Arc::new(InMemoryProfileStore::default());
        let service = BindingService::new(
            store.clone(),
            BindingResources {
                sandboxes: SandboxService::new(
                    store.clone(),
                    profiles.clone(),
                    SandboxPolicy::default(),
                    Arc::new(FixedSandboxCredentialPrerequisite::SetupRequired),
                ),
                workspaces: WorkspaceService::new(store.clone(), profiles.clone()),
                agents: AgentService::new(
                    store,
                    profiles,
                    Arc::new(crate::sandbox::LiveAgentRegistry::default()),
                    RuntimeCapabilities::new([RuntimeCapability::Conversation]),
                ),
            },
            Arc::new(NoCustody),
            BindingPolicy {
                provider: Default::default(),
                supported_model_api: "supported".into(),
                bounds: ProfileCapacityBounds {
                    min_cpu_millis: 250,
                    max_cpu_millis: 8000,
                    min_memory_mib: 512,
                    max_memory_mib: 16384,
                },
                sandbox: Arc::new(SandboxPolicy::default()),
            },
        );
        [false, true].map(|enabled| {
            layer1_public::rest::binding_router(service.clone()).merge(
                layer1_public::rest::binding_conversation_reset_router(service.clone(), enabled),
            )
        })
    }
    struct Provider {
        base: String,
        enabled: Arc<AtomicBool>,
        lose_once: Arc<AtomicBool>,
        routes: Arc<RwLock<[Router; 2]>>,
        task: tokio::task::JoinHandle<()>,
    }
    impl Drop for Provider {
        fn drop(&mut self) {
            self.task.abort();
        }
    }
    impl Provider {
        async fn start(store: MongoSandboxStore, binding: &ProviderBinding) -> Self {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            let routes = Arc::new(RwLock::new(routes(store)));
            let enabled = Arc::new(AtomicBool::new(false));
            let lose_once = Arc::new(AtomicBool::new(false));
            let (apps, gate, loss) = (routes.clone(), enabled.clone(), lose_once.clone());
            let manager = binding.manager.clone();
            let owner = binding.owner.to_string();
            let issuer = base.clone();
            let app = Router::new().route("/oauth/introspect", post(move |axum::Form(form): axum::Form<std::collections::BTreeMap<String, String>>| {
                let (owner, issuer) = (owner.clone(), issuer.clone());
                async move { Json(json!({"active":form.get("token").is_some_and(|token| token == "owner-fixture"),
                    "sub":owner,"iss":issuer,"exp":4102444800_u64,"token_type":"access","permissions":["cma-eg:channels:read","cma-eg:channels:update"]})) }
            })).fallback(move |mut request: Request| {
                let (apps, gate, loss, manager) = (apps.clone(), gate.clone(), loss.clone(), manager.clone());
                async move {
                    if request.headers().get("authorization").is_none_or(|v| v != "Bearer manager-fixture") {
                        return StatusCode::UNAUTHORIZED.into_response();
                    }
                    let reset = request.uri().path().ends_with("/resets") || request.uri().path().ends_with("/profile-transfers");
                    request.extensions_mut().insert(layer1_public::AuthenticatedSubject(manager));
                    request.extensions_mut().insert(Authorization::new([Capability::ManageGatewayRecords, Capability::ManageProviderBindings]));
                    let app = apps.read().await[usize::from(gate.load(Ordering::SeqCst))].clone();
                    let response = app.oneshot(request).await.unwrap();
                    if reset && response.status() == StatusCode::OK && loss.swap(false, Ordering::SeqCst) {
                        return StatusCode::SERVICE_UNAVAILABLE.into_response();
                    }
                    response
                }
            });
            let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            Self {
                base,
                routes,
                enabled,
                lose_once,
                task,
            }
        }
    }

    struct Gateway {
        database: mongodb::Database,
        child: Child,
        command: Command,
        base: String,
        master: String,
        cookie: std::path::PathBuf,
    }
    impl Drop for Gateway {
        fn drop(&mut self) {
            std::fs::remove_file(&self.cookie).expect("remove fixture cookie key");
        }
    }
    impl Gateway {
        async fn start(provider: &Provider, client: &mongodb::Client) -> Self {
            let database =
                client.database(&format!("gateway_reset_{}", uuid::Uuid::new_v4().simple()));
            cmaeg::records::mongo::migrate(&database).await.unwrap();
            database
                .run_command(cmaeg::records::mongo::role(database.name(), false))
                .await
                .unwrap();
            let password = uuid::Uuid::new_v4().to_string();
            database
                .run_command(doc! {"createUser":"runtime","pwd":&password,
                "roles":[{"role":"cma_eg_runtime","db":database.name()}]})
                .await
                .unwrap();
            let uri = std::env::var("CMA_IM_TEST_MONGODB_URI")
                .expect("run image-manager-mongo-contract.sh am for authenticated Mongo");
            let mut uri = reqwest::Url::parse(&uri).expect("valid authenticated Mongo fixture URI");
            uri.set_username("runtime").unwrap();
            uri.set_password(Some(&password)).unwrap();
            uri.set_path(&format!("/{}", database.name()));
            uri.set_query(Some("directConnection=true"));
            let cookie =
                std::env::temp_dir().join(format!("gateway-cookie-{}", uuid::Uuid::new_v4()));
            std::fs::write(&cookie, [7_u8; 32]).unwrap();
            let master = format!("1:{}", STANDARD.encode([1_u8; 32]));
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            drop(listener);
            let base = format!("http://{address}");
            let binary = std::env::var_os("CMAEG_TEST_BINARY").unwrap_or_else(|| {
                std::env::current_exe()
                    .expect("CMA test executable path")
                    .with_file_name("../cmaeg")
                    .into_os_string()
            });
            let mut cmd = Command::new(binary);
            for (name, _) in std::env::vars().filter(|(name, _)| name.starts_with("CMAEG_")) {
                cmd.env_remove(name);
            }
            cmd.env("CMAEG_MONGODB_URI", uri.as_str())
                .env("CMAEG_MONGODB_DATABASE", database.name())
                .env("CMAEG_NYXID_CLIENT_ID", "gateway-management")
                .env("CMAEG_NYXID_CLIENT_SECRET", "gateway-management-fixture")
                .env("CMAEG_WEB_ORIGIN", "https://cma.example")
                .env("CMAEG_COOKIE_KEY_FILE", &cookie);
            cmd.kill_on_drop(true).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())
                .env("CMAEG_LISTEN", address.to_string()).env("CMAEG_METRICS_LISTEN", "127.0.0.1:0")
                .env("CMAEG_PUBLIC_URL", &base).env("CMAEG_CMA_BASE_URL", &provider.base)
                .env("CMAEG_NYXID_BASE_URL", &provider.base).env("CMAEG_CMA_SERVICE_TOKEN", "manager-fixture")
                .env("CMAEG_MASTER_KEYS", &master).env("CMAEG_PROVIDERS", json!([{"slug":"cma_codex","base_url":provider.base,"kind":"responses_http","max_inflight":8,
                    "paths":{"agent_card":"/.well-known/agent-card.json","binding":"/api/v1/cmaeg/bindings/{binding_id}",
                    "conversation":"/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}","responses":"/api/v2/responses"}}]).to_string());
            let child = Self::spawn(&mut cmd, &base).await;
            Self {
                database,
                child,
                command: cmd,
                base,
                master,
                cookie,
            }
        }
        async fn spawn(command: &mut Command, base: &str) -> Child {
            let mut child = command.spawn().unwrap();
            tokio::time::timeout(Duration::from_secs(10), async {
                loop {
                    if reqwest::get(format!("{base}/readyz"))
                        .await
                        .is_ok_and(|r| r.status().is_success())
                    {
                        break;
                    }
                    let status = child.try_wait().unwrap();
                    assert!(status.is_none(), "gateway exited before readiness");
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
            })
            .await
            .unwrap();
            child
        }
        async fn restart(&mut self) {
            self.child.kill().await.unwrap();
            self.child.wait().await.unwrap();
            self.child = Self::spawn(&mut self.command, &self.base).await;
        }
        async fn reset(&self, channel: &ChannelId, key: &str, body: &Value, status: u16) -> Value {
            let url = format!("{}/v1/channels/{channel}/partition/resets", self.base);
            let response = reqwest::Client::new()
                .post(url)
                .bearer_auth("owner-fixture")
                .header("Idempotency-Key", key)
                .json(body)
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), status);
            assert_eq!(response.headers()["cache-control"], "no-store");
            response.json().await.unwrap()
        }
        async fn inspect(&self, channel: &ChannelId, partition: &str, consumer: &str) -> Value {
            let response = reqwest::Client::new()
                .get(format!("{}/v1/channels/{channel}/partition", self.base))
                .bearer_auth("owner-fixture")
                .query(&[("partition_key", partition), ("consumer_id", consumer)])
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 200);
            response.json().await.unwrap()
        }
    }

    async fn seed(
        gateway: &Gateway,
        provider: &Provider,
        binding: &ProviderBinding,
        scope: &ProviderBinding,
    ) -> (ChannelId, Document) {
        let profile: EventWorkerProfile = serde_json::from_value(json!({"schema_version":1,
            "metadata":{"name":"repair","description":"","owner":{"kind":"human","subject":binding.owner},"provider":"cma_codex"},
            "model":{"access":"managed","service":"cma","api_type":"responses","model":"test","thinking_effort":"low"},
            "permission":{"action_mode":"guarded","service_scope":[]},"context":{"skills":[],"system_prompt":"","mcp_servers":[]},"environment":null})).unwrap();
        let id = RecordId::new(uuid::Uuid::new_v4().to_string()).unwrap();
        let definition: ChannelDefinition = serde_json::from_value(json!({"schema_version":1,"record_id":id,"owner":profile.metadata.owner,
            "profile":profile,"profile_digest":profile.digest(),"provider":{"slug":"cma_codex","binding_id":scope.binding_id,"level":"l1"},
            "sources":[],"partition":{"version":1,"key":"conversation_and_sender"},"reply":{"auto_final":false,"ack_after_ms":0,"sinks":[]},
            "routing":{"version":1,"default_consumer":{"name":"lead"},"default_targets":["default"],
                "consumers":[{"id":CONSUMER,"name":"repair","profile":profile,"profile_digest":profile.digest(),
                    "provider":{"slug":"cma_codex","binding_id":binding.binding_id,"level":"l1"},"agent_key_hash":format!("sha256:{}","b".repeat(64)),"created_at":"2026-01-01T00:00:00Z"}]},
            "secrets":{"agent_key_hash":format!("sha256:{}","a".repeat(64)),"provider_credential":"worker-default-fixture","consumer_keys":[[CONSUMER,"worker-named-fixture"]]},
            "created_at":"2026-01-01T00:00:00Z","version":1})).unwrap();
        definition.validate().unwrap();
        let keys = MasterKeyRing::parse(&gateway.master).unwrap();
        let blob = seal_blob(&keys, canonical_json(definition.to_blob_json()).as_bytes()).unwrap();
        let row = doc! {"_id":id.as_str(),"owner":binding.owner.as_str(),"version":1_i64,"schema_version":1_i32,
        "blob":mongodb::bson::Binary { subtype:mongodb::bson::spec::BinarySubtype::Generic,bytes:blob }};
        gateway
            .database
            .collection::<Document>("eg_channel_records")
            .insert_one(row.clone())
            .await
            .unwrap();
        let channel = cmaeg::channels::definition::channel_pointer(&provider.base, &id)
            .unwrap()
            .seal_id(&keys)
            .unwrap();
        (channel, row)
    }
    async fn binding_row(store: &MongoSandboxStore, id: &str) -> Document {
        let records = &store.bindings.records;
        records.find_one(doc! {"_id":id}).await.unwrap().unwrap()
    }
    async fn mapping(
        store: &MongoSandboxStore,
        binding: &ProviderBinding,
        old: &BindingConversation,
        conversation: String,
    ) -> BindingConversation {
        let mut value = old.clone();
        value.binding_id = binding.binding_id.clone();
        value.conversation_id = conversation;
        value.revision = 1;
        value.state = BindingConversationState::Provisioning;
        store
            .save_binding_conversation(binding.revision, None, value.clone())
            .await
            .unwrap();
        value.state = BindingConversationState::Ready;
        value.revision += 1;
        store
            .save_binding_conversation(binding.revision, Some(1), value.clone())
            .await
            .unwrap();
        value
    }

    #[tokio::test]
    #[ignore = "requires image-manager-mongo-contract.sh am and the compiled gateway"]
    async fn configured_gateway_replays_real_m1_reset_and_preserves_both_scope_floors() {
        let (client, database, store, binding, old) = super::fixture().await;
        let mut scope = binding.clone();
        scope.binding_id = "binding-default".into();
        scope.key_sha256 = "b".repeat(64);
        scope.revision = 1;
        scope.state = BindingState::Provisioning;
        scope.work = BindingWork::AwaitingCredential;
        store.save_binding(None, scope.clone()).await.unwrap();
        scope.state = BindingState::Ready;
        scope.work = BindingWork::Idle;
        scope.advance_revision(layer2_core::now_rfc3339()).unwrap();
        store.save_binding(Some(1), scope.clone()).await.unwrap();
        let provider = Provider::start(store.clone(), &binding).await;
        let mut gateway = Gateway::start(&provider, &client).await;
        let (channel, channel_before) = seed(&gateway, &provider, &binding, &scope).await;
        let keys = MasterKeyRing::parse(&gateway.master).unwrap();
        let conversation = ConversationId::derive_for_consumer(
            &keys,
            &channel,
            1,
            &ConsumerId::parse(CONSUMER).unwrap(),
            &compact_partition_key("repair"),
        );
        // Adapt the initial synthetic mapping to the gateway-derived transport ID.
        assert_eq!(
            store
                .bindings
                .conversations
                .delete_one(doc! {"_id":conversation_storage_key(&old)})
                .await
                .unwrap()
                .deleted_count,
            1
        );
        let old = mapping(&store, &binding, &old, conversation.to_string()).await;
        let control_id = ConversationId::derive(&keys, &channel, 1, "control");
        let control = mapping(&store, &scope, &old, control_id.to_string()).await;
        let before = [
            binding_row(&store, &binding.binding_id).await,
            binding_row(&store, &scope.binding_id).await,
        ];
        let logs = store.agent_log(&old.agent_id, 0, 100).await.unwrap();
        let inspected = gateway.inspect(&channel, "repair", CONSUMER).await;
        assert_eq!(inspected["mapping"]["agent_status"], "deleted");
        let control_before = gateway.inspect(&channel, "control", "default").await;
        let body = json!({"partition_key":"repair","consumer_id":CONSUMER,"expected_version":1,"expected_agent_id":old.agent_id});
        assert_eq!(
            gateway.reset(&channel, "repair", &body, 409).await["detail"],
            "partition_reset_disabled"
        );
        provider.enabled.store(true, Ordering::SeqCst);
        let mut wrong = body.clone();
        wrong["expected_agent_id"] = "agt_wrong".into();
        assert_eq!(
            gateway.reset(&channel, "refused", &wrong, 409).await["detail"],
            "partition_reset_mapping_changed"
        );
        let ids = [&binding.binding_id, &scope.binding_id];
        for (id, original) in ids.into_iter().zip(&before) {
            assert_eq!(&binding_row(&store, id).await, original);
            assert!(store
                .conversation_reset_state(id)
                .await
                .unwrap()
                .receipts
                .is_empty());
        }
        provider.lose_once.store(true, Ordering::SeqCst);
        gateway.reset(&channel, "repair", &body, 503).await;
        let retained = store
            .conversation_reset_state(&scope.binding_id)
            .await
            .unwrap();
        assert_eq!(retained.receipts.len(), 1);
        let receipt = retained.receipts.values().next().unwrap();
        assert_eq!(receipt.request.binding_id, binding.binding_id);
        assert_eq!(receipt.request.idempotency_binding_id, scope.binding_id);
        assert_eq!(receipt.request.expected_agent_id, old.agent_id);
        assert_eq!(receipt.request.owner, binding.resource_owner());
        assert!(store
            .get_agent(&receipt.head.successor.agent_id)
            .await
            .unwrap()
            .is_none());
        for original in [&binding, &scope] {
            let current = store.binding(&original.binding_id).await.unwrap().unwrap();
            assert_eq!(current.reset_format, Some(ConversationResetFormat::V1));
            assert_eq!(current.profile, original.profile);
            assert_eq!(current.generation, original.generation);
            let mut stale = original.clone();
            stale.advance_revision(layer2_core::now_rfc3339()).unwrap();
            assert_eq!(
                store.save_binding(Some(original.revision), stale).await,
                Err(SandboxStoreError::Conflict)
            );
        }
        provider.enabled.store(false, Ordering::SeqCst);
        *provider.routes.write().await =
            routes(MongoSandboxStore::open(&client, &database).await.unwrap());
        gateway.restart().await;
        let (one, two) = tokio::join!(
            gateway.reset(&channel, "repair", &body, 200),
            gateway.reset(&channel, "repair", &body, 200)
        );
        assert_eq!(one, two);
        assert_eq!(
            one["reset"]["successor_agent_id"],
            receipt.head.successor.agent_id.as_str()
        );
        assert_eq!(one["reset"]["state"], "reserved");
        assert_eq!(one["reset"]["incarnation"], 1);
        assert_eq!(
            gateway.reset(&channel, "new", &body, 409).await["detail"],
            "partition_reset_disabled"
        );
        let mut changed = body.clone();
        changed["partition_key"] = "other".into();
        assert_eq!(
            gateway.reset(&channel, "repair", &changed, 409).await["detail"],
            "partition_reset_key_conflict"
        );
        let after = gateway.inspect(&channel, "repair", CONSUMER).await;
        assert_eq!(
            after["mapping"]["agent_id"],
            one["reset"]["successor_agent_id"]
        );
        assert_eq!(after["mapping"]["incarnation"], 1);
        assert_eq!(
            gateway.inspect(&channel, "control", "default").await,
            control_before
        );
        for original in [&control, &old] {
            let stored = store
                .binding_conversation(&original.binding_id, 1, &original.conversation_id)
                .await
                .unwrap();
            assert_eq!(stored.as_ref(), Some(original));
        }
        assert_eq!(store.agent_log(&old.agent_id, 0, 100).await.unwrap(), logs);
        assert_eq!(
            store
                .conversation_reset_state(&scope.binding_id)
                .await
                .unwrap(),
            retained
        );
        let gateway_db = &gateway.database;
        let records = gateway_db.collection::<Document>("eg_channel_records");
        assert_eq!(
            records
                .find_one(doc! {"_id":channel_before.get_str("_id").unwrap()})
                .await
                .unwrap(),
            Some(channel_before)
        );
        assert_eq!(records.count_documents(doc! {}).await.unwrap(), 1);
        gateway.child.kill().await.unwrap();
        gateway.child.wait().await.unwrap();
        gateway_db.drop().await.unwrap();
        client.database(&database).drop().await.unwrap();
    }
    include!("gateway_profile_transfer_tests.rs");
}
