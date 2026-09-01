//! Lifecycle refusals precede capabilities retained from an outgoing plane.

use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

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
use serde_json::Value;
use tower::ServiceExt;

use crate::{
    auth::AuthenticatedSubject,
    rest::{self, SessionsState},
    tickets::{StoredTicket, TerminalTicketStore, TerminalTickets, TicketId},
};

const SESSION_ID: &str = "sesn_rest_lifecycle_precedence";

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
        panic!("a lifecycle refusal must not delete a session")
    }

    async fn append_events(&self, _events: &[Event]) -> Result<(), StoreError> {
        panic!("a lifecycle refusal must not append events")
    }

    async fn commit_session_idempotent(
        &self,
        _write_id: &str,
        _record: &SessionRecord,
        _events: &[Event],
    ) -> Result<u64, StoreError> {
        panic!("a lifecycle refusal must not commit a session change")
    }

    async fn create_session_idempotent(
        &self,
        _write_id: &str,
        _record: &SessionRecord,
        _events: &[Event],
    ) -> Result<u64, StoreError> {
        panic!("a lifecycle refusal must not create a session")
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
        panic!("the lifecycle fixture does not author profiles")
    }

    async fn replace_content(
        &self,
        _id: &EnvironmentProfileId,
        _content: &EnvironmentProfileSpec,
        _updated_at: &str,
    ) -> Result<Option<EnvironmentProfile>, StoreError> {
        panic!("the lifecycle fixture does not author profiles")
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
        panic!("the lifecycle fixture does not author profiles")
    }

    async fn preferred_for(
        &self,
        _owner: &SubjectId,
    ) -> Result<Option<EnvironmentProfile>, StoreError> {
        Ok(None)
    }

    async fn delete(&self, _id: &EnvironmentProfileId) -> Result<(), StoreError> {
        panic!("the lifecycle fixture does not author profiles")
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
        panic!("these lifecycle tests do not use idempotent operations")
    }

    async fn complete(&self, _claim: &OperationClaim, _outcome: Vec<u8>) -> Result<(), StoreError> {
        panic!("these lifecycle tests do not use idempotent operations")
    }
}

#[derive(Default)]
struct CountingTicketStore {
    puts: AtomicUsize,
}

#[async_trait::async_trait]
impl TerminalTicketStore for CountingTicketStore {
    async fn put(&self, _ticket: StoredTicket) -> Result<(), StoreError> {
        self.puts.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn take(&self, _id: &TicketId) -> Result<Option<StoredTicket>, StoreError> {
        panic!("these lifecycle tests do not redeem tickets")
    }
}

fn owner() -> SubjectId {
    SubjectId("subject-rest-lifecycle".into())
}

fn session(status: SessionStatus, capabilities: SessionCapabilities) -> SessionRecord {
    SessionRecord::new(Session {
        id: SessionId(SESSION_ID.into()),
        owner: owner(),
        kernel: KernelId::new("test-kernel"),
        status,
        recovery_state: RecoveryState::Normal,
        stop_reason: None,
        options: Default::default(),
        environment_profile: None,
        geometry: TerminalGeometry::default(),
        last_seq: 0,
        runtime_version: None,
        rotations: 0,
        rotated_at: None,
        capabilities,
        granted_services: Some(Vec::new()),
        granted_skills: Some(Vec::new()),
        pricing: None,
        chat: None,
        created_at: "2026-08-27T00:00:00Z".into(),
    })
}

fn app(record: SessionRecord, ticket_store: Arc<CountingTicketStore>) -> Router {
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
        Arc::new(|_| panic!("lifecycle refusal must precede runtime recovery"));
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
        tickets: TerminalTickets::new(ticket_store),
        environments,
        operations: Arc::new(NoOperations),
        triggers: None,
    };
    let authorization = Authorization::from_capabilities([Capability::UseOwnSessions]);

    rest::router(state)
        .layer(Extension(authorization))
        .layer(Extension(AuthenticatedSubject(owner())))
}

async fn assert_problem(response: axum::response::Response, code: &str) {
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE),
        Some(&header::HeaderValue::from_static(
            "application/problem+json"
        ))
    );
    let bytes = to_bytes(response.into_body(), 16 * 1024)
        .await
        .expect("problem documents are small");
    let problem: Value = serde_json::from_slice(&bytes).expect("the problem is JSON");
    assert_eq!(problem["code"], code);
}

async fn post(app: Router, path: &str, body: &'static str) -> axum::response::Response {
    app.oneshot(
        Request::post(path)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body))
            .expect("request builds"),
    )
    .await
    .expect("the REST router answers")
}

#[tokio::test]
async fn steer_preserves_rotation_and_termination_before_the_steer_grant() {
    for (status, code) in [
        (SessionStatus::Rotating, "session_rotating"),
        (SessionStatus::Terminated, "session_terminated"),
    ] {
        let tickets = Arc::new(CountingTicketStore::default());
        let response = post(
            app(
                session(
                    status,
                    SessionCapabilities {
                        conversation: true,
                        steer: false,
                        ..Default::default()
                    },
                ),
                tickets,
            ),
            "/api/v1/sessions/sesn_rest_lifecycle_precedence/steer",
            r#"{"text":"redirect the turn"}"#,
        )
        .await;
        assert_problem(response, code).await;
    }
}

#[tokio::test]
async fn terminal_ticket_refuses_lifecycle_before_both_plane_grants_without_issuing() {
    for (status, terminal, code) in [
        (SessionStatus::Rotating, true, "session_rotating"),
        (SessionStatus::Rotating, false, "session_rotating"),
        (SessionStatus::Terminated, false, "session_terminated"),
    ] {
        let ticket_store = Arc::new(CountingTicketStore::default());
        let response = post(
            app(
                session(
                    status,
                    SessionCapabilities {
                        terminal,
                        conversation: !terminal,
                        ..Default::default()
                    },
                ),
                Arc::clone(&ticket_store),
            ),
            "/api/v1/sessions/sesn_rest_lifecycle_precedence/terminal-ticket",
            "{}",
        )
        .await;
        assert_problem(response, code).await;
        assert_eq!(
            ticket_store.puts.load(Ordering::SeqCst),
            0,
            "a refused lifecycle must not mint a terminal ticket"
        );
    }
}
