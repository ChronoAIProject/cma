use super::*;
use crate::stream_telemetry::{isolated_test, snapshot, BodyEnd, CloseReason, Contract, Kind};
#[tokio::test]
async fn responses_initial_output_and_read_failure_have_exact_counts() {
    isolated_test(async {
        let (state, agent) = completed().await;
        let caller = layer2_core::Caller::new(
            layer2_core::SubjectId("owner-a".into()),
            layer2_core::Authorization::new([layer2_core::Capability::UseOwnSandboxes]),
        );
        let stream = super::super::stream::open(state, caller.clone(), agent, 3, None);
        assert_eq!(snapshot(Kind::Responses), (1, [0; 3]));
        let evidence = stream.extensions().get::<Contract>().unwrap().clone();
        let bytes = to_bytes(stream.into_body(), 1024 * 1024).await.unwrap();
        let text = std::str::from_utf8(&bytes).unwrap();
        assert!(text.contains("response.completed"));
        assert_eq!(
            evidence.close_reason(BodyEnd::Eof),
            CloseReason::NormalTerminal
        );
        assert_eq!(snapshot(Kind::Responses), (0, [0, 1, 0]));
        let (state, _) = conversation_fixture().await;
        let agent = seeded_agent("agt_unreadable", AgentStatus::Working, 10, Some(3));
        let stream = super::super::stream::open(state, caller, agent, 3, None);
        assert_eq!(snapshot(Kind::Responses).0, 1);
        let evidence = stream.extensions().get::<Contract>().unwrap().clone();
        let bytes = to_bytes(stream.into_body(), 65536).await.unwrap();
        assert!(bytes.is_empty());
        assert_eq!(
            evidence.close_reason(BodyEnd::Eof),
            CloseReason::AuthorizationUnavailable
        );
        assert_eq!(snapshot(Kind::Responses), (0, [0, 1, 1]));
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn responses_refresh_counts_quiet_output_and_failure_in_both_read_paths() {
    use futures_util::StreamExt;
    use layer2_core::sandbox::store::AgentStore;
    use std::time::Duration;

    for authorized in [false, true] {
        isolated_test(async {
            let (state, store) = crate::rest::agents_tests::authorized_conversation_fixture().await;
            let mut agent = seeded_agent("agt_telemetry", AgentStatus::Working, 4, Some(3));
            insert_agent(
                &store,
                agent.clone(),
                vec![
                    AgentLogKind::TurnStarted {
                        model: None,
                        turn_sequence: 3,
                        origin_key: None,
                    },
                    AgentLogKind::OwnerMessage {
                        content: "test input".into(),
                    },
                ],
            )
            .await;
            let context = crate::agent_v2::fixtures::context("owner-a");
            let response = if authorized {
                super::super::stream::open_authorized(
                    state,
                    crate::AuthenticatedResourceCaller {
                        context,
                        credential: None,
                    },
                    agent.clone(),
                    3,
                    None,
                )
                .await
            } else {
                super::super::stream::open(state, context.caller, agent.clone(), 3, None)
            };
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(snapshot(Kind::Responses), (1, [0; 3]));
            let evidence = response.extensions().get::<Contract>().unwrap().clone();
            let mut body = response.into_body().into_data_stream();
            for event in ["response.created", "response.in_progress"] {
                let frame = body.next().await.unwrap().unwrap();
                assert!(std::str::from_utf8(&frame).unwrap().contains(event));
                assert_eq!(snapshot(Kind::Responses), (1, [0, 1, 0]));
            }
            // Unlike the other streams, v1 reads immediately before sleeping:
            // quiet polls occur at 0, 100, 200 and 300 ms.
            assert!(
                tokio::time::timeout(Duration::from_millis(350), body.next())
                    .await
                    .is_err()
            );
            assert_eq!(snapshot(Kind::Responses), (1, [4, 1, 0]));
            let revision = agent.revision;
            agent.revision += 1;
            agent.conversation.last_seq += 1; // absent row: refresh must fail closed
            store.save_agent(revision, agent).await.unwrap();
            assert!(body.next().await.is_none());
            assert_eq!(
                evidence.close_reason(BodyEnd::Eof),
                CloseReason::ServerError
            );
            assert_eq!(snapshot(Kind::Responses), (0, [4, 1, 1]));
        })
        .await;
    }
}

#[tokio::test(start_paused = true)]
async fn responses_without_a_projectable_turn_count_quiet_polls_and_drop() {
    use futures_util::StreamExt;
    use std::time::Duration;

    isolated_test(async {
        let (state, store) = conversation_fixture().await;
        let agent = seeded_agent("agt_telemetry", AgentStatus::Working, 4, Some(3));
        insert_agent(
            &store,
            agent.clone(),
            vec![
                AgentLogKind::TurnStarted {
                    model: None,
                    turn_sequence: 3,
                    origin_key: None,
                },
                AgentLogKind::OwnerMessage {
                    content: "test input".into(),
                },
            ],
        )
        .await;
        let caller = crate::agent_v2::fixtures::context("owner-a").caller;
        let response = super::super::stream::open(state, caller, agent, 5, None);
        assert_eq!(snapshot(Kind::Responses), (1, [0; 3]));
        let evidence = response.extensions().get::<Contract>().unwrap().clone();
        let mut body = response.into_body().into_data_stream();
        assert!(
            tokio::time::timeout(Duration::from_millis(350), body.next())
                .await
                .is_err()
        );
        assert_eq!(snapshot(Kind::Responses), (1, [4, 0, 0]));
        drop(body);
        assert_eq!(
            evidence.close_reason(BodyEnd::Dropped),
            CloseReason::DropUnknown
        );
        assert_eq!(snapshot(Kind::Responses), (0, [4, 0, 0]));
    })
    .await;
}

#[tokio::test]
async fn buffered_terminal_cannot_hide_lost_authority() {
    use futures_util::StreamExt;
    use layer2_core::sandbox::store::{SandboxStore, SandboxWorkerStore};
    {
        let (state, store) = crate::rest::agents_tests::authorized_conversation_fixture().await;
        let agent = seeded_agent("agt_terminal_fence", AgentStatus::Ready, 5, None);
        insert_agent(
            &store,
            agent.clone(),
            vec![
                AgentLogKind::TurnStarted {
                    model: None,
                    turn_sequence: 3,
                    origin_key: None,
                },
                AgentLogKind::OwnerMessage {
                    content: "synthetic input".into(),
                },
                AgentLogKind::TurnCompleted { turn_sequence: 3 },
            ],
        )
        .await;
        let resource = crate::AuthenticatedResourceCaller {
            context: crate::agent_v2::fixtures::context("owner-a"),
            credential: None,
        };
        let response =
            super::super::stream::open_authorized(state, resource, agent.clone(), 3, None).await;
        assert_eq!(response.status(), StatusCode::OK);
        let evidence = response.extensions().get::<Contract>().unwrap().clone();
        let mut body = response.into_body().into_data_stream();
        let first = body.next().await.unwrap().unwrap();
        assert!(!std::str::from_utf8(&first)
            .unwrap()
            .contains("response.completed"));

        let mut sandbox = store.get_sandbox(&agent.sandbox_id).await.unwrap().unwrap();
        let revision = sandbox.revision;
        sandbox.status = layer2_core::sandbox::SandboxStatus::Retired;
        sandbox.retired_at = Some("2026-09-05T00:00:00Z".into());
        sandbox.purge_after = sandbox.retired_at.clone();
        sandbox.revision += 1;
        store.save_sandbox(revision, sandbox).await.unwrap();
        store
            .purge_sandbox(&agent.sandbox_id, revision + 1, "2026-09-05T00:00:00Z")
            .await
            .unwrap();

        assert!(body.next().await.is_none());
        assert_eq!(
            evidence.close_reason(BodyEnd::Eof),
            CloseReason::AuthorizationUnavailable
        );
    }
}

use layer2_core::sandbox::{
    agent_output_index::{
        AgentOutputChange, AgentOutputTool, AgentToolOutputState, AgentTurnOutputIndex,
    },
    store::{
        indexed::{AgentOutputMetadata, AgentOutputReader, AgentOutputTurn},
        AgentResponseLogStore, AgentResponseLogWindow, SandboxStoreError,
    },
    Agent, AgentLogEntry, AgentLogEvidenceChange, AgentLogProjectionState,
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

#[derive(Clone)]
struct IndexedSource {
    metadata: AgentOutputMetadata,
    rows: Vec<AgentLogEntry>,
    fail: Arc<AtomicBool>,
}
#[async_trait::async_trait]
impl AgentResponseLogStore for IndexedSource {
    async fn agent_output_available(&self, _: &Agent) -> Result<bool, SandboxStoreError> {
        Ok(true)
    }
    async fn agent_output_reader(
        &self,
        _: &Agent,
        _: Option<u64>,
    ) -> Result<Option<Box<dyn AgentOutputReader + '_>>, SandboxStoreError> {
        if self.fail.load(Ordering::Relaxed) {
            return Err(SandboxStoreError::Unavailable(
                "synthetic read failure".into(),
            ));
        }
        Ok(Some(Box::new(self.clone())))
    }
    async fn agent_response_log_window(
        &self,
        _: &Agent,
        _: Option<u64>,
    ) -> Result<Option<AgentResponseLogWindow>, SandboxStoreError> {
        panic!("indexed stream must not read legacy output")
    }
}
#[async_trait::async_trait]
impl AgentOutputReader for IndexedSource {
    fn metadata(&self) -> &AgentOutputMetadata {
        &self.metadata
    }
    async fn changes(
        &mut self,
        _: u64,
        _: u16,
    ) -> Result<Vec<AgentOutputChange>, SandboxStoreError> {
        Ok(vec![])
    }
    async fn tool(&mut self, _: &str) -> Result<AgentToolOutputState, SandboxStoreError> {
        Err(SandboxStoreError::NotFound)
    }
    async fn points(&mut self, seqs: &[u64]) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
        seqs.iter()
            .map(|seq| {
                self.rows
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
            .rows
            .iter()
            .filter(|row| row.seq > after)
            .take(limit.into())
            .cloned()
            .collect())
    }
}

#[tokio::test]
async fn indexed_response_records_normal_completion_and_typed_read_failure() {
    use futures_util::StreamExt;
    for fail in [false, true] {
        let (mut state, store) = crate::rest::agents_tests::authorized_conversation_fixture().await;
        let agent = seeded_agent("agt_indexed_semantic", AgentStatus::Ready, 5, None);
        insert_agent(
            &store,
            agent.clone(),
            vec![
                AgentLogKind::TurnStarted {
                    model: None,
                    turn_sequence: 3,
                    origin_key: None,
                },
                AgentLogKind::OwnerMessage {
                    content: "synthetic input".into(),
                },
                AgentLogKind::TurnCompleted { turn_sequence: 3 },
            ],
        )
        .await;
        let context = crate::agent_v2::fixtures::context("owner-a");
        let (_, rows) = state
            .reads
            .log_snapshot(&context.caller, &agent.id)
            .await
            .unwrap();
        let mut fold = AgentLogProjectionState::default();
        let mut facts = None;
        let mut index = AgentTurnOutputIndex::start(&rows[2]).unwrap();
        for row in &rows {
            for change in fold.observe(&agent, row, None).unwrap() {
                if let AgentLogEvidenceChange::Turn(value) = change {
                    facts = Some(*value);
                }
            }
            if row.seq > 3 {
                index.observe(row, AgentOutputTool::None).unwrap();
            }
        }
        let facts = facts.unwrap();
        let source = Arc::new(IndexedSource {
            metadata: AgentOutputMetadata {
                agent_id: agent.id.clone(),
                through: agent.conversation.last_seq,
                created_at: agent.created_at.clone(),
                runtime_generation: agent.conversation.runtime_generation,
                active_turn: None,
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
            fail: Arc::default(),
        });
        state.reads = state.reads.with_response_logs(Some(source.clone()));
        let response = super::super::stream::open_authorized(
            state,
            crate::AuthenticatedResourceCaller {
                context,
                credential: None,
            },
            agent,
            3,
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let evidence = response.extensions().get::<Contract>().unwrap().clone();
        let mut body = response.into_body().into_data_stream();
        for kind in ["response.created", "response.in_progress"] {
            let frame = body.next().await.unwrap().unwrap();
            assert!(std::str::from_utf8(&frame).unwrap().contains(kind));
        }
        source.fail.store(fail, Ordering::Relaxed);
        let mut tail = String::new();
        while let Some(frame) = body.next().await {
            tail.push_str(std::str::from_utf8(&frame.unwrap()).unwrap());
        }
        if fail {
            assert!(tail.is_empty());
            assert_eq!(
                evidence.close_reason(BodyEnd::Eof),
                CloseReason::DependencyError
            );
        } else {
            assert!(tail.contains("response.completed"));
            assert_eq!(
                evidence.close_reason(BodyEnd::Eof),
                CloseReason::NormalTerminal
            );
        }
    }
}
