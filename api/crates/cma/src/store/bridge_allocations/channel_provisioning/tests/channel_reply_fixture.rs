use super::*;
use axum::{body::Body, http::Request, Router};
use layer2_core::bridge::invoke::*;
use layer2_core::resource_access::ResourceAccessService;
use layer2_core::sandbox::{agent_io::*, store::*, AgentOperationService};
use layer3_adapter::{AgentSlot, RuntimeCredential};
use mongodb::bson::Document;
use serde_json::{json, Value};
use tower::ServiceExt;

#[path = "channel_reply_inbound.rs"]
mod inbound;

pub(super) struct ReplyFixture {
    pub client: mongodb::Client,
    pub database: String,
    pub resources: Arc<crate::store::MongoSandboxStore>,
    pub invokes: BridgeInvokeService,
    pub grant: BridgeInvokeGrantView,
    pub command: AdmittedAgentCommand,
    pub credential: RuntimeCredential,
    pub router: Router,
    pub backend: layer3_adapter::hosted::InMemoryHostedBackend,
    pub runtime: RuntimeHandle,
    pub registry: Arc<crate::sandbox::LiveAgentRegistry>,
    pub inbox: StoredInbox,
    pub allocation: AllocationRecord,
    pub allocations: Arc<MongoBridgeAllocationStore>,
    pub bridges: Arc<MongoBridgeStore>,
    pub svc: BridgeService,
    pub replies: Arc<crate::store::MongoBridgeReplyToolStore>,
}

impl ReplyFixture {
    pub async fn new() -> Self {
        Self::build(false).await
    }
    pub async fn with_newer_inbound() -> Self {
        Self::build(true).await
    }
    async fn build(newer: bool) -> Self {
        let mut f = Fixture::new().await;
        f.record = f.ready().await;
        if newer {
            inbound::update(&mut f).await;
        }
        let agent_id = f.record.placement.target().agent_id.clone();
        let registry = crate::sandbox::channel_startup::test_support::publish_connection(
            f.resources.clone(),
            f.adapter.clone(),
            &agent_id,
        )
        .await;
        f.authority = renew_retained(&f.svc, &f.bridges, &f.record).await.unwrap();
        let store = Arc::new(
            crate::store::MongoBridgeInvokeStore::open(
                f.bridges.clone(),
                f.resources.clone(),
                &f.database,
            )
            .await
            .unwrap(),
        );
        let allocations = Arc::new(
            MongoBridgeAllocationStore::open(f.bridges.clone(), Some(store.clone()), &f.database)
                .await
                .unwrap(),
        );
        let invokes = BridgeInvokeService::new(
            f.svc.clone(),
            Arc::new(ResourceAccessService::new(
                f.resources.clone(),
                Arc::new(Authority),
            )),
            f.resources.clone(),
            store.clone(),
        );
        let inbound = f.authority.bridge().admission_pin().unwrap();
        let grant = invokes
            .issue_channel(f.lease.clone(), &f.record.id, &inbound, f.authority)
            .await
            .unwrap();
        let callback = crate::store::bridge_definitions::mongo_integration_tests::inbox::callback;
        let (body, token, _) = callback("reply-client-input", "Work on this and keep me updated.");
        let write = crate::store::bridge_definitions::mongo_integration_tests::inbox::write(
            &f.bridges,
            &f.record.pin.bridge_id,
            &body,
            &token,
        )
        .await;
        let inbox = write.record().clone();
        let inbox_store = crate::store::MongoCallbackInbox::open(
            &f.bridges,
            &f.database,
            "fixture-deployment",
            &custody_key(),
        )
        .await
        .unwrap();
        inbox_store.accept(write).await.unwrap();
        let channel = ChannelAgentCommandService::new(Arc::new(
            crate::store::MongoChannelCommandStore::new(store.clone()),
        ));
        let proof = invokes
            .authorize(BridgeInvokeRequest {
                grant_id: grant.id.clone(),
                pin: inbox.pin.clone(),
                target: f.record.placement.target().clone(),
                effect: BridgeInvokeEffect::AdmitMessage {
                    inbox_id: inbox.id.clone(),
                },
            })
            .await
            .unwrap();
        let normalized = NormalizedPartitionSource::from_stored(&inbox, proof.bridge()).unwrap();
        let PartitionAdmissionDecision::Admitted { key, .. } =
            normalized.admit(&proof.grant().config).unwrap()
        else {
            panic!("signed fixture input must be admitted");
        };
        assert_eq!(
            key, f.record.identity.key,
            "signed follow-up must use the created partition"
        );
        ChannelCommandReservation::prepare(proof.clone(), inbox.clone(), f.record.clone())
            .expect("pure Channel source preparation");
        let held = channel
            .admit(proof, inbox.clone(), f.record.clone())
            .await
            .unwrap();
        let authority = channel.authorize_execution(&invokes, &held).await.unwrap();
        channel
            .release_execution_hold(&authority, &held)
            .await
            .unwrap();
        let stamp = std::time::SystemTime::now();
        let claimed = f
            .resources
            .claim_next_command(CommandClaimRequest {
                agent_id: agent_id.clone(),
                holder: "reply-integration-worker".into(),
                now: layer2_core::rfc3339_at(stamp),
                expires_at: layer2_core::rfc3339_at(stamp + Duration::from_secs(30)),
            })
            .await
            .unwrap()
            .unwrap();
        let authority = channel
            .authorize_execution(&invokes, &claimed)
            .await
            .unwrap();
        AgentOperationService::new(f.resources.clone(), registry.clone())
            .execute_channel_claim(&authority, &claimed)
            .await
            .unwrap();
        let command = f
            .resources
            .get_command(&claimed.receipt.envelope.command_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(command.status, AgentResponseStatus::InProgress);
        let credential = f.tools.fixture_credential(&agent_id).await;
        let replies = Arc::new(
            crate::store::MongoBridgeReplyToolStore::new(store, Arc::new(Authority))
                .await
                .unwrap(),
        );
        let router = crate::bridge::channel_tools::router(
            Arc::new(crate::bridge::channel_tools::StoredChannelTools {
                capabilities: f.tools,
                replies: replies.clone(),
            }),
            "https://cma.example",
        );
        let runtime = RuntimeHandle::new(
            &f.resources
                .get_sandbox(&f.record.placement.target().sandbox_id)
                .await
                .unwrap()
                .unwrap()
                .runtime
                .unwrap()
                .runtime_id,
        );
        Self {
            client: f.client,
            database: f.database,
            resources: f.resources,
            invokes,
            grant,
            command,
            credential,
            router,
            backend: f.backend,
            runtime,
            registry,
            inbox,
            allocation: f.record,
            allocations,
            bridges: f.bridges,
            svc: f.svc,
            replies,
        }
    }

    pub async fn raw_call(&self, name: &str, arguments: Value) -> axum::response::Response {
        self.router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/channel-tools")
                    .header("content-type", "application/json")
                    .header(
                        "authorization",
                        format!("Bearer {}", self.credential.expose()),
                    )
                    .body(Body::from(
                        json!({"jsonrpc":"2.0","id":1,"method":"tools/call",
                "params":{"name":name,"arguments":arguments}})
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    pub async fn call(&self, name: &str, arguments: Value) -> Value {
        let response = self.raw_call(name, arguments).await;
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), 32_768)
            .await
            .unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert!(value.get("error").is_none(), "{value}");
        value["result"].clone()
    }
    pub async fn context(&self) -> String {
        let result = self.call("read_channel_context", json!({})).await;
        assert_eq!(result["isError"], false, "{result}");
        result["structuredContent"]["reply_context"]
            .as_str()
            .unwrap()
            .to_owned()
    }
    pub async fn reply(&self, context: &str, key: &str, text: &str) -> Value {
        self.call(
            "reply_to_channel",
            json!({"reply_context":context,"request_key":key,"text":text}),
        )
        .await
    }
    pub async fn items(&self) -> Vec<Document> {
        use futures_util::TryStreamExt;
        self.client
            .database(&self.database)
            .collection::<Document>("bridge_outbox_items")
            .find(doc! {})
            .sort(doc! {"record.sequence":1})
            .await
            .unwrap()
            .try_collect()
            .await
            .unwrap()
    }
    pub async fn emit_assistant_text(&self, message_id: &str, text: &str) {
        use layer3_adapter::{
            ContentBlock, ContentChunk, ConversationUpdate, MessageId, SessionUpdate,
        };
        self.backend
            .send_agent_update(
                &self.runtime,
                &AgentSlot(self.command.receipt.envelope.agent_id.as_str().into()),
                ConversationUpdate::Said(SessionUpdate::AgentMessageChunk(
                    ContentChunk::new(ContentBlock::from(text))
                        .message_id(MessageId::new(message_id)),
                )),
            )
            .await
            .unwrap();
    }
    pub async fn await_assistant_text(&self, text: &str) {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let logs = self
                    .resources
                    .agent_log(&self.command.receipt.envelope.agent_id, 0, 100)
                    .await
                    .unwrap();
                if logs.iter().any(|entry| {
                    matches!(&entry.kind,
                    layer2_core::sandbox::AgentLogKind::AgentMessage { content } if content == text)
                }) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("actual runtime assistant output must reach the durable CMA log");
    }
    pub async fn finish(&self) {
        use layer3_adapter::{PromptResponse, TurnStopReason};
        self.backend
            .send_agent_update(
                &self.runtime,
                &AgentSlot(self.command.receipt.envelope.agent_id.as_str().into()),
                layer3_adapter::ConversationUpdate::TurnEnded(PromptResponse::new(
                    TurnStopReason::EndTurn,
                )),
            )
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if self
                    .resources
                    .get_agent(&self.command.receipt.envelope.agent_id)
                    .await
                    .unwrap()
                    .unwrap()
                    .conversation
                    .active_turn
                    .is_none()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }
    pub async fn close(self) {
        drop(self.registry);
        self.client.database(&self.database).drop().await.unwrap();
    }
}
