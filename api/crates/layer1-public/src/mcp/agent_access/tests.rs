use super::*;
use axum::{body::Body, http::Request as HttpRequest};
use layer2_core::sandbox::agent_access::{AgentAccessRequest, AgentAccessStatus};
use layer2_core::sandbox::agent_access_tool::{AgentAccessProposal, AgentAccessRequester};
use layer2_core::sandbox::{
    AgentCreationOrigin, AgentCreationProvenance, AgentOrigin, RuntimeBinding,
};
use layer2_core::{AgentId, SandboxId, SubjectId, WorkspaceId, WorktreeId};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Mutex,
};
use tower::ServiceExt;

#[path = "availability_tests.rs"]
mod availability_tests;
#[path = "deliverable_error_tests.rs"]
mod deliverable_error_tests;
#[path = "deliverable_tests.rs"]
mod deliverable_tests;
#[path = "document_tests.rs"]
mod document_tests;
#[path = "protocol_tests.rs"]
mod protocol_tests;

struct Stub {
    bearer: String,
    confirmed: AtomicBool,
    setup: AtomicUsize,
    calls: AtomicUsize,
    proposals: Mutex<Vec<(AgentAccessRequester, AgentAccessProposal)>>,
    failure: Mutex<Option<AgentAccessError>>,
    returned: Mutex<Option<AgentAccessRequest>>,
}
impl Stub {
    fn new(confirmed: bool) -> Arc<Self> {
        Arc::new(Self {
            bearer: format!(
                "cma_at_{}",
                uuid::Uuid::new_v4().simple().to_string().repeat(2)
            ),
            confirmed: AtomicBool::new(confirmed),
            setup: AtomicUsize::new(0),
            calls: AtomicUsize::new(0),
            proposals: Mutex::new(vec![]),
            failure: Mutex::new(None),
            returned: Mutex::new(None),
        })
    }
    fn router(self: &Arc<Self>) -> Router {
        agent_access_router(self.clone(), self.clone())
    }
}

#[async_trait::async_trait]
impl AgentAccessToolAuthenticator for Stub {
    async fn authenticate_setup(&self, capability: &str) -> Result<(), AgentAccessError> {
        self.setup.fetch_add(1, Ordering::SeqCst);
        (capability == self.bearer)
            .then_some(())
            .ok_or(AgentAccessError::Forbidden)
    }
    async fn authenticate(
        &self,
        capability: &str,
    ) -> Result<AgentAccessRequester, AgentAccessError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        (capability == self.bearer && self.confirmed.load(Ordering::SeqCst))
            .then(requester)
            .ok_or(AgentAccessError::Forbidden)
    }
}
#[async_trait::async_trait]
impl AgentAccessToolAdmission for Stub {
    async fn request(
        &self,
        identity: AgentAccessRequester,
        proposal: AgentAccessProposal,
    ) -> Result<AgentAccessRequest, AgentAccessError> {
        if let Some(error) = self.failure.lock().unwrap().clone() {
            return Err(error);
        }
        self.proposals
            .lock()
            .unwrap()
            .push((identity.clone(), proposal.clone()));
        if let Some(value) = self.returned.lock().unwrap().clone() {
            return Ok(value);
        }
        Ok(AgentAccessRequest {
            retry_of: proposal.retry_of,
            id: "access-request-one".into(),
            agent_id: identity.agent_id,
            status: AgentAccessStatus::Pending,
            services: proposal.services,
            reason: proposal.reason,
            error: None,
            github_approval: None,
        })
    }
}

fn requester() -> AgentAccessRequester {
    let agent_id = AgentId("agt_access".into());
    let sandbox_id = SandboxId("sbx_access".into());
    let owner = SubjectId("owner".into());
    AgentAccessRequester {
        agent_id: agent_id.clone(),
        sandbox_id: sandbox_id.clone(),
        owner: owner.clone(),
        conversation: "original-native-conversation".into(),
        runtime: RuntimeBinding {
            runtime_id: "runtime-one".into(),
            runtime_version: "pinned-image".into(),
            provision_generation: Some(1),
            boot_id: Some("a".repeat(32)),
            bound_at: "2026-10-13T00:00:00Z".into(),
        },
        creation: AgentCreationProvenance {
            schema_version: 1,
            agent_id,
            agent_created_at: "2026-10-13T00:00:00Z".into(),
            sandbox_id,
            sandbox_created_at: "2026-10-13T00:00:00Z".into(),
            workspace_id: WorkspaceId("wks_access".into()),
            worktree_id: WorktreeId("wkt_access".into()),
            owner: layer2_core::resource_access::ResourceOwner::Person { subject: owner },
            creation_origin: AgentCreationOrigin::Manual,
            created_by: None,
            provisioning_origin: AgentOrigin::Manual,
            creation_fingerprint: "c".repeat(64),
        },
    }
}
fn rpc(method: &str, params: Value) -> Value {
    json!({"jsonrpc":"2.0","id":7,"method":method,"params":params})
}
fn proposal() -> Value {
    json!({"request_key":"missing-github-1","services":["github"],"reason":"Read the repository requested by the owner."})
}
fn call(arguments: Value) -> Value {
    rpc(
        "tools/call",
        json!({"name":"agent_key_scope_revision","arguments":arguments}),
    )
}
fn request(stub: &Stub, value: &Value) -> HttpRequest<Body> {
    HttpRequest::post("/api/v1/agent-tools")
        .header(header::AUTHORIZATION, format!("Bearer {}", stub.bearer))
        .header(header::CONTENT_TYPE, "application/json")
        .header("mcp-protocol-version", VERSION)
        .body(Body::from(value.to_string()))
        .unwrap()
}
async fn body(response: Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), BODY_LIMIT).await.unwrap()).unwrap()
}

#[tokio::test]
async fn pending_capability_discovers_tools_but_cannot_request_access() {
    let stub = Stub::new(false);
    let app = stub.router();
    let initialized = app.clone().oneshot(request(&stub, &rpc("initialize", json!({"protocolVersion":VERSION,"capabilities":{},"clientInfo":{"name":"codex","version":"1"}})))).await.unwrap();
    assert_eq!(initialized.status(), StatusCode::OK);
    assert_eq!(initialized.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(
        body(initialized).await["result"]["protocolVersion"],
        VERSION
    );
    let listed = body(
        app.clone()
            .oneshot(request(&stub, &rpc("tools/list", json!({}))))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        listed["result"]["tools"][0]["name"],
        "agent_key_scope_revision"
    );
    assert_eq!(
        listed["result"]["tools"][0]["inputSchema"]["required"],
        json!(["request_key", "services", "reason"])
    );
    let service_guidance = listed["result"]["tools"][0]["inputSchema"]["properties"]["services"]
        ["description"]
        .as_str()
        .unwrap();
    assert!(service_guidance.contains("request api-github for that access"));
    assert!(service_guidance.contains("granting api-github-pat does not configure this gh route"));
    assert!(service_guidance.contains("does not authenticate native git clone/fetch/push"));
    let notified = app
        .clone()
        .oneshot(request(
            &stub,
            &json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        ))
        .await
        .unwrap();
    assert_eq!(notified.status(), StatusCode::ACCEPTED);
    let denied = app
        .oneshot(request(&stub, &call(proposal())))
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(stub.setup.load(Ordering::SeqCst), 3);
    assert_eq!(stub.calls.load(Ordering::SeqCst), 1);
    assert!(stub.proposals.lock().unwrap().is_empty());
}

#[tokio::test]
async fn confirmed_call_passes_exact_identity_and_stable_proposal_to_admission() {
    let stub = Stub::new(true);
    let app = stub.router();
    for _ in 0..2 {
        let response = app
            .clone()
            .oneshot(request(&stub, &call(proposal())))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let response = body(response).await;
        assert_eq!(response["result"]["isError"], false);
        assert_eq!(
            response["result"]["structuredContent"]["request_id"],
            "access-request-one"
        );
        assert_eq!(response["result"]["structuredContent"]["status"], "pending");
        assert!(!response.to_string().contains(&stub.bearer));
    }
    let proposals = stub.proposals.lock().unwrap();
    assert_eq!(proposals.len(), 2);
    assert_eq!(proposals[0], proposals[1]);
    assert_eq!(proposals[0].0, requester());
    assert_eq!(proposals[0].1.request_key, "missing-github-1");
    assert_eq!(stub.setup.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn github_cli_guidance_does_not_rewrite_independent_service_requests() {
    let stub = Stub::new(true);
    let services = ["api-github-pat", "other-service"];
    let mut arguments = proposal();
    arguments["services"] = json!(services);
    let response = body(
        stub.router()
            .oneshot(request(&stub, &call(arguments)))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(response["result"]["isError"], false);
    let proposals = stub.proposals.lock().unwrap();
    assert_eq!(proposals.len(), 1);
    assert_eq!(proposals[0].1.services, services);
}

#[tokio::test]
async fn tool_validation_rejects_extra_authority_duplicate_services_and_bad_keys() {
    let stub = Stub::new(true);
    for changed in [
        json!({"request_key":"a","services":["github"],"reason":"needed","agent_id":"foreign"}),
        json!({"request_key":"a","services":["github","github"],"reason":"needed"}),
        json!({"request_key":"bad key","services":["github"],"reason":"needed"}),
        json!({"request_key":"a","services":[],"reason":"needed"}),
        json!({"request_key":"a","services":["bad/scope"],"reason":"needed"}),
        json!({"request_key":"a","services":["github"],"reason":" "}),
        json!({"request_key":"a","services":["github"],"reason":"x".repeat(2049)}),
        json!({"request_key":"a","services":["github"],"reason":"bad\u{1b}"}),
    ] {
        let response = body(
            stub.router()
                .oneshot(request(&stub, &call(changed)))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(response["error"]["code"], -32602);
    }
    assert!(stub.proposals.lock().unwrap().is_empty());
}

#[tokio::test]
async fn admission_failures_are_fixed_tool_errors_with_retry_identity_guidance() {
    let stub = Stub::new(true);
    for error in [
        AgentAccessError::Conflict,
        AgentAccessError::Unavailable,
        AgentAccessError::Forbidden,
    ] {
        *stub.failure.lock().unwrap() = Some(error.clone());
        let response = body(
            stub.router()
                .oneshot(request(&stub, &call(proposal())))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(response["result"]["isError"], true);
        assert!(!response.to_string().contains(&stub.bearer));
        if error == AgentAccessError::Unavailable {
            assert!(response["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("same request_key"));
        }
    }
}

#[tokio::test]
async fn repeated_requests_are_rate_limited_without_growing_authority() {
    let stub = Stub::new(true);
    let app = stub.router();
    for _ in 0..60 {
        assert_eq!(
            app.clone()
                .oneshot(request(&stub, &rpc("ping", json!({}))))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
    }
    let response = app
        .oneshot(request(&stub, &rpc("ping", json!({}))))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(response.headers()[header::RETRY_AFTER], "60");
    assert!(stub.proposals.lock().unwrap().is_empty());
}

#[tokio::test]
async fn failed_attempt_returns_explicit_fresh_retry_without_claiming_owner_consent() {
    let stub = Stub::new(true);
    let failed_id = format!("aar_{}", "a".repeat(64));
    *stub.returned.lock().unwrap() = Some(AgentAccessRequest {
        id: failed_id.clone(),
        agent_id: requester().agent_id,
        status: AgentAccessStatus::Failed,
        services: vec!["api-github".into()],
        reason: "Read owner repository".into(),
        retry_of: None,
        error: Some("access_update_cannot_continue".into()),
        github_approval: None,
    });
    let response = body(
        stub.router()
            .oneshot(request(&stub, &call(proposal())))
            .await
            .unwrap(),
    )
    .await;
    let value = &response["result"]["structuredContent"];
    assert_eq!(response["result"]["isError"], true);
    assert_eq!(value["request_id"], failed_id);
    assert_eq!(value["status"], "failed");
    assert_eq!(value["error"], "access_update_cannot_continue");
    assert_eq!(value["requires_owner_approval"], true);
    let mut next: AgentAccessProposal =
        serde_json::from_value(value["next_request"].clone()).unwrap();
    next.validate().unwrap();
    assert_eq!(next.retry_of, None);
    assert_eq!(value["next_request"].as_object().unwrap().len(), 3);
    assert_eq!(value["next_request_retry_of"], failed_id);
    assert_ne!(next.request_key, proposal()["request_key"]);
    assert_eq!(next.services, ["api-github"]);
    assert_eq!(next.reason, "Read owner repository");
    let again = body(
        stub.router()
            .oneshot(request(&stub, &call(proposal())))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(again["result"], response["result"]);
    *stub.returned.lock().unwrap() = None;
    next.retry_of = Some(failed_id.clone());
    let response = body(
        stub.router()
            .oneshot(request(&stub, &call(serde_json::to_value(&next).unwrap())))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(response["result"]["structuredContent"]["status"], "pending");
    assert_eq!(
        response["result"]["structuredContent"]["retry_of"],
        failed_id
    );
    assert!(response["result"]["structuredContent"]
        .get("next_request")
        .is_none());
    assert_eq!(stub.proposals.lock().unwrap().last().unwrap().1, next);
}

#[test]
fn legacy_proposals_keep_their_digest_and_terminal_denial_has_no_retry_instruction() {
    let legacy = proposal();
    let parsed: AgentAccessProposal = serde_json::from_value(legacy.clone()).unwrap();
    assert_eq!(parsed.retry_of, None);
    assert_eq!(serde_json::to_value(parsed).unwrap(), legacy);
    let mut denied = AgentAccessRequest {
        id: format!("aar_{}", "b".repeat(64)),
        agent_id: requester().agent_id,
        status: AgentAccessStatus::Denied,
        services: vec!["api-github".into()],
        reason: "Read owner repository".into(),
        retry_of: None,
        error: None,
        github_approval: None,
    };
    for status in [
        AgentAccessStatus::Denied,
        AgentAccessStatus::Pending,
        AgentAccessStatus::Applying,
        AgentAccessStatus::Succeeded,
    ] {
        denied.status = status;
        let response = super::reply::admitted(denied.clone());
        assert!(response["structuredContent"].get("next_request").is_none());
    }
    for invalid in [
        "aar_short".into(),
        format!("aar_{}", "A".repeat(64)),
        "foreign".into(),
    ] {
        let mut changed = proposal();
        changed["retry_of"] = invalid.into();
        assert!(serde_json::from_value::<AgentAccessProposal>(changed)
            .unwrap()
            .validate()
            .is_err());
    }
}
#[tokio::test]
async fn account_deletion_authentication_is_a_typed_forbidden_problem() {
    let response = auth_error(AgentAccessError::AccountDeletionInProgress);
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "application/problem+json"
    );
    let value: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(value["code"], "account_deletion_in_progress");
}
