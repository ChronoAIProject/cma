use super::*;
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Router,
};
use futures_util::FutureExt;
use layer2_core::resource_access::*;
use layer2_core::sandbox::{
    AgentOperationService, AgentReadService, AgentService, SandboxResourceReadError,
    SandboxResourceReader, SandboxResourceReading,
};
use mongodb::bson::DateTime;
use serde_json::{json, Value};
use std::{panic::AssertUnwindSafe, sync::atomic::AtomicBool, time::Duration};
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
        panic!("personal command must not request organization membership")
    }
    async fn organization_choices(
        &self,
        _: &Caller,
        _: &str,
    ) -> Result<Vec<OrganizationChoice>, MembershipError> {
        panic!("personal command must not enumerate organizations")
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
        Ok(AgentAuthorizationRevision {
            replay_principal: None,
            authority_id: "personal-owner".into(),
            policy_revision: 1,
            grant_revision: None,
        })
    }
}

struct Commands {
    inner: Arc<MongoSandboxStore>,
    unavailable: AtomicBool,
    reserves: AtomicU64,
}
#[async_trait::async_trait]
impl AgentCommandStore for Commands {
    async fn reserve_command(
        &self,
        request: CommandReservation,
    ) -> Result<CommandReservationResult, CommandLedgerError> {
        self.reserves.fetch_add(1, Ordering::SeqCst);
        self.inner.reserve_command(request).await
    }
    async fn get_admitted_command(
        &self,
        scope: &str,
        key: &str,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError> {
        if self.unavailable.load(Ordering::SeqCst) {
            return Err(CommandLedgerError::Unavailable);
        }
        self.inner.get_admitted_command(scope, key).await
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
}

struct NoResources;
#[async_trait::async_trait]
impl SandboxResourceReader for NoResources {
    async fn sandbox_resources(
        &self,
        _: &Sandbox,
    ) -> Result<SandboxResourceReading, SandboxResourceReadError> {
        panic!("command admission must not inspect a runtime")
    }
}

async fn router(client: &Client, name: &str, commands: Arc<Commands>) -> Router {
    let store = Arc::new(MongoSandboxStore::open(client, name).await.unwrap());
    let authority = Arc::new(Authority);
    let access = Arc::new(ResourceAccessService::new(store.clone(), authority.clone()));
    let service = AgentCommandService::new(commands, access, authority);
    let runtime = Arc::new(crate::sandbox::LiveAgentRegistry::default());
    layer1_public::rest::agent_router(
        layer1_public::rest::AgentRoutesState::new(
            layer1_public::agent_page::AgentPageOrigin::new("https://cma.example").unwrap(),
            AgentService::new(
                store.clone(),
                Arc::new(layer2_core::profiles::InMemoryProfileStore::default()),
                runtime.clone(),
                Default::default(),
            ),
            AgentReadService::new(store.clone(), Arc::new(NoResources)),
            AgentOperationService::new(store, runtime),
            Arc::new(
                crate::store::MongoOperationStore::open(client, name)
                    .await
                    .unwrap(),
            ),
        )
        .with_commands(service),
    )
}

async fn send(router: &Router, agent: &Agent, body: &str) -> (StatusCode, Value) {
    let caller = Caller::new(
        agent.owner.clone(),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    let mut request = Request::builder()
        .method("POST")
        .uri(format!("/api/v1/agents/{}/input", agent.id))
        .header("content-type", "application/json")
        .header("idempotency-key", "retained-command")
        .body(Body::from(body.to_owned()))
        .unwrap();
    request
        .extensions_mut()
        .insert(layer1_public::AuthenticatedSubject(caller.subject.clone()));
    request
        .extensions_mut()
        .insert(caller.authorization.clone());
    request.extensions_mut().insert(CallerContext {
        credential_source: CredentialSource::Retained,
        credential: CredentialContext {
            issuer: "https://identity.test".into(),
            subject: caller.subject.clone(),
            credential_id: "receipt-fixture".into(),
        },
        actor: ResourceActor::Human {
            subject: caller.subject.clone(),
        },
        caller,
        memberships: vec![],
    });
    request
        .extensions_mut()
        .insert(layer1_public::DelegatedCredential::new(
            "receipt-fixture".into(),
        ));
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn expired_outer_receipt_recovers_exact_command_after_lookup_outage() {
    run_case(false).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn expired_outer_receipt_cannot_recover_a_different_command() {
    run_case(true).await;
}

async fn run_case(mismatch: bool) {
    let (client, name) = database("rest_receipt_recovery").await;
    let result = tokio::time::timeout(
        Duration::from_secs(120),
        AssertUnwindSafe(exercise(&client, &name, mismatch)).catch_unwind(),
    )
    .await;
    client.database(&name).drop().await.unwrap();
    match result {
        Ok(Ok(())) => {}
        Ok(Err(panic)) => std::panic::resume_unwind(panic),
        Err(_) => panic!("REST receipt recovery timed out"),
    }
}

async fn exercise(client: &Client, name: &str, mismatch: bool) {
    let (store, agent, parent) = fixture(name, client).await;
    store
        .create_workspace(ready_workspace(
            &parent,
            agent.workspace_id.as_str(),
            "project",
        ))
        .await
        .unwrap();
    let (mut worktree, _) = worktree_and_agent(&parent.id);
    worktree.id = agent.worktree_id.clone();
    worktree.workspace_id = agent.workspace_id.clone();
    worktree.agent_id = Some(agent.id.clone());
    store.create_worktree(worktree).await.unwrap();
    let store = Arc::new(store);
    let commands = Arc::new(Commands {
        inner: store.clone(),
        unavailable: AtomicBool::new(false),
        reserves: AtomicU64::new(0),
    });
    let app = router(client, name, commands.clone()).await;
    let body = r#"{"text":"Inspect this."}"#;
    let accepted = send(&app, &agent, body).await;
    assert_eq!(accepted, (StatusCode::ACCEPTED, json!({"queued":true})));
    let original = store
        .agent_commands
        .find_one(doc! {})
        .await
        .unwrap()
        .unwrap();
    let order = store
        .agent_command_orders
        .find_one(doc! {})
        .await
        .unwrap()
        .unwrap();
    let outer = client
        .database(name)
        .collection::<Document>(crate::store::operation::OPERATIONS);
    let completed = outer.find_one(doc! {}).await.unwrap().unwrap();
    assert!(matches!(completed.get("outcome"), Some(Bson::Binary(_))));
    commands.unavailable.store(true, Ordering::SeqCst);
    assert_eq!(send(&app, &agent, body).await, accepted);
    // Make the completed receipt eligible for the production reserve/TTL cleanup.
    // The unfinished claim created by the retry is never deleted or replaced here.
    assert_eq!(
        outer
            .update_one(
                doc! {"_id": completed.get("_id").unwrap().clone()},
                doc! {"$set": {"expires_at": DateTime::from_millis(0)}},
            )
            .await
            .unwrap()
            .modified_count,
        1
    );
    let retry_body = if mismatch {
        r#"{"text":"Different command"}"#
    } else {
        body
    };
    let (status, problem) = send(&app, &agent, retry_body).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{problem}");
    assert_eq!(problem["code"], "instance_service_unavailable");
    let pending = outer.find_one(doc! {}).await.unwrap().unwrap();
    assert_ne!(pending.get("claim_id"), completed.get("claim_id"));
    commands.unavailable.store(false, Ordering::SeqCst);
    drop(app);
    let peer = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let app = router(&peer, name, commands.clone()).await;
    for expired_lease in [false, true] {
        if expired_lease {
            assert_eq!(
                outer
                    .update_one(
                        doc! {"_id": pending.get("_id").unwrap().clone()},
                        doc! {"$set": {"lease_expires_at": DateTime::from_millis(0)}},
                    )
                    .await
                    .unwrap()
                    .modified_count,
                1
            );
        }
        let recovered = send(&app, &agent, retry_body).await;
        if mismatch {
            assert_eq!(
                recovered.0,
                StatusCode::UNPROCESSABLE_ENTITY,
                "{}",
                recovered.1
            );
            assert_eq!(recovered.1["code"], "idempotency_key_reused");
        } else {
            assert_eq!(recovered, accepted);
        }
        let row = outer.find_one(doc! {}).await.unwrap().unwrap();
        assert_eq!(row.get("claim_id"), pending.get("claim_id"));
        assert_eq!(row.get("outcome"), Some(&Bson::Null));
        assert_eq!(row.get("expires_at"), Some(&Bson::Null));
    }
    if !mismatch {
        let (status, problem) = send(&app, &agent, r#"{ "text": "Inspect this." }"#).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(problem["code"], "idempotency_key_reused");
    }
    assert_eq!(commands.reserves.load(Ordering::SeqCst), 1);
    assert_eq!(
        store.agent_commands.count_documents(doc! {}).await.unwrap(),
        1
    );
    assert_eq!(
        store.agent_commands.find_one(doc! {}).await.unwrap(),
        Some(original)
    );
    assert_eq!(
        store.agent_command_orders.find_one(doc! {}).await.unwrap(),
        Some(order)
    );
}
