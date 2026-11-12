//! Legacy replay keeps every update while bounding sparse-source work per yield.
use super::*;
use layer2_core::sandbox::store::{AgentLogStore, AgentStore};

async fn open(f: &IndexedFixture, indexed: bool) -> axum::response::Response {
    let mut state = f.state.clone();
    if !indexed {
        for row in f.source.data.rows.values() {
            f.store
                .append_agent_log(&f.agent.id, row.appended_at.clone(), row.kind.clone())
                .await
                .unwrap();
        }
        state.reads = state.reads.with_response_logs(None);
    }
    let response = crate::responses::stream::open_authorized(
        state,
        crate::AuthenticatedResourceCaller {
            context: context("owner-a"),
            credential: Some(crate::DelegatedCredential::new("fixture-credential".into())),
        },
        f.agent.clone(),
        3,
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    response
}

fn history(noise: usize, terminal: AgentLogKind) -> Vec<AgentLogKind> {
    let mut kinds = beginning();
    kinds.extend([
        message(""),
        tool("call", "running"),
        output("call", "first"),
        update("call", "completed"),
        output("call", "latest"),
    ]);
    kinds.extend((0..noise).map(|_| AgentLogKind::ModeChanged {
        mode: "x".repeat(512),
    }));
    kinds.extend([
        message("answer"),
        terminal,
        AgentLogKind::Usage {
            input_tokens: 2,
            output_tokens: 3,
            context_used: 5,
            context_window: None,
        },
    ]);
    kinds
}

async fn collect(f: &IndexedFixture, indexed: bool) -> (Vec<Value>, usize) {
    let mut stream = open(f, indexed).await.into_body().into_data_stream();
    let mut all = vec![];
    let mut comments = 0;
    loop {
        let before = f.source.counts.source_pages.load(Ordering::SeqCst);
        let frame = tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
            .await
            .expect("one body poll must finish without scanning the whole sparse turn");
        let after = f.source.counts.source_pages.load(Ordering::SeqCst);
        assert!(
            after - before <= 8,
            "one body poll read {} pages",
            after - before
        );
        let Some(frame) = frame else { break };
        let frame = frame.unwrap();
        assert!(frame.len() <= 4 * 1024 * 1024);
        let projected = events(&frame);
        if projected.is_empty() {
            assert!(std::str::from_utf8(&frame).unwrap().starts_with(':'));
            assert!(!std::str::from_utf8(&frame).unwrap().contains("id:"));
            comments += 1;
        }
        all.extend(projected);
    }
    for (sequence, event) in all.iter().enumerate() {
        assert_eq!(event["sequence_number"], sequence as u64);
    }
    (all, comments)
}

#[tokio::test]
async fn legacy_serialized_events_match_original_completion_failure_and_interruption() {
    for terminal in [
        AgentLogKind::TurnCompleted { turn_sequence: 3 },
        AgentLogKind::TurnInterrupted { turn_sequence: 3 },
        AgentLogKind::TurnFailed {
            turn_sequence: 3,
            failure: layer2_core::sandbox::TypedFailure {
                class: "insufficient_credits".into(),
                detail: "terminal fixture".into(),
            },
        },
    ] {
        let legacy = make(history(17, terminal.clone())).await;
        let indexed = make(history(17, terminal)).await;
        let original = collect(&legacy, false).await.0;
        let (actual, comments) = collect(&indexed, true).await;
        assert!(comments > 0);
        assert_eq!(
            serde_json::to_vec(&actual).unwrap(),
            serde_json::to_vec(&original).unwrap()
        );
        let activity: Vec<_> = actual
            .iter()
            .filter(|e| e["type"] == "response.cma_chat.tool_activity")
            .collect();
        assert_eq!(activity.len(), 4);
        assert_eq!(activity[1]["item"]["output"]["text"], "first");
        assert_eq!(activity[3]["item"]["output"]["text"], "latest");
    }
}

#[tokio::test]
async fn legacy_sparse_long_turn_yields_bounded_pages_without_losing_events_or_writing() {
    let terminal = AgentLogKind::TurnCompleted { turn_sequence: 3 };
    let small = make(history(0, terminal.clone())).await;
    let long = make(history(10_200, terminal)).await;
    assert!(long.agent.conversation.last_seq > 10_128);
    let before_source = serde_json::to_vec(&long.source.data.rows).unwrap();
    assert!(before_source.len() > 4 * 1024 * 1024);
    let before_agent = long.store.get_agent(&long.agent.id).await.unwrap();
    let expected = collect(&small, false).await.0;
    let (actual, comments) = collect(&long, true).await;
    assert!(comments > 1000);
    assert_eq!(
        serde_json::to_vec(&actual).unwrap(),
        serde_json::to_vec(&expected).unwrap()
    );
    let expected_rows = long.agent.conversation.last_seq - 2;
    assert_eq!(
        long.source.counts.source_rows.load(Ordering::SeqCst) as u64,
        expected_rows
    );
    assert_eq!(
        long.source.counts.source_pages.load(Ordering::SeqCst) as u64,
        expected_rows
    );
    assert_eq!(
        long.store.get_agent(&long.agent.id).await.unwrap(),
        before_agent
    );
    assert_eq!(
        serde_json::to_vec(&long.source.data.rows).unwrap(),
        before_source
    );
}

#[tokio::test]
async fn legacy_quiet_yield_cannot_bypass_lost_proof_or_revoked_authority() {
    for revoke in [false, true] {
        let f = make(history(
            64,
            AgentLogKind::TurnCompleted { turn_sequence: 3 },
        ))
        .await;
        let before_agent = f.store.get_agent(&f.agent.id).await.unwrap();
        let before_source = serde_json::to_vec(&f.source.data.rows).unwrap();
        let mut stream = open(&f, true).await.into_body().into_data_stream();
        loop {
            let frame = stream.next().await.unwrap().unwrap();
            if events(&frame).is_empty() {
                break;
            }
        }
        let pages = f.source.counts.source_pages.load(Ordering::SeqCst);
        if revoke {
            f.purge().await;
        } else {
            f.source.counts.fail.store(true, Ordering::SeqCst);
        }
        assert!(
            stream.next().await.is_none(),
            "no completion or stale output after proof/authority loss"
        );
        assert_eq!(f.source.counts.source_pages.load(Ordering::SeqCst), pages);
        assert_eq!(
            serde_json::to_vec(&f.source.data.rows).unwrap(),
            before_source
        );
        if !revoke {
            assert_eq!(f.store.get_agent(&f.agent.id).await.unwrap(), before_agent);
        }
    }
}
