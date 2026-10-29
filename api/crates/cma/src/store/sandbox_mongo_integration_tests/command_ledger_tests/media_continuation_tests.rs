//! Real Mongo, public binary route and file publication with an attestation peer.
use super::*;
use crate::media_continuations::{MediaContinuations, MediaUploadRecord, MediaUploadStore};
use axum::{
    body::{to_bytes, Body},
    extract::State,
    http::{Request, StatusCode},
    routing::post,
    Json, Router,
};
use cmaeg_core::{
    activity::Activity,
    media_continuation::{sha256, supplement_text, MediaArtifactClaim, MediaContinuationKind},
};
use layer1_public::{
    rest::binding_media::*, AuthenticatedResourceCaller, AuthenticatedSubject, DelegatedCredential,
};
use layer2_core::sandbox::agent_attachments::*;
use serde_json::{json, Value};
use std::{path::PathBuf, sync::atomic::AtomicBool};
use tower::ServiceExt;

struct Peer {
    active: AtomicBool,
    outage: AtomicBool,
    expected: Mutex<Value>,
    artifacts: Mutex<Vec<MediaArtifactClaim>>,
}
async fn attest(
    State(peer): State<Arc<Peer>>,
    Json(mut body): Json<Value>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    if peer.outage.load(Ordering::SeqCst) {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    let artifact = body
        .as_object_mut()
        .unwrap()
        .remove("artifact")
        .and_then(|v| serde_json::from_value::<MediaArtifactClaim>(v).ok());
    let current = peer.active.load(Ordering::SeqCst)
        && body == *peer.expected.lock().unwrap()
        && artifact.is_some_and(|claim| peer.artifacts.lock().unwrap().contains(&claim));
    Json(json!({"current":current})).into_response()
}
struct Files {
    root: PathBuf,
    writes: AtomicUsize,
}
#[async_trait::async_trait]
impl AttachmentWriter for Files {
    async fn write(
        &self,
        _: &Sandbox,
        attachment: &AgentAttachment,
        bytes: Vec<u8>,
    ) -> Result<AgentAttachment, AttachmentError> {
        assert_eq!(sha256(&bytes), attachment.sha256);
        let path = self.root.join(attachment.path.trim_start_matches('/'));
        tokio::fs::create_dir_all(path.parent().unwrap())
            .await
            .unwrap();
        if let Ok(existing) = tokio::fs::read(&path).await {
            assert_eq!(existing, bytes);
        } else {
            tokio::fs::write(&path, &bytes).await.unwrap();
            self.writes.fetch_add(1, Ordering::SeqCst);
        }
        Ok(attachment.clone())
    }
}
struct LostReceipt {
    store: Arc<MongoSandboxStore>,
    lose: AtomicBool,
}
#[async_trait::async_trait]
impl MediaUploadStore for LostReceipt {
    async fn get_media_upload(
        &self,
        id: &str,
    ) -> Result<Option<MediaUploadRecord>, BindingMediaError> {
        self.store.get_media_upload(id).await
    }
    async fn reserve_media_upload(
        &self,
        value: &MediaUploadRecord,
    ) -> Result<MediaUploadRecord, BindingMediaError> {
        self.store.reserve_media_upload(value).await
    }
    async fn complete_media_upload(
        &self,
        value: &MediaUploadRecord,
        receipt: &AgentAttachment,
    ) -> Result<MediaUploadRecord, BindingMediaError> {
        let saved = self.store.complete_media_upload(value, receipt).await?;
        if self.lose.swap(false, Ordering::SeqCst) {
            Err(BindingMediaError::Unavailable)
        } else {
            Ok(saved)
        }
    }
}
struct Resources;
#[async_trait::async_trait]
impl SandboxResourceReader for Resources {
    async fn sandbox_resources(
        &self,
        _: &Sandbox,
    ) -> Result<SandboxResourceReading, SandboxResourceReadError> {
        unreachable!()
    }
}
struct Fixture {
    client: Client,
    name: String,
    store: Arc<MongoSandboxStore>,
    agent: Agent,
    parent: AdmittedAgentCommand,
    caller: AuthenticatedResourceCaller,
    service: Arc<MediaContinuations>,
    commands: Arc<AgentCommandService>,
    access: Arc<ResourceAccessService>,
    auth: Arc<AuthState>,
    peer: Arc<Peer>,
    server: tokio::task::JoinHandle<()>,
    files: Arc<Files>,
    bytes: Vec<u8>,
    path: BindingMediaPath,
    artifact: MediaArtifactClaim,
    expiry: i64,
}
impl Fixture {
    async fn new(label: &str, lose: bool) -> Self {
        let (client, name) = database(label).await;
        let (store, mut agent, sandbox, _, _, _) = composed(&client, &name).await;
        // Public history requires the complete creation/conversation prefix;
        // the lower-level command fixture only seeds its conversation boundary.
        store
            .agent_log
            .delete_many(doc! {"agent_id":agent.id.as_str()})
            .await
            .unwrap();
        for (index, kind) in [
            AgentLogKind::Created {
                workspace_id: agent.workspace_id.clone(),
                worktree_id: agent.worktree_id.clone(),
            },
            AgentLogKind::ConversationOpened {
                conversation_id: agent.conversation.conversation_id.clone().unwrap(),
                runtime_generation: 1,
            },
        ]
        .into_iter()
        .enumerate()
        {
            store
                .agent_log
                .insert_one(
                    crate::store::agent_log::log_document(&AgentLogEntry {
                        agent_id: agent.id.clone(),
                        seq: index as u64 + 1,
                        appended_at: agent.created_at.clone(),
                        kind,
                    })
                    .unwrap(),
                )
                .await
                .unwrap();
        }
        agent.conversation.last_seq = 2;
        store
            .agents
            .replace_one(
                doc! {"_id":agent.id.as_str()},
                crate::store::agents::agent_document(&agent).unwrap(),
            )
            .await
            .unwrap();
        let (mut binding, conversation, key) = binding_for(&store, &agent).await;
        // Seed a retained later Manager generation; Gateway's original generation is absent.
        binding.generation = 3;
        store
            .bindings
            .records
            .update_one(
                doc! {"_id":&binding.binding_id},
                doc! {"$set":{"generation":3_i64,"record.generation":3_i64}},
            )
            .await
            .unwrap();
        store
            .bindings
            .conversations
            .delete_many(doc! {})
            .await
            .unwrap();
        let mut conversation = conversation;
        conversation.generation = 3;
        conversation.revision = 1;
        conversation.state = BindingConversationState::Provisioning;
        store
            .save_binding_conversation(binding.revision, None, conversation.clone())
            .await
            .unwrap();
        conversation.state = BindingConversationState::Ready;
        conversation.revision = 2;
        store
            .save_binding_conversation(binding.revision, Some(1), conversation.clone())
            .await
            .unwrap();
        let mut original = reservation(&store, &agent, &sandbox, &binding).await;
        let bytes = vec![42; 3 * 1024 * 1024];
        let artifact = MediaArtifactClaim {
            attachment_index: 0,
            kind: MediaContinuationKind::Original,
            content_sha256: sha256(&bytes),
            source_sha256: sha256(&bytes),
            name: "original.bin".into(),
            media_type: "application/octet-stream".into(),
            size_bytes: bytes.len() as u64,
            text_sha256: None,
        };
        let activity: Activity = serde_json::from_value(json!({"schema_version":1,"event_id":"media-event","kind":{"type":"message","text":"inspect","mentions_bot":false,"attachments":[{"reference":"original","kind":"file","filename":"original.bin","mime_type":"application/octet-stream","size_bytes":bytes.len(),"source":{"type":"url","url":"https://source.example/file"}}]},"actor":{"id":"sender","kind":"human"},"conversation":{"id":conversation.conversation_id,"kind":"custom"},"occurred_at":layer2_core::now_rfc3339(),"source":{"type":"custom"}})).unwrap();
        activity.validate().unwrap();
        let event = ExternalEventContext {
            provider_conversation_id: None,
            source_id: "media-channel".into(),
            conversation_id: conversation.conversation_id.clone(),
            event_id: "media-event".into(),
            reference: "sealed-media-reference".into(),
            payload: json!({"activity":activity,"channel_id":"media-channel","conversation_id":conversation.conversation_id,"event_ref":"sealed-media-reference"}),
        };
        let AgentCommand::Message {
            input, metadata, ..
        } = &mut original.envelope.command
        else {
            unreachable!()
        };
        metadata.insert(
            "ordinary_private_metadata".into(),
            "retained original value".into(),
        );
        input.content.push(AgentContent::ExternalEvent {
            context: event.clone(),
        });
        original.envelope.request_fingerprint = original.envelope.command.fingerprint().unwrap();
        let parent = record(store.reserve_command(original).await.unwrap());
        let expiry = (SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 172_800) as i64;
        let peer = Arc::new(Peer {
            active: AtomicBool::new(true),
            outage: AtomicBool::new(false),
            expected: Mutex::new(
                json!({"event_ref":event.reference,"provider_binding_id":binding.binding_id,"channel_id":event.source_id,"conversation_id":event.conversation_id,"event_id":event.event_id,"activity_sha256":cmaeg_core::seal::event_ref::activity_digest(&activity),"binding_generation":null,"event_expires_at":expiry}),
            ),
            artifacts: Mutex::new(vec![artifact.clone()]),
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let app = Router::new()
            .route("/v1/event-attestations", post(attest))
            .with_state(peer.clone());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let auth = auth(store.clone());
        let bindings = BindingAccessService::new(store.clone());
        let principal = bindings.authenticate(&key.sha256()).await.unwrap().unwrap();
        let caller = AuthenticatedResourceCaller {
            context: context(&principal),
            credential: Some(DelegatedCredential::new(key.expose().into())),
        };
        let access = Arc::new(
            ResourceAccessService::new(store.clone(), auth.clone()).with_bindings(bindings),
        );
        let commands = AgentCommandService::new(store.clone(), access.clone(), auth.clone());
        let files = Arc::new(Files {
            root: std::env::temp_dir().join(&name),
            writes: AtomicUsize::new(0),
        });
        let uploads = Arc::new(LostReceipt {
            store: store.clone(),
            lose: AtomicBool::new(lose),
        });
        let service = MediaContinuations::new(
            commands.clone(),
            store.clone(),
            uploads,
            access.clone(),
            auth.clone(),
            AgentAttachmentService::with_writer(access.clone(), files.clone()),
            &origin,
        )
        .unwrap();
        let path = BindingMediaPath {
            binding_id: binding.binding_id,
            conversation_id: conversation.conversation_id,
            event_id: "media-event".into(),
            attachment_index: 0,
        };
        Self {
            client,
            name,
            store,
            agent,
            parent,
            caller,
            service,
            commands,
            access,
            auth,
            peer,
            server,
            files,
            bytes,
            path,
            artifact,
            expiry,
        }
    }
    fn base(&self) -> String {
        format!(
            "/api/v1/cmaeg/bindings/{}/conversations/{}/events/{}/media/0",
            self.path.binding_id, self.path.conversation_id, self.path.event_id
        )
    }
    fn params(&self) -> BindingMediaUploadParams {
        BindingMediaUploadParams {
            name: self.artifact.name.clone(),
            sha256: self.artifact.source_sha256.clone(),
            gateway_binding_generation: None,
            event_expires_at: self.expiry,
        }
    }
    fn supplement(&self, receipt: &AgentAttachment, text: &str) -> BindingMediaSupplement {
        let mut artifact = self.artifact.clone();
        if !text.is_empty() {
            artifact.kind = MediaContinuationKind::Transcript;
            artifact.content_sha256 = sha256(text.as_bytes());
        }
        artifact.text_sha256 = Some(sha256(
            supplement_text(&artifact, &receipt.path, text)
                .unwrap()
                .as_bytes(),
        ));
        self.peer.artifacts.lock().unwrap().push(artifact.clone());
        BindingMediaSupplement {
            gateway_binding_generation: None,
            event_expires_at: self.expiry,
            artifact: serde_json::to_value(artifact).unwrap(),
            derived_text: text.into(),
        }
    }
    async fn http(&self, mut request: Request<Body>) -> (StatusCode, Value) {
        request.extensions_mut().insert(AuthenticatedSubject(
            self.caller.context.caller.subject.clone(),
        ));
        request
            .extensions_mut()
            .insert(self.caller.context.caller.authorization.clone());
        request.extensions_mut().insert(self.caller.context.clone());
        request
            .extensions_mut()
            .insert(self.caller.credential.clone().unwrap());
        let response = binding_media_router(self.service.clone())
            .layer(axum::extract::DefaultBodyLimit::max(2 * 1024 * 1024))
            .oneshot(request)
            .await
            .unwrap();
        let status = response.status();
        let body =
            serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
        (status, body)
    }
    async fn upload(&self) -> Result<AgentAttachment, BindingMediaError> {
        self.service
            .clone()
            .prepare_upload(
                self.caller.clone(),
                self.path.clone(),
                self.params(),
                self.artifact.media_type.clone(),
                self.bytes.len() as u64,
            )
            .await?
            .upload(self.bytes.clone())
            .await
    }
    async fn close(self) {
        self.server.abort();
        self.client.database(&self.name).drop().await.unwrap();
        let _ = tokio::fs::remove_dir_all(&self.files.root).await;
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn media_continuation_upload_roundtrip_unknown_receipt_and_separate_response_replay() {
    let f = Fixture::new("media_roundtrip", true).await;
    let upload = || {
        Request::post(format!(
            "{}/attachment?name=original.bin&sha256={}&event_expires_at={}",
            f.base(),
            f.artifact.source_sha256,
            f.expiry
        ))
        .header("content-type", "application/octet-stream")
        .header("content-length", f.bytes.len())
        .body(Body::from(f.bytes.clone()))
        .unwrap()
    };
    assert_eq!(f.http(upload()).await.0, StatusCode::SERVICE_UNAVAILABLE);
    let (status, receipt) = f
        .http(
            Request::get(format!("{}/attachment", f.base()))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let receipt: AgentAttachment = serde_json::from_value(receipt).unwrap();
    assert_eq!(receipt.agent_id, f.agent.id);
    assert_eq!(
        tokio::fs::read(f.files.root.join(receipt.path.trim_start_matches('/')))
            .await
            .unwrap(),
        f.bytes
    );
    assert_eq!(f.http(upload()).await.0, StatusCode::CREATED);
    assert_eq!(f.files.writes.load(Ordering::SeqCst), 1);
    let record = f
        .store
        .get_media_upload(&receipt.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.binding_generation, 3);
    assert_eq!(record.source_generation, None);
    let body = f.supplement(&receipt, " exact derived text\n");
    let command = f
        .service
        .supplement(f.caller.clone(), f.path.clone(), body.clone())
        .await
        .unwrap();
    let duplicate = f
        .service
        .supplement(f.caller.clone(), f.path.clone(), body)
        .await
        .unwrap();
    assert_eq!(command.command, duplicate.command);
    assert_eq!(command.metadata, duplicate.metadata);
    assert_ne!(
        command.command.receipt.response_id,
        f.parent.receipt.response_id
    );
    let reads = AgentReadService::new(f.store.clone(), Arc::new(Resources))
        .with_resource_access(f.access.clone());
    let authorization = reads
        .authorize_event_stream(
            &f.caller.context,
            Some(f.caller.credential.as_ref().unwrap().value()),
            &f.agent.id,
        )
        .await
        .unwrap();
    let replay = f
        .commands
        .response_for_stream(
            &authorization,
            command.command.receipt.response_id.as_ref().unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(replay.metadata, command.metadata);
    let original_replay = f
        .commands
        .response_for_stream(
            &authorization,
            f.parent.receipt.response_id.as_ref().unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        original_replay.metadata,
        f.parent.receipt.envelope.response_metadata()
    );
    assert_eq!(
        original_replay.metadata["ordinary_private_metadata"],
        "retained original value"
    );
    assert_eq!(
        replay.metadata["cma_parent_response_id"],
        f.parent.receipt.response_id.as_ref().unwrap().to_string()
    );
    assert_eq!(
        f.store
            .get_command(&f.parent.receipt.envelope.command_id)
            .await
            .unwrap()
            .unwrap(),
        f.parent
    );
    let changed = f.supplement(&receipt, "changed transcript");
    assert!(matches!(
        f.service
            .supplement(f.caller.clone(), f.path.clone(), changed)
            .await,
        Err(BindingMediaError::Conflict)
    ));
    let mut wrong = f.params();
    wrong.sha256 = sha256(b"other");
    assert!(f
        .service
        .clone()
        .prepare_upload(
            f.caller.clone(),
            f.path.clone(),
            wrong,
            f.artifact.media_type.clone(),
            f.bytes.len() as u64
        )
        .await
        .is_err());
    let reopened = MongoSandboxStore::open(&f.client, &f.name).await.unwrap();
    assert_eq!(
        reopened
            .get_media_upload(&receipt.id)
            .await
            .unwrap()
            .unwrap()
            .receipt,
        Some(receipt)
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn media_continuation_pending_wrong_parent_and_revoked_source_cannot_admit() {
    let f = Fixture::new("media_refusals", false).await;
    let pending = f
        .service
        .clone()
        .prepare_upload(
            f.caller.clone(),
            f.path.clone(),
            f.params(),
            f.artifact.media_type.clone(),
            f.bytes.len() as u64,
        )
        .await
        .unwrap();
    let record = f
        .store
        .get_media_upload(&MediaUploadRecord::id(
            &f.parent.receipt.envelope.command_id,
            0,
        ))
        .await
        .unwrap()
        .unwrap();
    let body = f.supplement(&record.expected_receipt(), "");
    assert!(matches!(
        f.service
            .supplement(f.caller.clone(), f.path.clone(), body.clone())
            .await,
        Err(BindingMediaError::NotReady)
    ));
    assert!(matches!(
        f.service.attachment(f.caller.clone(), f.path.clone()).await,
        Err(BindingMediaError::NotFound)
    ));
    assert!(matches!(
        pending.upload(b"changed".to_vec()).await,
        Err(BindingMediaError::Conflict)
    ));
    assert_eq!(f.files.writes.load(Ordering::SeqCst), 0);
    let receipt = f.upload().await.unwrap();
    let mut wrong = f.path.clone();
    wrong.event_id = "other-event".into();
    assert!(matches!(
        f.service.attachment(f.caller.clone(), wrong).await,
        Err(BindingMediaError::Forbidden)
    ));
    f.peer.active.store(false, Ordering::SeqCst);
    assert!(matches!(
        f.service
            .supplement(f.caller.clone(), f.path.clone(), body)
            .await,
        Err(BindingMediaError::Forbidden)
    ));
    assert!(matches!(
        f.service.attachment(f.caller.clone(), f.path.clone()).await,
        Err(BindingMediaError::Forbidden)
    ));
    assert_eq!(
        f.store
            .get_media_upload(&receipt.id)
            .await
            .unwrap()
            .unwrap()
            .receipt,
        Some(receipt)
    );
    assert_eq!(
        f.store
            .agent_commands
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    f.close().await;
}

struct RevokingRuntime {
    peer: Arc<Peer>,
    at: usize,
    resolutions: AtomicUsize,
    outage: bool,
    sender: layer3_adapter::ConversationInputSender,
}
#[async_trait::async_trait]
impl AgentRuntimeRegistry for RevokingRuntime {
    async fn fence(&self, _: &AgentId, _: u64) -> Result<(), AgentRuntimeError> {
        unreachable!()
    }
    async fn resolve(
        &self,
        _: &AgentRuntimeTarget,
    ) -> Result<AgentRuntimeConnection, AgentRuntimeError> {
        if self.resolutions.fetch_add(1, Ordering::SeqCst) + 1 == self.at {
            if self.outage {
                self.peer.outage.store(true, Ordering::SeqCst);
            } else {
                self.peer.active.store(false, Ordering::SeqCst);
            }
        }
        Ok(AgentRuntimeConnection {
            capabilities: RuntimeCapabilities::new([RuntimeCapability::Conversation]),
            input: self.sender.clone(),
        })
    }
}
fn runtime(f: &Fixture, at: usize, outage: bool) -> (Arc<RevokingRuntime>, Arc<AtomicUsize>) {
    let sends = Arc::new(AtomicUsize::new(0));
    let count = sends.clone();
    let sender = layer3_adapter::ConversationInputSender::spawn(4, move |_| {
        count.fetch_add(1, Ordering::SeqCst);
        async { Ok(()) }
    });
    (
        Arc::new(RevokingRuntime {
            peer: f.peer.clone(),
            outage,
            at,
            resolutions: AtomicUsize::new(0),
            sender,
        }),
        sends,
    )
}
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn media_continuation_source_revocation_at_ordered_writer_blocks_runtime_input() {
    let f = Fixture::new("media_writer_revoke", false).await;
    let receipt = f.upload().await.unwrap();
    let admitted = f
        .service
        .supplement(
            f.caller.clone(),
            f.path.clone(),
            f.supplement(&receipt, "late transcript"),
        )
        .await
        .unwrap();
    f.store
        .reject_unclaimed_command(
            &f.parent.receipt.envelope.command_id,
            f.parent.revision,
            CommandFailureCode::AgentUnavailable,
            layer2_core::now_rfc3339(),
        )
        .await
        .unwrap();
    let claimed = f
        .store
        .claim_next_command(claim_request(
            &f.agent,
            "media-worker",
            &layer2_core::now_rfc3339(),
        ))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        claimed.receipt.envelope.command_id,
        admitted.command.receipt.command_id
    );
    let authority = f
        .auth
        .authorize_resource_command(&f.access, &claimed.receipt.envelope)
        .await
        .unwrap()
        .with_source_guard(f.service.clone());
    authority.current_snapshot().await.unwrap();
    let (runtime, sends) = runtime(&f, 2, false);
    let result = AgentOperationService::new(f.store.clone(), runtime)
        .execute_resource_claim(&authority, &claimed)
        .await;
    assert!(result.is_err());
    assert_eq!(sends.load(Ordering::SeqCst), 0);
    let saved = f
        .store
        .get_command(&claimed.receipt.envelope.command_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved.receipt, claimed.receipt);
    assert!(saved.work.dispatch.is_some());
    let agent = f.store.get_agent(&f.agent.id).await.unwrap().unwrap();
    assert_eq!(
        agent.conversation.operation.unwrap().state,
        AgentOperationDelivery::PreWriteFailed
    );
    assert_eq!(agent.status, AgentStatus::Ready);
    f.close().await;
}
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn media_continuation_append_preserves_active_original_turn() {
    let f = Fixture::new("media_active_original", false).await;
    let original = f
        .store
        .claim_next_command(claim_request(
            &f.agent,
            "original-worker",
            &layer2_core::now_rfc3339(),
        ))
        .await
        .unwrap()
        .unwrap();
    let authority = f
        .auth
        .authorize_resource_command(&f.access, &original.receipt.envelope)
        .await
        .unwrap();
    let (runtime, sends) = runtime(&f, usize::MAX, false);
    AgentOperationService::new(f.store.clone(), runtime)
        .execute_resource_claim(&authority, &original)
        .await
        .unwrap();
    let active = f.store.get_agent(&f.agent.id).await.unwrap().unwrap();
    assert_eq!(active.status, AgentStatus::Working);
    let receipt = f.upload().await.unwrap();
    let accepted = f
        .service
        .supplement(f.caller.clone(), f.path.clone(), f.supplement(&receipt, ""))
        .await
        .unwrap();
    let supplement = f
        .store
        .get_command(&accepted.command.receipt.command_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(supplement.status, AgentResponseStatus::Queued);
    assert!(matches!(
        supplement.receipt.envelope.command,
        AgentCommand::Message {
            order: AgentInputOrder::Append,
            ..
        }
    ));
    assert_eq!(
        f.store.get_agent(&f.agent.id).await.unwrap().unwrap(),
        active
    );
    assert_eq!(sends.load(Ordering::SeqCst), 1);
    f.close().await;
}

async fn deferred_fixture(label: &str) -> (Fixture, AdmittedAgentCommand) {
    let f = Fixture::new(label, false).await;
    let receipt = f.upload().await.unwrap();
    f.service
        .supplement(
            f.caller.clone(),
            f.path.clone(),
            f.supplement(&receipt, "retry transcript"),
        )
        .await
        .unwrap();
    f.store
        .reject_unclaimed_command(
            &f.parent.receipt.envelope.command_id,
            f.parent.revision,
            CommandFailureCode::AgentUnavailable,
            layer2_core::now_rfc3339(),
        )
        .await
        .unwrap();
    let claimed = f
        .store
        .claim_next_command(claim_request(
            &f.agent,
            "retry-worker",
            &layer2_core::now_rfc3339(),
        ))
        .await
        .unwrap()
        .unwrap();
    let authority = f
        .auth
        .authorize_resource_command(&f.access, &claimed.receipt.envelope)
        .await
        .unwrap()
        .with_source_guard(f.service.clone());
    let (runtime, sends) = runtime(&f, 2, true);
    assert!(AgentOperationService::new(f.store.clone(), runtime)
        .execute_resource_claim(&authority, &claimed)
        .await
        .is_err());
    assert_eq!(sends.load(Ordering::SeqCst), 0);
    (f, claimed)
}
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI and CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn media_continuation_unknown_requeue_commit_recovers_same_slot_after_source_outage() {
    let (f, claimed) = deferred_fixture("media_unknown_requeue").await;
    let app = format!("media-requeue-{}", uuid::Uuid::new_v4().simple());
    let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    options.app_name = Some(app.clone());
    let client = Client::with_options(options).unwrap();
    let retry_store = MongoSandboxStore::open(&client, &f.name).await.unwrap();
    let admin = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap())
        .await
        .unwrap();
    admin.database("admin").run_command(doc!{"configureFailPoint":"failCommand","mode":{"times":5_i32},"data":{"failCommands":["commitTransaction"],"appName":app,"writeConcernError":{"code":50_i32,"codeName":"MaxTimeMSExpired","errmsg":"injected media retry ambiguity"}}}).await.unwrap();
    let result = retry_store
        .reconcile_command(&claimed.receipt.envelope.command_id)
        .await;
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .unwrap();
    assert!(matches!(result, Err(CommandLedgerError::Unavailable)));
    let reopened = MongoSandboxStore::open(&f.client, &f.name).await.unwrap();
    let queued = reopened
        .reconcile_command(&claimed.receipt.envelope.command_id)
        .await
        .unwrap();
    assert_eq!(queued.status, AgentResponseStatus::Queued);
    assert_eq!(queued.receipt, claimed.receipt);
    assert_eq!(queued.work.deferred_attempts.len(), 1);
    assert_eq!(
        queued.work.deferred_attempts[0].dispatch.claim,
        claimed.work.lease.as_ref().unwrap().claim
    );
    assert!(queued.work.dispatch.is_none());
    assert!(matches!(
        f.service.check(&queued.receipt.envelope).await,
        Err(CommandAuthorizationError::Unavailable)
    ));
    assert_eq!(
        reopened
            .reconcile_command(&queued.receipt.envelope.command_id)
            .await
            .unwrap(),
        queued
    );
    if !queued
        .continuation_retry_delay(&layer2_core::now_rfc3339())
        .is_zero()
    {
        assert!(f
            .store
            .claim_next_command(claim_request(
                &f.agent,
                "early-retry",
                &layer2_core::now_rfc3339()
            ))
            .await
            .unwrap()
            .is_none());
    }
    f.peer.outage.store(false, Ordering::SeqCst);
    let receipt = f
        .service
        .attachment(f.caller.clone(), f.path.clone())
        .await
        .unwrap();
    let later = f
        .service
        .supplement(f.caller.clone(), f.path.clone(), f.supplement(&receipt, ""))
        .await
        .unwrap();
    assert!(later.command.receipt.agent_order > queued.receipt.agent_order);
    // A later artifact must not overtake this exact original queue position.
    assert!(f
        .store
        .claim_next_command(claim_request(
            &f.agent,
            "fifo-during-backoff",
            &queued.work.deferred_attempts[0].failed_at
        ))
        .await
        .unwrap()
        .is_none());
    tokio::time::sleep(queued.continuation_retry_delay(&layer2_core::now_rfc3339())).await;
    let retry = f
        .store
        .claim_next_command(claim_request(
            &f.agent,
            "recovered-worker",
            &layer2_core::now_rfc3339(),
        ))
        .await
        .unwrap()
        .unwrap();
    let authority = f
        .auth
        .authorize_resource_command(&f.access, &retry.receipt.envelope)
        .await
        .unwrap()
        .with_source_guard(f.service.clone());
    let (runtime, sends) = runtime(&f, usize::MAX, false);
    AgentOperationService::new(f.store.clone(), runtime)
        .execute_resource_claim(&authority, &retry)
        .await
        .unwrap();
    let saved = f
        .store
        .reconcile_command(&retry.receipt.envelope.command_id)
        .await
        .unwrap();
    assert_eq!(saved.receipt, claimed.receipt);
    assert_eq!(saved.work.deferred_attempts, queued.work.deferred_attempts);
    assert_eq!(saved.status, AgentResponseStatus::InProgress);
    assert_eq!(sends.load(Ordering::SeqCst), 1);
    f.close().await;
}
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn media_continuation_cancellation_racing_retry_claim_is_not_resurrected() {
    let (f, claimed) = deferred_fixture("media_retry_cancel").await;
    let queued = f
        .store
        .reconcile_command(&claimed.receipt.envelope.command_id)
        .await
        .unwrap();
    let submission = QueuedCancellationSubmission {
        agent_id: f.agent.id.clone(),
        target: QueuedCancellationTarget {
            response_id: queued.receipt.response_id.clone().unwrap(),
            source_command_id: queued.receipt.envelope.command_id.clone(),
            expected_command_revision: queued.revision,
        },
        idempotency_key: "cancel-pending-media".into(),
    };
    tokio::time::sleep(queued.continuation_retry_delay(&layer2_core::now_rfc3339())).await;
    let (cancel, retry) = tokio::join!(
        f.commands.cancel_queued_response(
            &f.caller.context,
            f.caller.credential.as_ref().unwrap().value(),
            CommandSubmissionSource::Api,
            submission.clone()
        ),
        f.store.claim_next_command(claim_request(
            &f.agent,
            "racing-retry",
            &layer2_core::now_rfc3339()
        ))
    );
    let retry = retry.unwrap();
    if let Some(retry) = retry {
        assert!(
            cancel.is_err(),
            "a live retry claim and queued cancellation cannot both win"
        );
        // No IO was attempted. Release the known claim, then explicitly cancel
        // the freshly observed revision; this does not clear a dispatch.
        f.store
            .release_command_claim(
                &retry.work.lease.as_ref().unwrap().claim,
                layer2_core::now_rfc3339(),
            )
            .await
            .unwrap();
        let current = f
            .store
            .get_command(&queued.receipt.envelope.command_id)
            .await
            .unwrap()
            .unwrap();
        let mut next = submission;
        next.target.expected_command_revision = current.revision;
        next.idempotency_key = "cancel-after-retry-claim-release".into();
        f.commands
            .cancel_queued_response(
                &f.caller.context,
                f.caller.credential.as_ref().unwrap().value(),
                CommandSubmissionSource::Api,
                next,
            )
            .await
            .unwrap();
    } else {
        cancel.unwrap();
    }
    f.peer.outage.store(false, Ordering::SeqCst);
    let saved = f
        .store
        .reconcile_command(&queued.receipt.envelope.command_id)
        .await
        .unwrap();
    assert_eq!(saved.status, AgentResponseStatus::Cancelled);
    assert_eq!(saved.receipt, queued.receipt);
    assert_eq!(saved.work.deferred_attempts, queued.work.deferred_attempts);
    assert!(f
        .store
        .claim_next_command(claim_request(
            &f.agent,
            "cancelled-retry",
            &layer2_core::now_rfc3339()
        ))
        .await
        .unwrap()
        .is_none());
    f.close().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn media_continuation_retry_requires_exact_atomic_no_write_pair_and_current_lifecycle() {
    let (f, claimed) = deferred_fixture("media_retry_proof").await;
    let saved = f
        .store
        .get_command(&claimed.receipt.envelope.command_id)
        .await
        .unwrap()
        .unwrap();
    let agent = f.store.get_agent(&f.agent.id).await.unwrap().unwrap();
    let logs = f
        .store
        .agent_log_prefix(&f.agent.id, agent.conversation.last_seq)
        .await
        .unwrap();
    let reads = AgentReadService::new(f.store.clone(), Arc::new(Resources))
        .with_resource_access(f.access.clone());
    let authorization = reads
        .authorize_event_stream(
            &f.caller.context,
            Some(f.caller.credential.as_ref().unwrap().value()),
            &f.agent.id,
        )
        .await
        .unwrap();
    let reading = f
        .commands
        .response_for_stream(&authorization, saved.receipt.response_id.as_ref().unwrap())
        .await
        .unwrap();
    let visible = reads.event_stream_snapshot(&authorization).await.unwrap();
    let failed = visible
        .history
        .iter()
        .find(|entry| entry.deferred_failure_detail.is_some())
        .unwrap();
    assert_eq!(
        failed.deferred_failure_detail,
        reading.deferred_failure_detail
    );
    assert!(
        matches!(&failed.kind,Some(AgentLogKind::TurnFailed {failure,..}) if failure.detail == "This Agent turn failed.")
    );
    let deferred = logs.iter().position(|entry| matches!(&entry.kind, AgentLogKind::TurnFailed { failure, .. } if failure.class == SOURCE_DEFERRED_FAILURE)).unwrap();
    let prewrite = deferred - 1;
    for case in [
        "valid",
        "generic",
        "wrong-fingerprint",
        "missing-pair",
        "wrong-operation",
        "different-commit",
        "unknown-write",
        "binding-revoked",
        "stop",
        "expired",
    ] {
        let mut evidence_logs = logs.clone();
        let mut current = saved.clone();
        let mut current_agent = agent.clone();
        match case {
            "generic" => {
                let AgentLogKind::TurnFailed { failure, .. } = &mut evidence_logs[deferred].kind
                else {
                    unreachable!()
                };
                failure.class = "input_not_written".into();
            }
            "wrong-fingerprint" => {
                let AgentLogKind::TurnFailed { failure, .. } = &mut evidence_logs[deferred].kind
                else {
                    unreachable!()
                };
                failure.detail.push('x');
            }
            "missing-pair" => {
                evidence_logs[prewrite].kind = AgentLogKind::OwnerMessage {
                    content: "no durable delivery proof".into(),
                }
            }
            "wrong-operation" => {
                let AgentLogKind::OperationDeliveryChanged {
                    operation_sequence, ..
                } = &mut evidence_logs[prewrite].kind
                else {
                    unreachable!()
                };
                *operation_sequence += 100;
            }
            "different-commit" => {
                evidence_logs[deferred].appended_at =
                    layer2_core::rfc3339_at(SystemTime::now() + Duration::from_secs(5))
            }
            "unknown-write" => {
                evidence_logs.truncate(prewrite + 1);
                let AgentLogKind::OperationDeliveryChanged { state, .. } =
                    &mut evidence_logs[prewrite].kind
                else {
                    unreachable!()
                };
                *state = AgentOperationDelivery::RecoveryRequired;
                current_agent.conversation.last_seq = evidence_logs.last().unwrap().seq;
                current_agent.status = AgentStatus::Working;
                current_agent.conversation.active_turn = current
                    .work
                    .dispatch
                    .as_ref()
                    .map(|dispatch| dispatch.turn_sequence);
            }
            "stop" => current_agent.requested_stop_reason = Some(AgentStopReason::Owner),
            "expired" => {
                current.receipt.admitted_at =
                    layer2_core::rfc3339_at(SystemTime::now() - Duration::from_secs(3600));
                current.queue_deadline =
                    layer2_core::rfc3339_at(SystemTime::now() - Duration::from_secs(1));
            }
            _ => {}
        }
        let mut evidence = CommandEvidence::new(&current).unwrap();
        for entry in &evidence_logs {
            evidence.observe(entry).unwrap();
        }
        let mut lane = CommandLane {
            last_order: current.receipt.agent_order,
            next_fence: 2,
            lease: current.work.lease.clone(),
        };
        evidence
            .apply_with_continuation_retry(
                &mut current,
                &mut lane,
                &current_agent,
                case != "binding-revoked",
            )
            .unwrap();
        if case == "valid" {
            assert_eq!(current.status, AgentResponseStatus::Queued);
            assert_eq!(current.work.deferred_attempts.len(), 1);
            assert_eq!(current.receipt, saved.receipt);
            assert!(current.work.dispatch.is_none());
            let mut unbounded = current.clone();
            unbounded.work.deferred_attempts = vec![current.work.deferred_attempts[0].clone(); 257];
            assert_eq!(unbounded.validate(), Err(CommandLedgerError::Corrupt));
        } else {
            assert!(current.work.deferred_attempts.is_empty(), "{case}");
            assert_eq!(current.work.dispatch, saved.work.dispatch, "{case}");
            assert_eq!(
                current.status,
                if case == "unknown-write" {
                    AgentResponseStatus::InProgress
                } else {
                    AgentResponseStatus::Failed
                },
                "{case}"
            );
        }
    }
    // A new process recovers the committed no-write logs without a saved requeue.
    let reopened = MongoSandboxStore::open(&f.client, &f.name).await.unwrap();
    let queued = reopened
        .reconcile_command(&claimed.receipt.envelope.command_id)
        .await
        .unwrap();
    assert_eq!(queued.status, AgentResponseStatus::Queued);
    assert_eq!(queued.work.deferred_attempts.len(), 1);
    assert_eq!(
        reopened
            .reconcile_command(&claimed.receipt.envelope.command_id)
            .await
            .unwrap(),
        queued
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn media_continuation_retirement_or_stop_fences_requeue_and_revocation_blocks_retry() {
    for change in ["binding", "stop-race", "source-after-requeue"] {
        let (f, claimed) = deferred_fixture("media_retry_fences").await;
        let id = &claimed.receipt.envelope.command_id;
        if change == "binding" {
            let mut binding = f.store.binding(&f.path.binding_id).await.unwrap().unwrap();
            let revision = binding.revision;
            binding.state = BindingState::Retired;
            binding.work = BindingWork::RetireCurrentConversations;
            binding
                .advance_revision(layer2_core::now_rfc3339())
                .unwrap();
            f.store.save_binding(Some(revision), binding).await.unwrap();
            let terminal = f.store.reconcile_command(id).await.unwrap();
            assert_eq!(terminal.status, AgentResponseStatus::Failed);
            assert_eq!(
                terminal.work.failure.unwrap().code,
                CommandFailureCode::AuthorityRevoked
            );
            assert!(terminal.work.deferred_attempts.is_empty());
        } else if change == "stop-race" {
            let mut stopped = f.store.get_agent(&f.agent.id).await.unwrap().unwrap();
            let revision = stopped.revision;
            stopped
                .request_stop(AgentStopReason::Owner, layer2_core::now_rfc3339())
                .unwrap();
            let (stop, requeue) = tokio::join!(
                f.store.save_agent(revision, stopped),
                f.store.reconcile_command(id)
            );
            stop.unwrap();
            let result = requeue.unwrap();
            assert!(matches!(
                result.status,
                AgentResponseStatus::Queued | AgentResponseStatus::Failed
            ));
            f.peer.outage.store(false, Ordering::SeqCst);
            if result.status == AgentResponseStatus::Queued {
                tokio::time::sleep(result.continuation_retry_delay(&layer2_core::now_rfc3339()))
                    .await;
                let retry = f
                    .store
                    .claim_next_command(claim_request(
                        &f.agent,
                        "stop-fenced-retry",
                        &layer2_core::now_rfc3339(),
                    ))
                    .await
                    .unwrap()
                    .unwrap();
                let authority = f
                    .auth
                    .authorize_resource_command(&f.access, &retry.receipt.envelope)
                    .await;
                if let Ok(authority) = authority {
                    let (runtime, sends) = runtime(&f, usize::MAX, false);
                    assert!(AgentOperationService::new(f.store.clone(), runtime)
                        .execute_resource_claim(
                            &authority.with_source_guard(f.service.clone()),
                            &retry
                        )
                        .await
                        .is_err());
                    assert_eq!(sends.load(Ordering::SeqCst), 0);
                }
            }
            let agent = f.store.get_agent(&f.agent.id).await.unwrap().unwrap();
            assert_eq!(agent.requested_stop_reason, Some(AgentStopReason::Owner));
        } else {
            let queued = f.store.reconcile_command(id).await.unwrap();
            f.peer.outage.store(false, Ordering::SeqCst);
            f.peer.active.store(false, Ordering::SeqCst);
            assert_eq!(
                f.service.check(&queued.receipt.envelope).await,
                Err(CommandAuthorizationError::Revoked)
            );
        }
        f.close().await;
    }
}
