use super::{agents_tests as agents, resource_access_tests, sandbox_commands_router};
use axum::{
    body::{to_bytes, Body},
    http::{Method, Request, StatusCode},
    Router,
};
use layer2_core::{
    resource_access::*,
    sandbox::{agent_io::*, control::*, store::*, FixedSandboxCredentialPrerequisite},
    Authorization, Caller, Capability, SandboxId, SubjectId,
};
use serde_json::{json, Value};
use std::{
    sync::{Arc, Mutex},
    time::SystemTime,
};
use tower::ServiceExt;

pub(super) fn context(subject: &str) -> CallerContext {
    let mut result = resource_access_tests::context(subject);
    if subject == "owner-a" {
        result.caller.authorization =
            Authorization::new([Capability::UseOwnSandboxes, Capability::DeleteOwnSandbox]);
    }
    result
}
pub(super) struct Authority(pub Mutex<Result<OrganizationRole, MembershipError>>);
#[async_trait::async_trait]
impl OrganizationAuthority for Authority {
    async fn organization_choices(
        &self,
        _: &Caller,
        _: &str,
    ) -> Result<Vec<OrganizationChoice>, MembershipError> {
        unreachable!()
    }
    async fn organization_context(
        &self,
        caller: &Caller,
        _: &str,
        organization: &SubjectId,
    ) -> Result<CallerContext, MembershipError> {
        let role = self.0.lock().unwrap().clone()?;
        let mut current = context(caller.subject.as_str());
        current.memberships.push(OrganizationMembership::new(
            current.credential.clone(),
            organization.clone(),
            role,
            SystemTime::now(),
            SystemTime::now() + MAX_MEMBERSHIP_AGE,
        )?);
        Ok(current)
    }
}
pub(super) struct Custody(pub Mutex<Result<(), CommandAuthorizationError>>);
#[async_trait::async_trait]
impl AgentCommandAdmissionAuthority for Custody {
    async fn command_admission_authority(
        &self,
        _: &CallerContext,
        _: &str,
        _: &ResourceOwner,
    ) -> Result<AgentAuthorizationRevision, CommandAuthorizationError> {
        self.0.lock().unwrap().clone()?;
        Ok(AgentAuthorizationRevision {
            authority_id: "private-sign-in-reference".into(),
            policy_revision: 1,
            grant_revision: None,
        })
    }
}
#[async_trait::async_trait]
impl SandboxControlAuthorizer for Custody {
    async fn authorize_sandbox_control(
        &self,
        access: &ResourceAccessService,
        record: &SandboxControlRecord,
    ) -> Result<Arc<SandboxControlAuthorization>, ResourceCommandAuthorizationError> {
        self.0.lock().unwrap().clone()?;
        let actor = record.envelope.actor_subject().unwrap();
        Ok(access
            .authorize_sandbox_control(&context(actor.as_str()), "request-credential", record)
            .await?)
    }
}
pub(super) struct Fixture {
    pub router: Router,
    pub store: Arc<InMemorySandboxStore>,
    pub access: Arc<ResourceAccessService>,
    pub authority: Arc<Authority>,
    pub custody: Arc<Custody>,
    pub worker: Arc<SandboxControlWorker>,
    pub service: Arc<SandboxControlService>,
    pub share: ResourceShare,
}
impl Fixture {
    pub async fn new() -> Self {
        let store = Arc::new(InMemorySandboxStore::default());
        store
            .create_sandbox(agents::sandbox("sbx_route", "owner-a"))
            .await
            .unwrap();
        let authority = Arc::new(Authority(Mutex::new(Ok(OrganizationRole::Admin))));
        let custody = Arc::new(Custody(Mutex::new(Ok(()))));
        let access = Arc::new(ResourceAccessService::new(store.clone(), authority.clone()));
        let share = access
            .change_share(
                &context("owner-a"),
                Some("request-credential"),
                ShareCommand {
                    selector: ResourceSelector::Sandbox {
                        sandbox_id: SandboxId("sbx_route".into()),
                    },
                    change: ShareChange::Create {
                        target_org_id: SubjectId::new("org"),
                        mode: ShareMode::OrgRoles,
                    },
                    expected_share_revision: None,
                    idempotency_key: "share".into(),
                },
            )
            .await
            .unwrap()
            .share;
        let service = SandboxControlService::new(store.clone(), access.clone(), custody.clone());
        let worker = SandboxControlWorker::new(
            store.clone(),
            access.clone(),
            custody.clone(),
            Arc::new(FixedSandboxCredentialPrerequisite::Ready),
        );
        let router = sandbox_commands_router(service.clone());
        Self {
            router,
            store,
            access,
            authority,
            custody,
            worker,
            service,
            share,
        }
    }
    pub async fn admit(&self, subject: &str) -> SandboxControlRecord {
        self.service
            .admit(
                &context(subject),
                "request-credential",
                SandboxControlSubmission {
                    sandbox_id: SandboxId("sbx_route".into()),
                    command: SandboxControl::Pause,
                    expected_revision: 1,
                    idempotency_key: "admitted".into(),
                },
            )
            .await
            .unwrap()
    }
    pub async fn revoke(&self) {
        self.access
            .change_share(
                &context("owner-a"),
                Some("request-credential"),
                ShareCommand {
                    selector: self.share.selector.clone(),
                    change: ShareChange::Revoke {
                        share_id: self.share.id.clone(),
                    },
                    expected_share_revision: Some(self.share.revision),
                    idempotency_key: "revoke".into(),
                },
            )
            .await
            .unwrap();
    }
}
fn request(
    method: Method,
    path: &str,
    subject: &str,
    key: Option<&str>,
    body: Option<Value>,
) -> Request<Body> {
    let mut request = agents::request(method, path, subject, key, body);
    let caller = context(subject);
    request
        .extensions_mut()
        .insert(caller.caller.authorization.clone());
    request.extensions_mut().insert(caller);
    request
        .extensions_mut()
        .insert(crate::DelegatedCredential::new("request-credential".into()));
    request
}
fn submit(subject: &str, key: Option<&str>, command: &str, revision: u64) -> Request<Body> {
    request(
        Method::POST,
        "/api/v1/sandboxes/sbx_route/commands",
        subject,
        key,
        Some(json!({"command":command,"expected_revision":revision})),
    )
}
fn safe(value: &Value) {
    let fields = value.as_object().unwrap();
    assert_eq!(fields.len(), 8);
    for field in [
        "owner",
        "actor",
        "authorization",
        "profile",
        "runtime",
        "admission_audit",
        "credential",
        "envelope",
    ] {
        assert!(!fields.contains_key(field));
    }
    assert!(!value.to_string().contains("private-sign-in-reference"));
}
#[tokio::test]
async fn sandbox_command_http_acceptance_replay_recovery_and_application_keep_safe_receipts() {
    let f = Fixture::new().await;
    let response = f
        .router
        .clone()
        .oneshot(submit("recipient", Some("retry"), "pause", 1))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    assert_eq!(response.headers()["cache-control"], "private, no-store");
    let location = response.headers()["location"].to_str().unwrap().to_string();
    let accepted: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    safe(&accepted);
    assert_eq!(accepted["state"]["status"], "queued");
    let (status, replay) =
        agents::json_response(&f.router, submit("recipient", Some("retry"), "pause", 1)).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(accepted, replay);
    let (status, recovered) = agents::json_response(
        &f.router,
        request(Method::GET, &location, "recipient", None, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(recovered, accepted);
    let id = layer2_core::ids::SandboxCommandId(accepted["command_id"].as_str().unwrap().into());
    f.worker.run_one(&id).await.unwrap();
    let (status, applied) = agents::json_response(
        &f.router,
        request(Method::GET, &location, "recipient", None, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    safe(&applied);
    assert_eq!(applied["state"]["status"], "applied");
    assert_eq!(applied["state"]["disposition"], "scheduled");
    let (status, replay) =
        agents::json_response(&f.router, submit("recipient", Some("retry"), "pause", 1)).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(replay, applied);
    let before = f
        .store
        .get_sandbox(&SandboxId("sbx_route".into()))
        .await
        .unwrap();
    f.worker.run_one(&id).await.unwrap();
    assert_eq!(
        f.store
            .get_sandbox(&SandboxId("sbx_route".into()))
            .await
            .unwrap(),
        before
    );
}
#[tokio::test]
async fn sandbox_command_http_rechecks_permission_before_replay_and_receipt_reads() {
    let f = Fixture::new().await;
    let record = f.admit("recipient").await;
    let path = format!("/api/v1/sandbox-commands/{}", record.envelope.command_id);
    assert_eq!(
        agents::json_response(
            &f.router,
            request(Method::GET, &path, "unrelated", None, None)
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    f.revoke().await;
    assert_eq!(
        agents::json_response(&f.router, submit("recipient", Some("admitted"), "pause", 1))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        agents::json_response(
            &f.router,
            request(Method::GET, &path, "recipient", None, None)
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        f.store
            .sandbox_control(&record.envelope.command_id)
            .await
            .unwrap(),
        Some(record)
    );
}
#[tokio::test]
async fn sandbox_command_http_rejects_viewers_retirement_spoofing_and_missing_custody() {
    for cause in [
        "viewer",
        "retire",
        "actor",
        "credential",
        "unknown-field",
        "missing-key",
        "stale",
    ] {
        let f = Fixture::new().await;
        let mut req = submit(
            "recipient",
            Some("invalid"),
            if cause == "retire" { "retire" } else { "pause" },
            if cause == "stale" { 2 } else { 1 },
        );
        let expected = match cause {
            "viewer" => {
                *f.authority.0.lock().unwrap() = Ok(OrganizationRole::Viewer);
                StatusCode::FORBIDDEN
            }
            "retire" => StatusCode::FORBIDDEN,
            "actor" => {
                req.extensions_mut().insert(context("owner-a"));
                StatusCode::UNAUTHORIZED
            }
            "credential" => {
                req.extensions_mut().remove::<crate::DelegatedCredential>();
                StatusCode::FORBIDDEN
            }
            "unknown-field" => {
                *req.body_mut() =
                    Body::from(r#"{"command":"pause","expected_revision":1,"owner":"owner-a"}"#);
                StatusCode::UNPROCESSABLE_ENTITY
            }
            "missing-key" => {
                req.headers_mut().remove("idempotency-key");
                StatusCode::BAD_REQUEST
            }
            "stale" => StatusCode::CONFLICT,
            _ => unreachable!(),
        };
        let (status, _) = agents::json_response(&f.router, req).await;
        assert_eq!(status, expected, "{cause}");
        assert!(f
            .store
            .queued_sandbox_controls(&PageRequest::default())
            .await
            .unwrap()
            .items
            .is_empty());
    }
}

struct Stopped;
#[async_trait::async_trait]
impl layer2_core::budget::PlatformSpendLedger for Stopped {
    async fn admission_state(
        &self,
        _: u64,
    ) -> Result<layer2_core::budget::PlatformAdmissionState, layer2_core::budget::BudgetError> {
        Ok(layer2_core::budget::PlatformAdmissionState {
            configured: true,
            operator_control: layer2_core::budget::OperatorAdmissionState::Open,
            closures: vec![layer2_core::budget::Closure {
                reason: layer2_core::budget::ClosureReason::SpendStop,
                since: 1,
                settings_revision: 1,
                committed_at_entry: 0,
            }],
        })
    }
    async fn spend_state(
        &self,
        _: u64,
    ) -> Result<layer2_core::budget::PlatformSpendState, layer2_core::budget::BudgetError> {
        unreachable!("controls read only the admission state")
    }
    async fn record_cloud_observation(
        &self,
        _: layer2_core::budget::CloudObservationInput,
        _: u64,
    ) -> Result<layer2_core::budget::PlatformSpendState, layer2_core::budget::ObservationError>
    {
        unreachable!("controls record no observation")
    }
}

#[tokio::test]
async fn sandbox_command_http_refuses_revive_under_a_platform_stop_with_402_and_no_retry() {
    let f = Fixture::new().await;
    f.service.attach_platform_spend(Arc::new(Stopped));
    let response = f
        .router
        .clone()
        .oneshot(submit("owner-a", Some("revive"), "revive", 1))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PAYMENT_REQUIRED);
    assert!(response.headers().get("retry-after").is_none());
    assert_eq!(response.headers()["cache-control"], "private, no-store");
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(body["code"], "platform_budget_exhausted");
    assert_eq!(
        body["detail"],
        layer2_core::budget::BudgetError::PlatformExhausted.guidance()
    );
    let (status, _) =
        agents::json_response(&f.router, submit("owner-a", Some("pause"), "pause", 1)).await;
    assert_eq!(status, StatusCode::ACCEPTED);
}
