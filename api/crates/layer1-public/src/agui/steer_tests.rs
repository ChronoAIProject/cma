//! The public prompt boundaries share Layer 2's mid-turn capability refusal.

use std::sync::Arc;

use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    Extension, Router,
};
use layer2_core::{
    AgentConfiguration, Authorization, Baseline, Capability, Deployment, DirectoryQuery,
    EnvironmentProfile, EnvironmentProfileId, EnvironmentProfileService, EnvironmentProfileSpec,
    EnvironmentProfileStore, Event, KernelId, OperationClaim, OperationReservation, OperationStore,
    Page, PageRequest, PlatformSettingsService, RecoveryState, RegisteredUser, Reserved,
    ScreenFactory, Session, SessionCapabilities, SessionId, SessionListQuery, SessionPorts,
    SessionRecord, SessionService, SessionStatus, SessionStore, StoreError, StoredEvent, SubjectId,
    TerminalGeometry, UserDirectory, UserDirectoryStore,
};
use serde_json::{json, Value};
use tower::ServiceExt;

use crate::{
    agui,
    auth::AuthenticatedSubject,
    rest::{self, SessionsState},
    tickets::TerminalTickets,
};

const SESSION_ID: &str = "sesn_running_without_steer";

struct OneSession(SessionRecord);

#[async_trait::async_trait]
impl SessionStore for OneSession {
    async fn ping(&self) -> Result<(), StoreError> {
        Ok(())
    }

    async fn get(&self, id: &SessionId) -> Result<Option<SessionRecord>, StoreError> {
        Ok((self.0.session.id == *id).then(|| self.0.clone()))
    }

    async fn list_for(
        &self,
        owner: &SubjectId,
        _query: &SessionListQuery,
    ) -> Result<Page<SessionRecord>, StoreError> {
        Ok(Page {
            items: (&self.0.session.owner == owner)
                .then(|| self.0.clone())
                .into_iter()
                .collect(),
            next: None,
        })
    }

    async fn reserved_capacity(&self) -> Result<Reserved, StoreError> {
        Ok(Reserved::default())
    }

    async fn list_all(&self, _query: &SessionListQuery) -> Result<Page<SessionRecord>, StoreError> {
        Ok(Page {
            items: vec![self.0.clone()],
            next: None,
        })
    }

    async fn delete(&self, _id: &SessionId) -> Result<(), StoreError> {
        panic!("the refusal fixture is read-only")
    }

    async fn append_events(&self, _events: &[Event]) -> Result<(), StoreError> {
        panic!("a refused prompt must not append events")
    }

    async fn commit_session_idempotent(
        &self,
        _write_id: &str,
        _record: &SessionRecord,
        _events: &[Event],
    ) -> Result<u64, StoreError> {
        panic!("a refused prompt must not commit a session change")
    }

    async fn create_session_idempotent(
        &self,
        _write_id: &str,
        _record: &SessionRecord,
        _events: &[Event],
    ) -> Result<u64, StoreError> {
        panic!("the existing-session fixture must not create a session")
    }

    async fn events_since(
        &self,
        id: &SessionId,
        _after: u64,
    ) -> Result<Vec<StoredEvent>, StoreError> {
        assert_eq!(id.0, SESSION_ID);
        Ok(Vec::new())
    }
}

struct NoProfiles;

#[async_trait::async_trait]
impl EnvironmentProfileStore for NoProfiles {
    async fn upsert(&self, _profile: &EnvironmentProfile) -> Result<(), StoreError> {
        panic!("the refusal fixture does not author profiles")
    }

    async fn replace_content(
        &self,
        _id: &EnvironmentProfileId,
        _content: &EnvironmentProfileSpec,
        _updated_at: &str,
    ) -> Result<Option<EnvironmentProfile>, StoreError> {
        panic!("the refusal fixture does not author profiles")
    }

    async fn get(
        &self,
        _id: &EnvironmentProfileId,
    ) -> Result<Option<EnvironmentProfile>, StoreError> {
        Ok(None)
    }

    async fn list_for(
        &self,
        _owner: &SubjectId,
        _page: &PageRequest,
    ) -> Result<Page<EnvironmentProfile>, StoreError> {
        Ok(Page {
            items: Vec::new(),
            next: None,
        })
    }

    async fn set_preferred(
        &self,
        _owner: &SubjectId,
        _chosen: Option<&EnvironmentProfileId>,
    ) -> Result<(), StoreError> {
        panic!("the refusal fixture does not author profiles")
    }

    async fn preferred_for(
        &self,
        _owner: &SubjectId,
    ) -> Result<Option<EnvironmentProfile>, StoreError> {
        Ok(None)
    }

    async fn delete(&self, _id: &EnvironmentProfileId) -> Result<(), StoreError> {
        panic!("the refusal fixture does not author profiles")
    }
}

struct NoDirectory;

#[async_trait::async_trait]
impl UserDirectoryStore for NoDirectory {
    async fn record_sign_in(&self, _user: &RegisteredUser) -> Result<(), StoreError> {
        Ok(())
    }

    async fn recent(&self, _query: &DirectoryQuery) -> Result<Vec<RegisteredUser>, StoreError> {
        Ok(Vec::new())
    }

    async fn by_subjects(
        &self,
        _subjects: &[SubjectId],
    ) -> Result<Vec<RegisteredUser>, StoreError> {
        Ok(Vec::new())
    }
}

struct NoOperations;

#[async_trait::async_trait]
impl OperationStore for NoOperations {
    async fn reserve(&self, _claim: &OperationClaim) -> Result<OperationReservation, StoreError> {
        panic!("the steer fixture must refuse before reserving an operation")
    }

    async fn complete(&self, _claim: &OperationClaim, _outcome: Vec<u8>) -> Result<(), StoreError> {
        panic!("the steer fixture must refuse before completing an operation")
    }
}

fn owner() -> SubjectId {
    SubjectId("subject-steer-boundary".into())
}

fn app() -> Router {
    let owner = owner();
    let record = SessionRecord::new(Session {
        id: SessionId(SESSION_ID.into()),
        owner: owner.clone(),
        kernel: KernelId::new("test-kernel"),
        status: SessionStatus::Running,
        recovery_state: RecoveryState::Normal,
        stop_reason: None,
        options: Default::default(),
        environment_profile: None,
        geometry: TerminalGeometry::default(),
        last_seq: 0,
        runtime_version: None,
        rotations: 0,
        rotated_at: None,
        capabilities: SessionCapabilities {
            conversation: true,
            steer: false,
            ..Default::default()
        },
        granted_services: Some(Vec::new()),
        granted_skills: Some(Vec::new()),
        pricing: None,
        created_at: "2026-08-26T00:00:00Z".into(),
    });
    let settings = Arc::new(PlatformSettingsService::unconfigured());
    let environments = EnvironmentProfileService::new(
        Arc::new(NoProfiles),
        Baseline::default(),
        None,
        None,
        AgentConfiguration::default(),
        settings,
    );
    let screen_factory: ScreenFactory =
        Arc::new(|_| panic!("the capability refusal must precede runtime recovery"));
    let sessions = SessionService::new(
        Default::default(),
        screen_factory,
        Arc::new(OneSession(record)),
        SessionPorts::default(),
        Arc::clone(&environments),
        Deployment::default(),
    );
    let state = SessionsState {
        sessions,
        users: UserDirectory::new(Arc::new(NoDirectory)),
        tickets: TerminalTickets::in_memory(),
        environments,
        operations: Arc::new(NoOperations),
    };
    let authorization = Authorization::from_capabilities([Capability::UseOwnSessions]);

    agui::router(state.clone())
        .merge(rest::router(state))
        .layer(Extension(authorization))
        .layer(Extension(AuthenticatedSubject(owner)))
}

async fn assert_named_steer_problem(response: axum::response::Response) {
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE),
        Some(&header::HeaderValue::from_static(
            "application/problem+json"
        )),
        "the refusal must be rendered before an SSE response is opened"
    );
    let bytes = to_bytes(response.into_body(), 16 * 1024)
        .await
        .expect("problem documents are small");
    let problem: Value = serde_json::from_slice(&bytes).expect("the problem is JSON");
    assert_eq!(problem["code"], "capability_unsupported");
    assert_eq!(problem["capability"], "steer");
}

#[tokio::test]
async fn both_public_prompt_routes_refuse_ungranted_mid_turn_input_before_streaming() {
    for (path, body) in [
        (
            "/api/v1/agui/run",
            // An explicit runId is durably reserved so its refusal can replay.
            json!({
                "threadId": SESSION_ID,
                "messages": [{ "role": "user", "content": "redirect the turn" }]
            }),
        ),
        (
            "/api/v1/sessions/sesn_running_without_steer/input",
            json!({ "text": "redirect the turn" }),
        ),
    ] {
        let response = app()
            .oneshot(
                Request::post(path)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body.to_string()))
                    .expect("request builds"),
            )
            .await
            .expect("the router answers");
        assert_named_steer_problem(response).await;
    }
}
