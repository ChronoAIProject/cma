use super::*;
use std::time::SystemTime;

struct Admission;
#[async_trait::async_trait]
impl AgentCommandAdmissionAuthority for Admission {
    async fn command_admission_authority(
        &self,
        context: &CallerContext,
        credential: &str,
        owner: &AgentOwner,
    ) -> Result<AgentAuthorizationRevision, CommandAuthorizationError> {
        if credential != "stream-fixture"
            || !matches!(owner,
            AgentOwner::Person { subject } if subject == &context.caller.subject)
        {
            return Err(CommandAuthorizationError::Revoked);
        }
        Ok(AgentAuthorizationRevision {
            authority_id: "containment-fixture".into(),
            policy_revision: 1,
            grant_revision: None,
        })
    }
}

pub(crate) fn controls_router(
    store: Arc<MongoSandboxStore>,
    reads: Arc<AgentReadService>,
) -> Router {
    let access = Arc::new(ResourceAccessService::new(
        store.clone(),
        Arc::new(super::super::fixture::ReadOnly),
    ));
    let commands = AgentCommandService::new(store, access, Arc::new(Admission));
    layer1_public::responses::v2::router(layer1_public::responses::v2::ResponsesV2State::new(
        layer1_public::agent_page::AgentPageOrigin::new("https://cma.example").unwrap(),
        commands,
        reads,
    ))
}

struct Runtime(Arc<AtomicUsize>);
#[async_trait::async_trait]
impl AgentRuntimeRegistry for Runtime {
    async fn resolve(
        &self,
        _: &AgentRuntimeTarget,
    ) -> Result<AgentRuntimeConnection, AgentRuntimeError> {
        let sends = self.0.clone();
        Ok(AgentRuntimeConnection {
            capabilities: RuntimeCapabilities::new([
                RuntimeCapability::Conversation,
                RuntimeCapability::Interrupt,
            ]),
            input: AgentRuntimeInputSender::spawn(4, move |_| {
                sends.fetch_add(1, Ordering::SeqCst);
                async { Ok(()) }
            }),
        })
    }
    async fn fence(&self, _: &AgentId, _: u64) -> Result<(), AgentRuntimeError> {
        Ok(())
    }
}

pub(super) async fn execute(
    replica: &Replica,
    authority: &Arc<Authority>,
    sends: Arc<AtomicUsize>,
) -> AgentCommandId {
    authority
        .run(async {
            let now = SystemTime::now();
            let claimed = replica
                .store
                .claim_next_command(CommandClaimRequest {
                    agent_id: authority.token.agent_id.clone(),
                    holder: "containment-worker".into(),
                    now: layer2_core::rfc3339_at(now),
                    expires_at: layer2_core::rfc3339_at(now + Duration::from_secs(30)),
                })
                .await
                .unwrap()
                .unwrap();
            let agent = replica
                .store
                .get_agent(&authority.token.agent_id)
                .await
                .unwrap()
                .unwrap();
            let caller = fixture::context(agent.owner).caller;
            AgentOperationService::new(replica.store.clone(), Arc::new(Runtime(sends)))
                .execute_claimed_command(&caller, &claimed)
                .await
                .unwrap();
            let settled = replica
                .store
                .reconcile_command(&claimed.receipt.envelope.command_id)
                .await
                .unwrap();
            if let Some(lease) = settled.work.lease {
                replica
                    .store
                    .release_command_claim(&lease.claim, layer2_core::now_rfc3339())
                    .await
                    .unwrap();
            }
            claimed.receipt.envelope.command_id
        })
        .await
        .unwrap()
}

pub(super) async fn start(
    replica: &Replica,
    authority: &Arc<Authority>,
    sends: Arc<AtomicUsize>,
) -> String {
    let response = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .post(format!(
            "{}/api/v2/agents/{}/responses",
            replica.address, authority.token.agent_id
        ))
        .header(
            "Idempotency-Key",
            format!("source-{}", authority.token.agent_id),
        )
        .json(&json!({"input":"synthetic source", "stream":true}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let mut stream = SseReader {
        response,
        buffered: String::new(),
    };
    let frame = stream.event("response.created").await;
    let id = frame.body["response"]["id"].as_str().unwrap().to_owned();
    execute(replica, authority, sends).await;
    id
}

pub(super) async fn call(
    replica: &Replica,
    method: reqwest::Method,
    path: &str,
    body: Option<Value>,
) -> (reqwest::StatusCode, Value) {
    let mut request = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap()
        .request(method, format!("{}{path}", replica.address));
    if let Some(body) = body {
        request = request
            .header("Idempotency-Key", format!("control-{path}"))
            .json(&body);
    }
    let response = request.send().await.unwrap();
    let status = response.status();
    (status, response.json().await.unwrap())
}

pub(super) async fn cancel(replica: &Replica, agent: &AgentId, response: &str) -> AgentCommandId {
    let path = format!("/api/v2/agents/{agent}/responses/{response}/controls");
    let (status, observed) = call(replica, reqwest::Method::GET, &path, None).await;
    assert_eq!(status, reqwest::StatusCode::OK, "{observed}");
    assert_eq!(observed["target_active"], true, "{observed}");
    let (status, accepted) = call(
        replica,
        reqwest::Method::POST,
        &path,
        Some(json!({
            "turn_sequence":observed["target"]["turn_sequence"],
            "expected_agent_revision":observed["agent_revision"], "command":{"type":"cancel"},
        })),
    )
    .await;
    assert_eq!(status, reqwest::StatusCode::ACCEPTED, "{accepted}");
    AgentCommandId(
        accepted["control"]["receipt"]["command_id"]
            .as_str()
            .unwrap()
            .into(),
    )
}
