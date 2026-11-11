use super::*;
use crate::store::agent_log_segments::{
    capture,
    projection::{self, EvidenceRecord},
    publication, writer_commit,
};
use layer2_core::sandbox::{AgentLogKind, RecordedToolOutput};

async fn publish(f: &mut Fixture, kinds: Vec<AgentLogKind>, key: &str, commit: bool) {
    let mut next = f.agent.clone();
    let logs = kinds
        .into_iter()
        .enumerate()
        .map(|(offset, kind)| AgentLogEntry {
            agent_id: f.agent.id.clone(),
            seq: f.agent.conversation.last_seq + offset as u64 + 1,
            appended_at: NOW.into(),
            kind,
        })
        .collect::<Vec<_>>();
    next.conversation.last_seq += logs.len() as u64;
    next.revision += 1;
    let operation = PublicationIdentity {
        key: key.into(),
        appended_at: NOW.into(),
        fingerprint: publication::request_digest(&logs).unwrap(),
    };
    let mut session = transaction(f).await;
    let mut scope = TransactionScope::start(&mut session).await.unwrap();
    let captured = succeeded(capture::capture_transaction(&f.store, &f.agent, &mut scope).await);
    let plan = succeeded(
        writer_commit::prepare(
            &f.store,
            &mut scope,
            &captured,
            PublicationInput {
                operation: &operation,
                current: &f.agent,
                next: &next,
                logs: &logs,
                capacity: AppendCapacity::Normal,
                result_digest: publication::result_digest(&next).unwrap(),
            },
            None,
            None,
        )
        .await,
    );
    succeeded(
        writer_commit::persist(
            &f.store,
            &mut scope,
            plan,
            AggregateUpdate {
                filter: doc! {"_id":f.agent.id.as_str()},
                fields: agent_replacement_fields(&next).unwrap(),
            },
        )
        .await,
    );
    if commit {
        assert!(matches!(
            commit_transaction(scope.release()).await,
            CommitOutcome::Committed
        ));
        f.agent = next;
        f.head = Head::read(
            collection(&f.store, HEADS)
                .find_one(doc! {"_id":f.agent.id.as_str()})
                .await
                .unwrap()
                .unwrap(),
        )
        .unwrap();
    } else {
        scope.release().abort_transaction().await.unwrap();
    }
}

fn original_turn() -> Vec<AgentLogKind> {
    vec![
        AgentLogKind::TurnStarted {
            turn_sequence: 3,
            origin_key: Some("fixture-turn".into()),
            model: Some("original-model".into()),
        },
        AgentLogKind::OwnerMessage {
            content: "fixture input".into(),
        },
        AgentLogKind::AgentMessage { content: "".into() },
        AgentLogKind::AgentMessage {
            content: " ".into(),
        },
        AgentLogKind::ToolCallStarted {
            call_id: "call:fixture".into(),
            title: "tool".into(),
            kind: "execute".into(),
            status: "running".into(),
        },
        AgentLogKind::ToolCallUpdated {
            call_id: "call:fixture".into(),
            title: None,
            status: "completed".into(),
            output: Some(RecordedToolOutput {
                text: "first".into(),
                truncated: false,
            }),
        },
        AgentLogKind::TurnCompleted { turn_sequence: 3 },
    ]
}

async fn evidence(f: &Fixture, kind: &str, key: &str) -> Document {
    collection(&f.store, EVIDENCE)
        .find_one(doc! {"_id":projection::identity(&f.head, kind, key)})
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn output_evidence_commits_atomically_and_late_output_preserves_first_terminal() {
    let mut f = fixture(2).await;
    publish(&mut f, original_turn(), "fixture-output-abort", false).await;
    unchanged(&f).await;
    assert_eq!(
        collection(&f.store, EVIDENCE)
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    publish(&mut f, original_turn(), "fixture-output-commit", true).await;
    let row = evidence(&f, "turn_output", "3").await;
    let EvidenceRecord::TurnOutput { index } =
        projection::read_evidence(&f.head, "turn_output", "3", row).unwrap()
    else {
        panic!("turn output")
    };
    assert_eq!(
        (
            index.changes,
            index.totals.messages,
            index.totals.nonempty_messages
        ),
        (4, 2, 1)
    );
    assert_eq!(
        (
            index.totals.items,
            index.totals.tool_starts,
            index.totals.tool_finishes
        ),
        (3, 1, 1)
    );
    let terminal_before = evidence(&f, "output_change", "3:3").await;
    assert_eq!(terminal_before.get_i64("change_ordinal").unwrap(), 3);
    publish(
        &mut f,
        vec![AgentLogKind::ToolOutputReported {
            call_id: "call:fixture".into(),
            output: RecordedToolOutput {
                text: "late".into(),
                truncated: false,
            },
        }],
        "fixture-late-output",
        true,
    )
    .await;
    assert_eq!(evidence(&f, "output_change", "3:3").await, terminal_before);
    let key = projection::output::tool_key(3, "call:fixture");
    let tool = evidence(&f, "tool_output", &key).await;
    let EvidenceRecord::ToolOutput { tool } =
        projection::read_evidence(&f.head, "tool_output", &key, tool).unwrap()
    else {
        panic!("tool")
    };
    assert_eq!(
        (
            tool.start_sequence,
            tool.terminal_sequence,
            tool.output_sequence
        ),
        (7, Some(8), Some(10))
    );
    assert_eq!(
        collection(&f.store, EVIDENCE)
            .count_documents(doc! {"kind":"output_change"})
            .await
            .unwrap(),
        4
    );
    f.destroy().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn missing_current_output_index_refuses_before_any_publication_write() {
    let mut f = fixture(2).await;
    publish(&mut f, original_turn(), "fixture-output-seed", true).await;
    collection(&f.store, EVIDENCE)
        .delete_one(doc! {"kind":"turn_output"})
        .await
        .unwrap();
    let mut next = f.agent.clone();
    next.conversation.last_seq += 1;
    next.revision += 1;
    let logs = [AgentLogEntry {
        agent_id: f.agent.id.clone(),
        seq: next.conversation.last_seq,
        appended_at: NOW.into(),
        kind: AgentLogKind::ConversationGap { frames: 1 },
    }];
    let operation = PublicationIdentity {
        key: "fixture-missing-output".into(),
        appended_at: NOW.into(),
        fingerprint: publication::request_digest(&logs).unwrap(),
    };
    let mut session = transaction(&f).await;
    let mut scope = TransactionScope::start(&mut session).await.unwrap();
    let captured = succeeded(capture::capture_transaction(&f.store, &f.agent, &mut scope).await);
    assert!(writer_commit::prepare(
        &f.store,
        &mut scope,
        &captured,
        PublicationInput {
            operation: &operation,
            current: &f.agent,
            next: &next,
            logs: &logs,
            capacity: AppendCapacity::Normal,
            result_digest: publication::result_digest(&next).unwrap(),
        },
        None,
        None
    )
    .await
    .is_err());
    scope.release().abort_transaction().await.unwrap();
    unchanged(&f).await;
    assert_eq!(
        collection(&f.store, PUBLICATIONS)
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    f.destroy().await;
}
