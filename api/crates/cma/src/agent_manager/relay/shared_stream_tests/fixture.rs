use super::*;

struct ReadOnly;
#[async_trait::async_trait]
impl OrganizationAuthority for ReadOnly {
    async fn organization_context(
        &self,
        _: &Caller,
        _: &str,
        _: &SubjectId,
    ) -> Result<CallerContext, MembershipError> {
        Err(MembershipError::Unavailable)
    }
    async fn organization_choices(
        &self,
        _: &Caller,
        _: &str,
    ) -> Result<Vec<OrganizationChoice>, MembershipError> {
        Err(MembershipError::Unavailable)
    }
}
#[async_trait::async_trait]
impl AgentCommandAdmissionAuthority for ReadOnly {
    async fn command_admission_authority(
        &self,
        _: &CallerContext,
        _: &str,
        _: &AgentOwner,
    ) -> Result<AgentAuthorizationRevision, CommandAuthorizationError> {
        Err(CommandAuthorizationError::Revoked)
    }
}
#[async_trait::async_trait]
impl SandboxResourceReader for ReadOnly {
    async fn sandbox_resources(
        &self,
        _: &Sandbox,
    ) -> Result<SandboxResourceReading, SandboxResourceReadError> {
        unreachable!("SSE must not read runtime resource usage")
    }
}

pub(super) struct Replica {
    pub(super) store: Arc<MongoSandboxStore>,
    pub(super) coordinator: Arc<OwnershipCoordinator>,
    pub(super) updates: Arc<DurableAgentUpdateRecorder>,
    address: String,
    server: JoinHandle<()>,
    renewal: JoinHandle<()>,
}
impl Drop for Replica {
    fn drop(&mut self) {
        self.coordinator.stop_admission();
        self.server.abort();
        self.renewal.abort();
    }
}
impl Replica {
    pub(super) async fn open(name: &str, owner: &SubjectId) -> Self {
        // Construct a new Client, not a clone sharing another replica's pool.
        let client = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
            .await
            .unwrap();
        let instance = uuid::Uuid::new_v4().simple().to_string();
        let store = Arc::new(
            MongoSandboxStore::open(&client, name)
                .await
                .unwrap()
                .enable_agent_ownership(instance.clone())
                .await
                .unwrap(),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let coordinator = OwnershipCoordinator::register(
            store.agent_ownership.clone().unwrap(),
            Member {
                instance_id: instance,
                protocol: PROTOCOL_VERSION,
                relay_address: address.clone(),
                expires_at_ms: 0,
                draining: false,
                owned_agents: 0,
                max_owned_agents: 256,
            },
        )
        .await
        .unwrap();
        let updates = DurableAgentUpdateRecorder::new(store.clone());
        let relay = Relay::new(
            coordinator.clone(),
            &updates,
            ring::hmac::Key::new(ring::hmac::HMAC_SHA256, b"synthetic-stream-fixture"),
        )
        .unwrap();
        updates.install_live_source(relay.live_source());
        let access = Arc::new(ResourceAccessService::new(
            store.clone(),
            Arc::new(ReadOnly),
        ));
        let context = context(owner.clone());
        let public = router(
            AgentV2State::new(
                AgentReadService::new(store.clone(), Arc::new(ReadOnly))
                    .with_resource_access(access.clone()),
                AgentCommandService::new(store.clone(), access, Arc::new(ReadOnly)),
            )
            .with_updates(updates.clone())
            // A delivered head hint must wake well before this fallback.
            .with_event_wake(true, Duration::from_secs(20)),
        )
        .layer(Extension(layer1_public::AuthenticatedSubject(
            owner.clone(),
        )))
        .layer(Extension(context.caller.authorization.clone()))
        .layer(Extension(context));
        // Public SSE reads its local store and follows the shared owner through
        // the private relay. It is not forwarded to a public owner endpoint.
        let app = public.merge(relay.routes(Router::new()));
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let renew = coordinator.clone();
        let renewal = tokio::spawn(async move {
            let mut ticks = tokio::time::interval(Duration::from_millis(HEARTBEAT_INTERVAL_MS));
            loop {
                ticks.tick().await;
                if renew.is_draining() {
                    break;
                }
                let result = renew.maintain().await;
                if !renew.is_draining() {
                    result.unwrap();
                }
            }
        });
        Self {
            store,
            coordinator,
            updates,
            address,
            server,
            renewal,
        }
    }

    pub(super) async fn stream(
        &self,
        id: &AgentId,
        after: Option<&str>,
        follow: bool,
    ) -> SseReader {
        let mut request = reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .get(format!("{}/api/v2/agents/{id}/events", self.address))
            .query(&[("follow", follow.to_string())]);
        if let Some(cursor) = after {
            request = request.query(&[("after_cursor", cursor)]);
        }
        let response = tokio::time::timeout(Duration::from_secs(5), request.send())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        assert!(response.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/event-stream"));
        SseReader {
            response,
            buffered: String::new(),
        }
    }
}

impl Replica {
    pub(super) async fn close(mut self) {
        for authority in self.coordinator.begin_drain().await {
            self.coordinator
                .handoff(&authority, async { Ok(()) })
                .await
                .unwrap();
        }
        self.server.abort();
        self.renewal.abort();
        for task in [&mut self.server, &mut self.renewal] {
            if let Err(error) = task.await {
                assert!(error.is_cancelled(), "fixture task failed: {error}");
            }
        }
    }
}

fn context(subject: SubjectId) -> CallerContext {
    CallerContext {
        credential_source: CredentialSource::Retained,
        caller: Caller::new(
            subject.clone(),
            Authorization::new([Capability::UseOwnSandboxes]),
        ),
        credential: CredentialContext {
            issuer: "https://identity.example.test".into(),
            subject: subject.clone(),
            credential_id: "stream-fixture".into(),
        },
        actor: ResourceActor::Human { subject },
        memberships: Vec::new(),
    }
}

pub(super) async fn seed(store: &MongoSandboxStore, db: &mongodb::Database) -> Agent {
    use layer2_core::profiles::{
        WorkspaceLocation, WorkspaceProfileContent, WorkspaceProfileSnapshot,
    };
    let (sandbox, mut agent) = retained_fixture(store, "shared-stream").await;
    // Maintenance's minimal fixture starts with ConversationOpened. Public
    // replay requires the complete retained history, including creation.
    use mongodb::bson::{doc, Document};
    let logs = db.collection::<Document>("agent_log");
    let filter = doc! {"agent_id": agent.id.as_str(), "seq": 1_i64};
    let mut opened = logs.find_one(filter.clone()).await.unwrap().unwrap();
    opened.insert("_id", format!("{}:{:020}", agent.id, 2));
    opened.insert("seq", 2_i64);
    opened
        .get_document_mut("record")
        .unwrap()
        .insert("seq", 2_i64);
    let created = AgentLogKind::Created {
        workspace_id: agent.workspace_id.clone(),
        worktree_id: agent.worktree_id.clone(),
    };
    logs.update_one(
        filter,
        doc! {"$set": {"record.kind": mongodb::bson::serialize_to_bson(&created).unwrap()}},
    )
    .await
    .unwrap();
    logs.insert_one(opened).await.unwrap();
    db.collection::<Document>("agents")
        .update_one(
            doc! {"_id": agent.id.as_str()},
            doc! {"$set": {"last_seq": 2_i64, "record.conversation.last_seq": 2_i64}},
        )
        .await
        .unwrap();
    agent.conversation.last_seq = 2;
    let now = layer2_core::now_rfc3339();
    store
        .create_workspace(Workspace {
            id: agent.workspace_id.clone(),
            sandbox_id: sandbox.id.clone(),
            owner: sandbox.owner,
            origin: WorkspaceOrigin::Manual,
            profile: WorkspaceProfileSnapshot {
                id: WorkspaceProfileId("wkp_stream".into()),
                revision: 1,
                digest: "d".repeat(64),
                content: WorkspaceProfileContent {
                    location: WorkspaceLocation::Directory {
                        name: "stream".into(),
                    },
                    name: "stream".into(),
                    setup_commands: vec![],
                    agent_profiles: vec![],
                },
            },
            name: "stream".into(),
            path: "/home/cma/stream".into(),
            source: WorkspaceSource::Empty,
            resolved_commit: None,
            status: WorkspaceStatus::Ready,
            work: WorkspaceWorkState::new(WorkspaceWorkStep::None, now.clone()),
            next_worktree_number: 2,
            created_at: now.clone(),
            ready_at: Some(now),
            failed: None,
            removed_at: None,
            revision: 1,
        })
        .await
        .unwrap();
    store
        .create_worktree(Worktree {
            id: agent.worktree_id.clone(),
            workspace_id: agent.workspace_id.clone(),
            sandbox_id: agent.sandbox_id.clone(),
            owner: agent.owner.clone(),
            name: "wt-1".into(),
            path: "/home/cma/stream/.worktrees/wt-1".into(),
            branch: "cma/wt-1".into(),
            base_commit: "a".repeat(40),
            status: WorktreeStatus::Ready,
            agent_id: Some(agent.id.clone()),
            revision: 1,
        })
        .await
        .unwrap();
    agent
}
