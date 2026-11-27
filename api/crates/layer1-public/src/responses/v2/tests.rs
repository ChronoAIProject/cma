use layer2_core::{sandbox::agent_io::AgentCommand, AgentId};
use serde_json::json;

#[path = "page_tests.rs"]
mod page_tests;
#[path = "replay_tests.rs"]
mod replay_tests;
#[path = "shared_tests.rs"]
mod shared_tests;
#[path = "submission_tests.rs"]
mod submission_tests;

#[path = "telemetry_tests.rs"]
mod telemetry_tests;

#[path = "startup_failure_tests.rs"]
mod startup_failure_tests;

#[test]
fn conversation_only_forms_canonicalize_and_aliases_select_the_same_command() {
    for input in [
        json!("Hello world"),
        json!([{"role":"user","content":"Hello world"}]),
        json!([{"type":"message","role":"user","content":[{"type":"input_text","text":"Hello "},{"type":"input_text","text":"world"}]}]),
    ] {
        let parsed = super::parse::parse(
            json!({"conversation":"agt_fixture","stream":true,"input":input}),
            None,
            "same-key".into(),
        )
        .unwrap();
        let alias = super::parse::parse(
            json!({"stream":true,"input":"Hello world"}),
            Some(AgentId("agt_fixture".into())),
            "same-key".into(),
        )
        .unwrap();
        assert_eq!(parsed.agent_id, alias.agent_id);
        assert_eq!(parsed.command, AgentCommand::message("Hello world"));
        assert_eq!(parsed.command, alias.command);
    }
}

use crate::rest::agents_tests as fixtures;
use async_trait::async_trait;
use axum::{
    body::Body,
    http::{Method, Request, StatusCode},
    Router,
};
use futures_util::StreamExt;
use layer2_core::{
    resource_access::*,
    sandbox::{
        agent_io::*,
        store::{AgentStore, InMemorySandboxStore, PageRequest},
        AgentStatus,
    },
    Authorization, Caller, Capability, SubjectId,
};
use serde_json::Value;
use std::{
    sync::Arc,
    time::{Duration, SystemTime},
};
use tower::ServiceExt;

struct Authority;
pub(crate) fn context(subject: &str) -> CallerContext {
    CallerContext {
        credential_source: layer2_core::resource_access::CredentialSource::Retained,
        caller: Caller::new(
            SubjectId::new(subject),
            Authorization::new([Capability::UseOwnSandboxes]),
        ),
        actor: ResourceActor::Api {
            subject: SubjectId::new(subject),
            delegation_id: None,
        },
        credential: CredentialContext {
            issuer: "https://fixture.test".into(),
            subject: SubjectId::new(subject),
            credential_id: "exact-fixture".into(),
        },
        memberships: vec![],
    }
}
#[async_trait]
impl AgentCommandAdmissionAuthority for Authority {
    async fn command_admission_authority(
        &self,
        context: &CallerContext,
        credential: &str,
        owner: &AgentOwner,
    ) -> Result<AgentAuthorizationRevision, CommandAuthorizationError> {
        if credential != "fixture-credential"
            || context.caller.subject != SubjectId::new("owner-a")
            || owner
                != &(AgentOwner::Person {
                    subject: context.caller.subject.clone(),
                })
        {
            return Err(CommandAuthorizationError::Revoked);
        }
        Ok(AgentAuthorizationRevision {
            replay_principal: None,
            authority_id: "fixture-custody".into(),
            policy_revision: 1,
            grant_revision: None,
        })
    }
}
#[async_trait]
impl OrganizationAuthority for Authority {
    async fn organization_context(
        &self,
        caller: &Caller,
        _: &str,
        org: &SubjectId,
    ) -> Result<CallerContext, MembershipError> {
        if org != &SubjectId::new("org") || !matches!(caller.subject.as_str(), "owner-a" | "reader")
        {
            return Err(MembershipError::NotMember);
        }
        let mut value = context(caller.subject.as_str());
        let now = SystemTime::now();
        value.memberships.push(OrganizationMembership::new(
            value.credential.clone(),
            org.clone(),
            OrganizationRole::Viewer,
            now,
            now + Duration::from_secs(60),
        )?);
        Ok(value)
    }
    async fn organization_choices(
        &self,
        _: &Caller,
        _: &str,
    ) -> Result<Vec<OrganizationChoice>, MembershipError> {
        Ok(vec![])
    }
}
pub(crate) struct Fixture {
    pub(crate) router: Router,
    pub(crate) store: Arc<InMemorySandboxStore>,
    pub(crate) access: Arc<ResourceAccessService>,
}
pub(crate) async fn fixture() -> Fixture {
    fixture_with_events(None, false).await
}
async fn fixture_with_events(
    reader: Option<Arc<dyn layer2_core::sandbox::store::AgentResponseLogStore>>,
    paged_events: bool,
) -> Fixture {
    fixture_mode(reader, paged_events, false).await
}
async fn fixture_mode(
    reader: Option<Arc<dyn layer2_core::sandbox::store::AgentResponseLogStore>>,
    paged_events: bool,
    shared: bool,
) -> Fixture {
    fixture_with_events_and_standing(
        reader,
        paged_events,
        shared,
        Arc::new(layer2_core::account_deletion::AvailableAccounts),
    )
    .await
    .0
}
pub(crate) async fn fixture_with_standing(
    standing: Arc<dyn layer2_core::account_deletion::AccountStanding>,
) -> Fixture {
    fixture_with_events_and_standing(None, false, false, standing)
        .await
        .0
}
async fn fixture_with_events_and_standing(
    reader: Option<Arc<dyn layer2_core::sandbox::store::AgentResponseLogStore>>,
    paged_events: bool,
    shared: bool,
    standing: Arc<dyn layer2_core::account_deletion::AccountStanding>,
) -> (
    Fixture,
    Option<(
        Arc<layer2_core::sandbox::agent_observation::AgentObservationAuthority>,
        Arc<shared_tests::Facts>,
    )>,
) {
    let (state, store) = fixtures::conversation_fixture().await;
    fixtures::insert_agent(
        &store,
        fixtures::seeded_agent("agt_fixture", AgentStatus::Ready, 2, None),
        vec![],
    )
    .await;
    let authority = Arc::new(Authority);
    let access = Arc::new(
        ResourceAccessService::new(store.clone(), authority.clone())
            .with_account_standing(standing),
    );
    let commands = AgentCommandService::new(store.clone(), access.clone(), authority);
    let reads = state
        .reads
        .with_resource_access(access.clone())
        .with_response_logs(reader);
    let (reads, observer) = if shared {
        let agent = store
            .get_agent(&AgentId("agt_fixture".into()))
            .await
            .unwrap()
            .unwrap();
        let (reads, custody, facts) = shared_tests::install(reads, store.clone(), &agent);
        (reads, Some((custody, facts)))
    } else {
        (reads, None)
    };
    let router = super::router(
        super::ResponsesV2State::new(crate::agent_page::AgentPageOrigin::test(), commands, reads)
            .with_paged_events(paged_events),
    );
    let router = match observer.as_ref().map(|(custody, _)| custody.clone()) {
        Some(custody) => router.layer(axum::middleware::from_fn(
            move |request: axum::extract::Request, next: axum::middleware::Next| {
                let custody = custody.clone();
                async move {
                    layer2_core::sandbox::agent_observation::observation_scope(
                        custody,
                        next.run(request),
                    )
                    .await
                }
            },
        )),
        None => router,
    };
    (
        Fixture {
            router,
            store,
            access,
        },
        observer,
    )
}
fn request(
    method: Method,
    path: &str,
    subject: &str,
    key: Option<&str>,
    body: Option<Value>,
) -> Request<Body> {
    let mut request = fixtures::request(method, path, subject, key, body);
    request.extensions_mut().insert(context(subject));
    request
        .extensions_mut()
        .insert(crate::DelegatedCredential::new("fixture-credential".into()));
    request
}
async fn read_json(
    router: &Router,
    method: Method,
    path: &str,
    subject: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    fixtures::json_response(
        router,
        request(method, path, subject, Some("same-key"), body),
    )
    .await
}
async fn create(f: &Fixture, alias: bool) -> axum::response::Response {
    let path = if alias {
        "/api/v2/agents/agt_fixture/responses"
    } else {
        "/api/v2/responses"
    };
    f.router.clone().oneshot(request(Method::POST, path, "owner-a", Some("same-key"), Some(json!({"conversation":"agt_fixture","input":"Hello","stream":true,"metadata":{"private":"owner metadata"}})))).await.unwrap()
}
fn events(bytes: &[u8]) -> Vec<Value> {
    std::str::from_utf8(bytes)
        .unwrap()
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[path = "indexed_tests.rs"]
mod indexed_tests;
