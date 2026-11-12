use super::*;
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Router,
};
use layer1_public::agent_v2::{router, AgentV2State};
use tower::ServiceExt;

struct Authority;
#[async_trait::async_trait]
impl OrganizationAuthority for Authority {
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
impl AgentCommandAdmissionAuthority for Authority {
    async fn command_admission_authority(
        &self,
        _: &CallerContext,
        _: &str,
        _: &AgentOwner,
    ) -> Result<AgentAuthorizationRevision, CommandAuthorizationError> {
        Err(CommandAuthorizationError::Revoked)
    }
}
struct Resources;
#[async_trait::async_trait]
impl SandboxResourceReader for Resources {
    async fn sandbox_resources(
        &self,
        _: &Sandbox,
    ) -> Result<SandboxResourceReading, SandboxResourceReadError> {
        unreachable!("history does not inspect runtime resources")
    }
}

pub(super) struct Fixture {
    pub client: Client,
    pub name: String,
    pub store: MongoSandboxStore,
    pub agent: Agent,
    pub meter: Arc<meter::Meter>,
}
impl Fixture {
    pub async fn new(rows: u64) -> Self {
        let meter = Arc::new(meter::Meter::default());
        let client = meter.client().await;
        let name = format!("cma_history_page_{}", uuid::Uuid::new_v4().simple());
        let (store, target) = super::super::fixture(&client, &name).await;
        let original = store.resource_access_snapshot(&target).await.unwrap();
        let mut agent = original.lineage.agent.unwrap();
        let parent = original.lineage.sandbox;
        let mut commands = Vec::new();
        for (order, operation, command) in [
            (1, 3, AgentCommand::message("Fixture request")),
            (
                2,
                4,
                AgentCommand::Steer {
                    input: AgentInput::text("Fixture steering"),
                },
            ),
        ] {
            let mut request = super::super::super::command_ledger_tests::request(
                &agent,
                &parent,
                &format!("history-{order}"),
            );
            request.envelope.command = command;
            request.envelope.request_fingerprint = request.envelope.command.fingerprint().unwrap();
            if order == 2 {
                request.response_id = None;
            }
            let mut record = request
                .record(order, operation - 1, (order == 2).then_some(3))
                .unwrap();
            record.status = AgentResponseStatus::InProgress;
            record.work.dispatch = Some(CommandDispatch {
                claim: CommandClaim {
                    agent_id: agent.id.clone(),
                    command_id: record.receipt.envelope.command_id.clone(),
                    holder: "history-fixture".into(),
                    fence: 1,
                },
                operation_sequence: operation,
                turn_sequence: 3,
                runtime_generation: 1,
                fenced_at: NOW.into(),
                delivery: AgentOperationDelivery::DispatchFenced,
            });
            store
                .agent_commands
                .insert_one(crate::store::agent_commands::records::document(&record).unwrap())
                .await
                .unwrap();
            commands.push(record);
        }
        let mut entries = Vec::new();
        for seq in 3..=rows {
            let kind = match seq {
                3 => AgentLogKind::TurnStarted {
                    model: None,
                    turn_sequence: 3,
                    origin_key: Some(format!(
                        "command:{}",
                        commands[0].receipt.envelope.command_id
                    )),
                },
                4 => AgentLogKind::OperationDeliveryChanged {
                    operation_sequence: 4,
                    operation: AgentOperationKind::Steer,
                    state: AgentOperationDelivery::DispatchFenced,
                },
                5 => AgentLogKind::OwnerMessage {
                    content: "Fixture steering".into(),
                },
                _ => AgentLogKind::ModeChanged {
                    mode: "code".into(),
                },
            };
            entries.push(
                log_document(&AgentLogEntry {
                    agent_id: agent.id.clone(),
                    seq,
                    appended_at: NOW.into(),
                    kind,
                })
                .unwrap(),
            );
        }
        store.agent_log.insert_many(entries).await.unwrap();
        agent.conversation.last_seq = rows;
        agent.conversation.active_turn = Some(3);
        agent.status = AgentStatus::Working;
        agent.validate_conversation().unwrap();
        agent.revision += 1;
        store
            .agents
            .replace_one(
                doc! {"_id":agent.id.as_str()},
                crate::store::agents::agent_document(&agent).unwrap(),
            )
            .await
            .unwrap();
        Self {
            client,
            name,
            store,
            agent,
            meter,
        }
    }
    pub fn app(&self, access_session: bool, single_fence_pages: bool) -> Router {
        let store = Arc::new(
            self.store
                .clone()
                .with_access_snapshot_session(access_session),
        );
        let access = Arc::new(ResourceAccessService::new(
            store.clone(),
            Arc::new(Authority),
        ));
        router(
            AgentV2State::new(
                AgentReadService::new(store.clone(), Arc::new(Resources))
                    .with_resource_access(access.clone()),
                AgentCommandService::new(store, access, Arc::new(Authority)),
            )
            .with_single_fence_pages(single_fence_pages),
        )
    }
    pub async fn request(&self, app: &Router, path: &str) -> Vec<u8> {
        let mut request = Request::get(path)
            .header("prefer", "cma-message-bindings")
            .body(Body::empty())
            .unwrap();
        request
            .extensions_mut()
            .insert(layer1_public::AuthenticatedSubject(
                self.agent.owner.clone(),
            ));
        request
            .extensions_mut()
            .insert(Authorization::new([Capability::UseOwnSandboxes]));
        request.extensions_mut().insert(CallerContext {
            credential_source: CredentialSource::Retained,
            caller: Caller::new(
                self.agent.owner.clone(),
                Authorization::new([Capability::UseOwnSandboxes]),
            ),
            credential: CredentialContext {
                issuer: "https://identity.example.test".into(),
                subject: self.agent.owner.clone(),
                credential_id: "history-fixture".into(),
            },
            actor: ResourceActor::Human {
                subject: self.agent.owner.clone(),
            },
            memberships: Vec::new(),
        });
        let response = app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 8 * 1024 * 1024)
            .await
            .unwrap();
        assert_eq!(
            status,
            StatusCode::OK,
            "{}",
            String::from_utf8_lossy(&bytes)
        );
        bytes.to_vec()
    }
    pub async fn idle(&self) {
        let mut agent = self.agent.clone();
        agent.status = AgentStatus::Ready;
        agent.conversation.active_turn = None;
        agent.revision += 1;
        self.store
            .agents
            .replace_one(
                doc! {"_id":agent.id.as_str()},
                crate::store::agents::agent_document(&agent).unwrap(),
            )
            .await
            .unwrap();
    }
    pub async fn close(self) {
        self.client.database(&self.name).drop().await.unwrap();
        self.client.shutdown().await;
    }
}
