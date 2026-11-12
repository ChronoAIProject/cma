use super::*;
use segments::{
    audit,
    projection::{self, EvidenceRecord},
};

async fn source(store: &MongoSandboxStore) -> (Agent, Vec<AgentLogEntry>) {
    let mut agent = agent();
    agent.conversation.conversation_id = Some("native-fixture".into());
    agent.conversation.runtime_generation = Some(1);
    agent.conversation.active_turn = Some(3);
    let kinds = vec![
        AgentLogKind::Created {
            workspace_id: agent.workspace_id.clone(),
            worktree_id: agent.worktree_id.clone(),
        },
        AgentLogKind::ConversationOpened {
            conversation_id: "native-fixture".into(),
            runtime_generation: 1,
        },
        AgentLogKind::TurnStarted {
            turn_sequence: 3,
            origin_key: None,
            model: None,
        },
        AgentLogKind::OwnerMessage {
            content: "fixture".into(),
        },
        AgentLogKind::ToolCallStarted {
            call_id: "call-fixture".into(),
            title: "tool".into(),
            kind: "shell".into(),
            status: "running".into(),
        },
        AgentLogKind::ToolOutputReported {
            call_id: "call-fixture".into(),
            output: layer2_core::sandbox::RecordedToolOutput {
                text: "first".into(),
                truncated: false,
            },
        },
        AgentLogKind::ToolCallUpdated {
            call_id: "call-fixture".into(),
            status: "completed".into(),
            title: None,
            output: None,
        },
        AgentLogKind::ToolOutputReported {
            call_id: "call-fixture".into(),
            output: layer2_core::sandbox::RecordedToolOutput {
                text: "later".into(),
                truncated: false,
            },
        },
        AgentLogKind::ToolCallStarted {
            call_id: "call-fixture".into(),
            title: "duplicate".into(),
            kind: "shell".into(),
            status: "failed".into(),
        },
        AgentLogKind::AgentMessage {
            content: String::new(),
        },
        AgentLogKind::AgentMessage {
            content: "answer".into(),
        },
        AgentLogKind::ModeChanged {
            mode: "code".into(),
        },
    ];
    let entries = kinds
        .into_iter()
        .enumerate()
        .map(|(i, kind)| AgentLogEntry {
            agent_id: agent.id.clone(),
            seq: i as u64 + 1,
            appended_at: NOW.into(),
            kind,
        })
        .collect();
    fixtures::install(store, agent, entries, 2, 5).await
}

async fn evidence(store: &MongoSandboxStore, head: &Head, kind: &str, key: &str) -> EvidenceRecord {
    let row = store
        .segment_logs
        .evidence
        .find_one(doc! {"_id":projection::identity(head,kind,key)})
        .await
        .unwrap()
        .unwrap();
    projection::read_evidence(head, kind, key, row).unwrap()
}

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn audit_rebuilds_output_across_units_and_refuses_missing_completed_output() {
    for damage in [false, true] {
        let (client, name) = database("audit_output").await;
        let db = client.database(&name);
        schema::migrate(&db).await.unwrap();
        let store = MongoSandboxStore::open(&client, &name).await.unwrap();
        let (agent, entries) = source(&store).await;
        let before = store
            .agent_log
            .clone_with_type::<RawDocumentBuf>()
            .find(doc! {})
            .sort(doc! {"seq":1})
            .await
            .unwrap()
            .try_collect::<Vec<_>>()
            .await
            .unwrap();
        let original_head = store
            .segment_logs
            .heads
            .find_one(doc! {"_id":agent.id.as_str()})
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            audit::run_unit(&client, &db, agent.id.as_str(), "outputs", false)
                .await
                .unwrap(),
            "running"
        );
        if damage {
            let progress = store
                .segment_logs
                .audits
                .find_one(doc! {"agent_id":agent.id.as_str()})
                .await
                .unwrap()
                .unwrap();
            store.segment_logs.evidence.delete_one(doc! {"agent_id":agent.id.as_str(),
                "evidence_generation":progress.get_str("result_generation").unwrap(),"kind":"turn_output"}).await.unwrap();
            assert_eq!(
                audit::run_unit(&client, &db, agent.id.as_str(), "outputs", false)
                    .await
                    .unwrap(),
                "corrupt"
            );
            assert!(audit::install(&client, &db, agent.id.as_str(), "outputs")
                .await
                .is_err());
            assert_eq!(
                store
                    .segment_logs
                    .heads
                    .find_one(doc! {"_id":agent.id.as_str()})
                    .await
                    .unwrap()
                    .unwrap(),
                original_head
            );
        } else {
            for expected in ["running", "complete"] {
                assert_eq!(
                    audit::run_unit(&client, &db, agent.id.as_str(), "outputs", false)
                        .await
                        .unwrap(),
                    expected
                );
            }
            assert_eq!(
                audit::install(&client, &db, agent.id.as_str(), "outputs")
                    .await
                    .unwrap(),
                "installed"
            );
            let head = Head::read(
                store
                    .segment_logs
                    .heads
                    .find_one(doc! {"_id":agent.id.as_str()})
                    .await
                    .unwrap()
                    .unwrap(),
            )
            .unwrap();
            let EvidenceRecord::TurnOutput { index } =
                evidence(&store, &head, "turn_output", "3").await
            else {
                panic!("turn output")
            };
            assert_eq!(index.through_sequence, entries.len() as u64);
            assert_eq!(index.changes, 4);
            assert_eq!(index.totals.items, 3);
            assert_eq!(index.totals.messages, 2);
            assert_eq!(index.totals.nonempty_messages, 1);
            assert_eq!(
                (index.totals.tool_starts, index.totals.tool_finishes),
                (1, 1)
            );
            let EvidenceRecord::OutputChange { change } =
                evidence(&store, &head, "output_change", "3:1").await
            else {
                panic!("terminal output")
            };
            let tool = change.tool.unwrap();
            assert_eq!(
                (
                    tool.start_sequence,
                    tool.output_sequence,
                    tool.terminal_sequence
                ),
                (5, Some(6), Some(7))
            );
            let key = projection::output::tool_key(3, "call-fixture");
            let EvidenceRecord::ToolOutput { tool } =
                evidence(&store, &head, "tool_output", &key).await
            else {
                panic!("tool output")
            };
            assert_eq!(tool.output_sequence, Some(8));
        }
        assert_eq!(
            store
                .agent_log
                .clone_with_type::<RawDocumentBuf>()
                .find(doc! {})
                .sort(doc! {"seq":1})
                .await
                .unwrap()
                .try_collect::<Vec<_>>()
                .await
                .unwrap(),
            before
        );
        db.drop().await.unwrap();
    }
}
