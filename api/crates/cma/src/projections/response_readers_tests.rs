//! Exercise the production projection factory through public response callers.
use super::*;
use crate::{auth::test_resources, sandbox::LiveAgentRegistry};
use async_trait::async_trait;
use axum::{
    body::Body,
    http::{Method, Request, StatusCode},
};
use futures_util::StreamExt;
use layer2_core::{
    profiles::InMemoryProfileStore,
    resource_access::*,
    sandbox::{
        agent_io::*,
        agent_output_index::*,
        store::{
            indexed::*, AgentLogStore, AgentResponseLogStore, AgentResponseLogWindow, AgentStore,
            SandboxStore, SandboxStoreError, WorkspaceStore, WorktreeStore,
        },
        *,
    },
    AgentId, Authorization, Caller, Capability, PlatformDefaults, SubjectId,
};
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst},
    Mutex,
};
use tower::ServiceExt;

#[path = "response_readers_fixture.rs"]
mod fixture_support;
use fixture_support::{request, Fixture};

#[path = "sse_observation_tests.rs"]
mod sse_observation_tests;

const AT: &str = "2026-09-05T00:00:00Z";

struct Defaults;
#[async_trait]
impl AgentContextDefaults for Defaults {
    async fn defaults(&self) -> Result<PlatformDefaults, AgentContextError> {
        panic!("response reads cannot create resources")
    }
}
struct Authority;
#[async_trait]
impl OrganizationAuthority for Authority {
    async fn organization_context(
        &self,
        _: &Caller,
        _: &str,
        _: &SubjectId,
    ) -> Result<CallerContext, MembershipError> {
        Err(MembershipError::NotMember)
    }
    async fn organization_choices(
        &self,
        _: &Caller,
        _: &str,
    ) -> Result<Vec<OrganizationChoice>, MembershipError> {
        Ok(vec![])
    }
}
#[async_trait]
impl AgentCommandAdmissionAuthority for Authority {
    async fn command_admission_authority(
        &self,
        context: &CallerContext,
        _: &str,
        owner: &AgentOwner,
    ) -> Result<AgentAuthorizationRevision, CommandAuthorizationError> {
        if owner
            != &(AgentOwner::Person {
                subject: context.caller.subject.clone(),
            })
        {
            return Err(CommandAuthorizationError::Revoked);
        }
        Ok(AgentAuthorizationRevision {
            replay_principal: None,
            authority_id: "composed-reader".into(),
            policy_revision: 1,
            grant_revision: None,
        })
    }
}

struct Source {
    state: Mutex<fixture_support::SourceState>,
    opens: AtomicUsize,
    fail: AtomicBool,
}
struct Reader<'a> {
    source: &'a Source,
    metadata: AgentOutputMetadata,
}
#[async_trait]
impl AgentResponseLogStore for Source {
    async fn agent_output_available(&self, _: &Agent) -> Result<bool, SandboxStoreError> {
        Ok(true)
    }
    async fn agent_output_reader(
        &self,
        agent: &Agent,
        turn: Option<u64>,
    ) -> Result<Option<Box<dyn AgentOutputReader + '_>>, SandboxStoreError> {
        self.opens.fetch_add(1, SeqCst);
        if self.fail.load(SeqCst) {
            return Err(SandboxStoreError::Unavailable(
                "response source unavailable".into(),
            ));
        }
        let mut metadata = self.state.lock().unwrap().metadata.clone();
        metadata.through = agent.conversation.last_seq;
        metadata.active_turn = agent.conversation.active_turn;
        if turn.is_none() {
            metadata.turn = None;
            metadata.index = None;
        }
        Ok(Some(Box::new(Reader {
            source: self,
            metadata,
        })))
    }
    async fn agent_response_log_window(
        &self,
        _: &Agent,
        _: Option<u64>,
    ) -> Result<Option<AgentResponseLogWindow>, SandboxStoreError> {
        panic!("adopted output must not use the legacy window")
    }
}
#[async_trait]
impl AgentOutputReader for Reader<'_> {
    fn metadata(&self) -> &AgentOutputMetadata {
        &self.metadata
    }
    async fn changes(
        &mut self,
        from: u64,
        limit: u16,
    ) -> Result<Vec<AgentOutputChange>, SandboxStoreError> {
        Ok(self
            .source
            .state
            .lock()
            .unwrap()
            .changes
            .iter()
            .skip(from as usize)
            .take(usize::from(limit))
            .cloned()
            .collect())
    }
    async fn tool(&mut self, _: &str) -> Result<AgentToolOutputState, SandboxStoreError> {
        Err(SandboxStoreError::NotFound)
    }
    async fn points(&mut self, sequences: &[u64]) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
        sequences
            .iter()
            .map(|seq| {
                self.source
                    .state
                    .lock()
                    .unwrap()
                    .rows
                    .iter()
                    .find(|row| row.seq == *seq)
                    .cloned()
                    .ok_or(SandboxStoreError::NotFound)
            })
            .collect()
    }
    async fn source_page(
        &mut self,
        after: u64,
        limit: u16,
    ) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
        Ok(self
            .source
            .state
            .lock()
            .unwrap()
            .rows
            .iter()
            .filter(|row| row.seq > after)
            .take(usize::from(limit))
            .cloned()
            .collect())
    }
}

async fn fixture() -> Fixture {
    fixture_with_terminal(Some(AgentLogKind::TurnCompleted { turn_sequence: 3 })).await
}

async fn fixture_with_terminal(terminal: Option<AgentLogKind>) -> Fixture {
    let store = Arc::new(InMemorySandboxStore::default());
    let profiles = Arc::new(InMemoryProfileStore::default());
    let sandbox = test_resources::sandbox("sbx_composed", "owner");
    let workspace = test_resources::workspace("wks_composed", sandbox.id.as_str(), "owner");
    let mut agent = test_resources::agent(
        "agt_composed",
        "wt_composed",
        workspace.id.as_str(),
        sandbox.id.as_str(),
        "owner",
        if terminal.is_some() {
            AgentStatus::Ready
        } else {
            AgentStatus::Working
        },
    );
    agent.conversation.conversation_id = Some("composed-conversation".into());
    agent.conversation.runtime_generation = Some(1);
    // A retained Agent already passed readiness before its first turn.
    agent.ready_at = Some(agent.created_at.clone());
    agent.conversation.active_turn = terminal.is_none().then_some(3);
    agent.conversation.last_seq = 5 + u64::from(terminal.is_some());
    let worktree = test_resources::worktree(
        agent.worktree_id.as_str(),
        workspace.id.as_str(),
        sandbox.id.as_str(),
        agent.id.as_str(),
        "owner",
    );
    let kinds = [
        AgentLogKind::Created {
            workspace_id: workspace.id.clone(),
            worktree_id: worktree.id.clone(),
        },
        AgentLogKind::ConversationOpened {
            conversation_id: "composed-conversation".into(),
            runtime_generation: 1,
        },
        AgentLogKind::TurnStarted {
            turn_sequence: 3,
            origin_key: None,
            model: Some("model-a".into()),
        },
        AgentLogKind::OwnerMessage {
            content: "question".into(),
        },
        AgentLogKind::AgentMessage {
            content: "retained answer".into(),
        },
    ];
    let rows: Vec<_> = kinds
        .into_iter()
        .chain(terminal)
        .enumerate()
        .map(|(i, kind)| AgentLogEntry {
            agent_id: agent.id.clone(),
            seq: i as u64 + 1,
            appended_at: AT.into(),
            kind,
        })
        .collect();
    store.create_sandbox(sandbox).await.unwrap();
    store.create_workspace(workspace).await.unwrap();
    store.create_worktree(worktree).await.unwrap();
    store.create_agent(agent.clone()).await.unwrap();
    for row in &rows {
        store
            .append_agent_log(&agent.id, AT.into(), row.kind.clone())
            .await
            .unwrap();
    }
    let mut fold = AgentLogProjectionState::default();
    let mut facts = None;
    for row in &rows {
        for change in fold.observe(&agent, row, None).unwrap() {
            if let AgentLogEvidenceChange::Turn(value) = change {
                facts = Some(*value);
            }
        }
    }
    let facts = facts.unwrap();
    let mut index = AgentTurnOutputIndex::start(&rows[2]).unwrap();
    let changes = rows[3..]
        .iter()
        .filter_map(|row| index.observe(row, AgentOutputTool::None).unwrap().change)
        .collect();
    let source = Arc::new(Source {
        state: Mutex::new(fixture_support::SourceState {
            metadata: AgentOutputMetadata {
                agent_id: agent.id.clone(),
                through: agent.conversation.last_seq,
                created_at: agent.created_at.clone(),
                runtime_generation: agent.conversation.runtime_generation,
                active_turn: agent.conversation.active_turn,
                initial_model: agent.profile.content.model.clone(),
                index: Some(index),
                turn: Some(AgentOutputTurn {
                    legacy_model_known: facts.legacy_model_known,
                    usage_sequence: facts.response_usage_sequence,
                    terminal_sequence: facts.end_sequence,
                    prewrite_sequence: facts.prewrite_failure_sequence,
                    facts,
                }),
            },
            rows,
            changes,
        }),
        opens: AtomicUsize::new(0),
        fail: AtomicBool::new(false),
    });
    let authority = Arc::new(Authority);
    let access = Arc::new(ResourceAccessService::new(store.clone(), authority.clone()));
    let commands = AgentCommandService::new(store.clone(), access.clone(), authority);
    let services = AgentProjectionServices::new(
        layer1_public::agent_page::AgentPageOrigin::new("https://cma.example").unwrap(),
        store.clone(),
        profiles.clone(),
        Arc::new(LiveAgentRegistry::default()),
        Arc::new(UnavailableSandboxResources),
        layer2_core::RuntimeCapabilities::new([layer2_core::RuntimeCapability::Conversation]),
    )
    .with_resource_access(access)
    .with_response_logs(Some(source.clone()))
    .with_commands(commands.clone());
    let context = AgentContextService::new(
        SandboxService::new(
            store.clone(),
            profiles.clone(),
            SandboxPolicy::default(),
            Arc::new(FixedSandboxCredentialPrerequisite::Ready),
        ),
        WorkspaceService::new(store.clone(), profiles.clone()),
        services.agents(),
        profiles,
        Arc::new(Defaults),
    );
    // Use the production factory: the legacy route and v2 stream both receive
    // the same configured service. No rollout switch is enabled in this fixture.
    let router = services
        .conversation_router(
            context,
            DurableAgentUpdateRecorder::new(store.clone()),
            "https://cma.example",
        )
        .merge(layer1_public::responses::v2::router(
            layer1_public::responses::v2::ResponsesV2State::new(
                layer1_public::agent_page::AgentPageOrigin::new("https://cma.example").unwrap(),
                commands,
                services.reads(),
            ),
        ));
    Fixture {
        router,
        store,
        source,
    }
}

async fn get(f: &Fixture, subject: &str) -> (StatusCode, Value) {
    let response = f
        .router
        .clone()
        .oneshot(request(
            Method::GET,
            "/api/v1/responses/resp_agt_composed_3",
            subject,
            None,
        ))
        .await
        .unwrap();
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    (status, serde_json::from_slice(&body).unwrap())
}

#[tokio::test]
async fn composed_legacy_route_uses_retained_reader_and_fails_closed_before_fallback() {
    let f = fixture().await;
    let (status, body) = get(&f, "owner").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["output"][0]["content"][0]["text"], "retained answer");
    assert!(f.source.opens.load(SeqCst) > 0);
    let before = f.source.opens.load(SeqCst);
    assert_eq!(get(&f, "foreign").await.0, StatusCode::NOT_FOUND);
    assert_eq!(f.source.opens.load(SeqCst), before);
    f.source.fail.store(true, SeqCst);
    assert_eq!(get(&f, "owner").await.0, StatusCode::SERVICE_UNAVAILABLE);
    assert!(f.source.opens.load(SeqCst) > before);
}

#[tokio::test]
async fn composed_v2_create_stream_keeps_the_shared_reader_with_paging_disabled() {
    let f = fixture().await;
    let response = f
        .router
        .clone()
        .oneshot(request(
            Method::POST,
            "/api/v2/responses",
            "owner",
            Some(json!({"conversation":"agt_composed", "input":"question", "stream":true})),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut frames = response.into_body().into_data_stream();
    let first = tokio::time::timeout(std::time::Duration::from_secs(2), frames.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let text = std::str::from_utf8(&first).unwrap();
    assert!(text.contains("response.created"), "{text}");
    assert_eq!(f.source.opens.load(SeqCst), 0);
    drop(frames);

    fixture_support::bind(&f).await;
    let response = f
        .router
        .clone()
        .oneshot(request(
            Method::POST,
            "/api/v2/responses",
            "owner",
            Some(json!({"conversation":"agt_composed", "input":"question", "stream":true})),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut frames = response.into_body().into_data_stream();
    let bound_first = tokio::time::timeout(std::time::Duration::from_secs(2), frames.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let bound_text = std::str::from_utf8(&bound_first).unwrap();
    assert!(bound_text.contains("response.created"), "{bound_text}");
    assert!(
        !bound_text.contains("response_observation_unavailable"),
        "{bound_text}"
    );
    let opens_before_failure = f.source.opens.load(SeqCst);
    assert!(opens_before_failure > 0);
    drop(frames);
    f.source.fail.store(true, SeqCst);
    let response = f
        .router
        .clone()
        .oneshot(request(
            Method::POST,
            "/api/v2/responses",
            "owner",
            Some(json!({"conversation":"agt_composed", "input":"question", "stream":true})),
        ))
        .await
        .unwrap();
    // Source work begins after headers. An unavailable reader must produce
    // a typed terminal stream error, never successful legacy fallback frames.
    assert_eq!(response.status(), StatusCode::OK);
    let mut frames = response.into_body().into_data_stream();
    let error = tokio::time::timeout(std::time::Duration::from_secs(2), frames.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let text = std::str::from_utf8(&error).unwrap();
    assert!(text.contains("response_observation_unavailable"), "{text}");
    assert!(!text.contains("response.created"), "{text}");
    assert!(f.source.opens.load(SeqCst) > opens_before_failure);
    assert!(frames.next().await.is_none());
}
