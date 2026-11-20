use super::*;
use std::time::{Duration, SystemTime};

pub(super) struct SourceState {
    pub(super) rows: Vec<AgentLogEntry>,
    pub(super) metadata: AgentOutputMetadata,
    pub(super) changes: Vec<AgentOutputChange>,
}

pub(super) struct Fixture {
    pub(super) router: Router,
    pub(super) store: Arc<InMemorySandboxStore>,
    pub(super) source: Arc<Source>,
}

struct Runtime;

#[async_trait]
impl layer2_core::sandbox::AgentRuntimeRegistry for Runtime {
    async fn resolve(
        &self,
        _: &layer2_core::sandbox::AgentRuntimeTarget,
    ) -> Result<layer2_core::sandbox::AgentRuntimeConnection, layer2_core::sandbox::AgentRuntimeError>
    {
        Ok(layer2_core::sandbox::AgentRuntimeConnection {
            capabilities: layer2_core::RuntimeCapabilities::new([
                layer2_core::RuntimeCapability::Conversation,
            ]),
            input: layer2_core::sandbox::AgentRuntimeInputSender::spawn(4, |_| async { Ok(()) }),
        })
    }

    async fn fence(
        &self,
        _: &layer2_core::AgentId,
        _: u64,
    ) -> Result<(), layer2_core::sandbox::AgentRuntimeError> {
        Ok(())
    }
}

pub(super) async fn bind(f: &Fixture) {
    let now = SystemTime::now();
    let claimed = f
        .store
        .claim_next_command(CommandClaimRequest {
            agent_id: AgentId("agt_composed".into()),
            holder: "response-reader-fixture-worker".into(),
            now: layer2_core::rfc3339_at(now),
            expires_at: layer2_core::rfc3339_at(now + Duration::from_secs(30)),
        })
        .await
        .unwrap()
        .unwrap();
    let caller = Caller::new(
        SubjectId::new("owner"),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    AgentOperationService::new(f.store.clone(), Arc::new(Runtime))
        .execute_claimed_command(&caller, &claimed)
        .await
        .unwrap();
    f.store
        .release_command_claim(
            &claimed.work.lease.as_ref().unwrap().claim,
            layer2_core::now_rfc3339(),
        )
        .await
        .unwrap();

    let agent = f
        .store
        .get_agent(&AgentId("agt_composed".into()))
        .await
        .unwrap()
        .unwrap();
    let rows = f
        .store
        .agent_log(&agent.id, 0, agent.conversation.last_seq as u16)
        .await
        .unwrap();
    f.source.align_bound(&agent, rows);
}

impl Source {
    fn align_bound(&self, agent: &Agent, rows: Vec<AgentLogEntry>) {
        let mut fold = AgentLogProjectionState::default();
        let mut facts = None;
        for row in &rows {
            for change in fold
                .observe(agent, row, AgentLogProjectionState::queued_owner(row))
                .unwrap()
            {
                if let AgentLogEvidenceChange::Turn(value) = change {
                    facts = Some(*value);
                }
            }
        }
        let facts = facts.unwrap();
        let turn = agent.conversation.active_turn.unwrap();
        let start = rows.iter().find(|row| row.seq == turn).unwrap();
        let mut index = AgentTurnOutputIndex::start(start).unwrap();
        let changes = rows
            .iter()
            .filter(|row| row.seq > turn)
            .filter_map(|row| index.observe(row, AgentOutputTool::None).unwrap().change)
            .collect();
        *self.state.lock().unwrap() = SourceState {
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
        };
    }
}

#[tokio::test]
async fn semantic_counter_normal_response() {
    assert_semantic_counter(false).await;
}

#[tokio::test]
async fn semantic_counter_snapshot_failure() {
    assert_semantic_counter(true).await;
}

async fn assert_semantic_counter(failure: bool) {
    let f = super::fixture().await;
    let input = Some(json!({"conversation":"agt_composed", "input":"question", "stream":true}));
    let initial = f
        .router
        .clone()
        .oneshot(request(
            Method::POST,
            "/api/v2/responses",
            "owner",
            input.clone(),
        ))
        .await
        .unwrap();
    assert_eq!(initial.status(), StatusCode::OK);
    drop(initial);
    if failure {
        bind(&f).await;
        f.source.fail.store(true, SeqCst);
    } else {
        let command = f
            .store
            .command_work_page(&layer2_core::sandbox::store::PageRequest::default())
            .await
            .unwrap()
            .items
            .remove(0);
        f.store
            .reject_unclaimed_command(
                &command.receipt.envelope.command_id,
                command.revision,
                CommandFailureCode::AuthorityRevoked,
                layer2_core::now_rfc3339(),
            )
            .await
            .unwrap();
    }
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let metrics = recorder.handle();
    let _recording = metrics::set_default_local_recorder(&recorder);
    let app = f
        .router
        .clone()
        .layer(axum::middleware::from_fn(crate::telemetry::http::observe));
    let response = app
        .oneshot(request(Method::POST, "/api/v2/responses", "owner", input))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let wire = std::str::from_utf8(&bytes).unwrap();
    let reason = if failure {
        assert!(wire.contains("response_observation_unavailable"));
        assert!(!wire.lines().any(|line| line.starts_with("id:")));
        "dependency_error"
    } else {
        assert!(wire.contains("response.failed"));
        assert!(wire.contains("id: 1"));
        "normal_terminal"
    };
    let text = metrics.render();
    assert!(text.contains("cma_sse_contract_opened_total{stream_kind=\"responses_v2\",consumer=\"unknown\",protocol=\"responses\"} 1"), "{text}");
    assert!(text.contains(&format!("cma_sse_contract_closed_total{{stream_kind=\"responses_v2\",consumer=\"unknown\",protocol=\"responses\",reason=\"{reason}\"}} 1")), "{text}");
    assert!(text.contains(
        "cma_sse_observation_closed_total{route=\"/api/v2/responses\",reason=\"eof\"} 1"
    ));
    for private in [
        "agt_composed",
        "fixture-credential",
        "response source unavailable",
    ] {
        assert!(!text.contains(private));
    }
}
