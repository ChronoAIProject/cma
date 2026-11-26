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
