use super::*;
use layer2_core::{
    agent_ownership::{
        test_support::MemoryOwnershipStore, Member, OwnershipCoordinator, PROTOCOL_VERSION,
    },
    ids::{AgentCommandId, AgentResponseId},
    sandbox::DurableAgentUpdateRecorder,
};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Mutex,
};

pub(in crate::agent_manager::relay::admission) struct ObservedStore {
    pub inner: Arc<dyn AgentCommandStore>,
    pub instance: String,
    pub calls: Mutex<Vec<OwnershipToken>>,
    pub lookups: AtomicUsize,
}
#[async_trait::async_trait]
impl AgentCommandStore for ObservedStore {
    async fn reserve_command(
        &self,
        request: CommandReservation,
    ) -> Result<CommandReservationResult, CommandLedgerError> {
        let authority = layer2_core::agent_ownership::execution_authority()
            .expect("admission must enter actual owner scope");
        assert!(authority.is_current());
        assert_eq!(authority.token.instance_id, self.instance);
        assert_eq!(authority.token.agent_id, request.envelope.agent_id);
        assert!(authority.token.epoch > 0);
        self.calls.lock().unwrap().push(authority.token.clone());
        self.inner.reserve_command(request).await
    }
    async fn edit_queued_input(
        &self,
        request: QueuedInputReservation,
    ) -> Result<QueuedInputOutcome, CommandLedgerError> {
        let authority = layer2_core::agent_ownership::execution_authority()
            .expect("edit must execute on owner");
        assert!(authority.is_current());
        assert_eq!(authority.token.instance_id, self.instance);
        assert_eq!(
            authority.token.agent_id,
            request.cancellation.command.envelope.agent_id
        );
        assert!(authority.token.epoch > 0);
        self.calls.lock().unwrap().push(authority.token.clone());
        self.inner.edit_queued_input(request).await
    }
    async fn lookup_queued_input(
        &self,
        request: QueuedInputReservation,
    ) -> Result<Option<AdmittedQueuedInput>, CommandLedgerError> {
        self.lookups.fetch_add(1, Ordering::SeqCst);
        self.inner.lookup_queued_input(request).await
    }
    async fn get_command(
        &self,
        id: &AgentCommandId,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError> {
        self.inner.get_command(id).await
    }
    async fn get_response_command(
        &self,
        id: &AgentResponseId,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError> {
        self.inner.get_response_command(id).await
    }
    async fn get_admitted_command(
        &self,
        scope: &str,
        key: &str,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError> {
        self.inner.get_admitted_command(scope, key).await
    }
}
pub(super) struct Peer {
    pub coordinator: Arc<OwnershipCoordinator>,
    pub service: Arc<AgentCommandService>,
    pub writes: Arc<ObservedStore>,
    pub drop_reply: Arc<AtomicBool>,
    pub release_reply: Arc<AtomicBool>,
    pub relay: Arc<Relay>,
    _updates: Arc<DurableAgentUpdateRecorder>,
    server: tokio::task::JoinHandle<()>,
}
impl Drop for Peer {
    fn drop(&mut self) {
        self.server.abort();
    }
}
pub(super) struct Cluster {
    pub auth: crate::auth::relay::tests::Fixture,
    pub ownership: Arc<MemoryOwnershipStore>,
    pub peers: Vec<Peer>,
    pub id: AgentId,
}
#[derive(Clone)]
struct ReplyFaults {
    lose: Arc<AtomicBool>,
    release: Arc<AtomicBool>,
    coordinator: Arc<OwnershipCoordinator>,
}
async fn lose_one_reply(
    State(faults): State<ReplyFaults>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let response = next.run(request).await;
    if faults.release.swap(false, Ordering::SeqCst) {
        // The handler has produced its valid reply. Release before the ingress
        // validates the reply's owner, without changing any response bytes.
        for authority in faults.coordinator.begin_drain().await {
            faults
                .coordinator
                .handoff(&authority, async { Ok(()) })
                .await
                .unwrap();
        }
    }
    if faults.lose.swap(false, Ordering::SeqCst) {
        axum::http::StatusCode::SERVICE_UNAVAILABLE.into_response()
    } else {
        response
    }
}
impl Cluster {
    pub async fn new() -> Self {
        let auth = crate::auth::relay::tests::Fixture::new(true).await;
        let id = AgentId("agt-peer-admission".into());
        let resources = super::super::super::public_access::fixture::fixture(
            auth.auth.clone(),
            id.as_str(),
            "owner",
        )
        .await;
        let ownership = Arc::new(MemoryOwnershipStore::default());
        let mut peers = Vec::new();
        for index in 0..2 {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let coordinator = OwnershipCoordinator::register(
                ownership.client(),
                Member {
                    instance_id: format!("admission-{index}"),
                    protocol: PROTOCOL_VERSION,
                    relay_address: format!("http://{}", listener.local_addr().unwrap()),
                    expires_at_ms: 0,
                    draining: false,
                    owned_agents: 0,
                    max_owned_agents: 32,
                },
            )
            .await
            .unwrap();
            let updates = DurableAgentUpdateRecorder::new(resources.store.clone());
            let relay = Relay::new(
                coordinator.clone(),
                &updates,
                ring::hmac::Key::new(ring::hmac::HMAC_SHA256, b"admission-peer-fixture-key"),
            )
            .unwrap();
            let writes = Arc::new(ObservedStore {
                inner: resources.store.clone(),
                instance: coordinator.instance_id.clone(),
                calls: Mutex::default(),
                lookups: AtomicUsize::new(0),
            });
            let router = AdmissionRouter::new("public", auth.auth.clone());
            let service = router.compose(AgentCommandService::new(
                writes.clone(),
                resources.access.clone(),
                auth.auth.clone(),
            ));
            relay.install_admissions(vec![router.clone()]).unwrap();
            let drop_reply = Arc::new(AtomicBool::new(false));
            let release_reply = Arc::new(AtomicBool::new(false));
            let app =
                relay
                    .routes(axum::Router::new())
                    .layer(axum::middleware::from_fn_with_state(
                        ReplyFaults {
                            lose: drop_reply.clone(),
                            release: release_reply.clone(),
                            coordinator: coordinator.clone(),
                        },
                        lose_one_reply,
                    ));
            let server = tokio::spawn(async move {
                axum::serve(listener, app).await.unwrap();
            });
            peers.push(Peer {
                coordinator,
                service,
                writes,
                drop_reply,
                release_reply,
                relay,
                _updates: updates,
                server,
            });
        }
        Self {
            auth,
            ownership,
            peers,
            id,
        }
    }
    pub async fn owner(&self) -> (usize, Arc<Authority>) {
        use layer2_core::agent_ownership::{candidates, OwnershipStore};
        let membership = self.ownership.membership().await.unwrap();
        let selected = candidates(&self.id, &membership, None).unwrap()[0]
            .instance_id
            .clone();
        let index = self
            .peers
            .iter()
            .position(|peer| peer.coordinator.instance_id == selected)
            .unwrap();
        let authority = self.peers[index]
            .coordinator
            .acquire(&self.id)
            .await
            .unwrap()
            .unwrap();
        (index, authority)
    }
    pub fn evidence(&self) -> OwnerCommandRoutingEvidence {
        Origin::capture(
            &axum::http::Method::POST,
            &"/api/v2/responses".parse().unwrap(),
            &self.auth.headers,
        )
        .unwrap()
        .evidence()
        .unwrap()
    }
    pub async fn edit(
        &self,
        ingress: usize,
        submission: QueuedInputSubmission,
    ) -> Result<QueuedInputStatusView, QueuedCancellationError> {
        let uri = format!(
            "/api/v2/agents/{}/responses/{}/queued-input",
            self.id, submission.target.response_id
        )
        .parse()
        .unwrap();
        Origin::capture(&axum::http::Method::POST, &uri, &self.auth.headers)
            .unwrap()
            .evidence()
            .unwrap()
            .scope(self.peers[ingress].service.edit_queued_input(
                &self.auth.context,
                "test-access",
                CommandSubmissionSource::Api,
                submission,
            ))
            .await
    }
    pub fn submission(&self, key: &str) -> CommandSubmission {
        CommandSubmission {
            agent_id: self.id.clone(),
            command: AgentCommand::message("keep the original input"),
            idempotency_key: key.into(),
            expected_agent_revision: None,
            requested_model: None,
        }
    }
    pub async fn admit(
        &self,
        ingress: usize,
        key: &str,
    ) -> Result<CommandStatusView, CommandAdmissionError> {
        self.evidence()
            .scope(self.peers[ingress].service.admit(
                &self.auth.context,
                "test-access",
                CommandSubmissionSource::Api,
                self.submission(key),
            ))
            .await
    }
}
