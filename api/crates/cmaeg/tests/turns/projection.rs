use super::{fake_provider::FakeProviderConfig, support::Fixture};
use cmaeg::{
    ingress::launch::{LaunchOutcome, TurnLauncher},
    turns::outcomes::TurnOutcome,
};
use serde_json::json;
#[tokio::test]
async fn messages_tools_and_final_snapshots_project_once_in_order() {
    let frames = vec![
        json!({"type":"response.in_progress","response":{"id":"response-1"}}),
        json!({"type":"response.output_item.added","output_index":0,"item":{"type":"message","id":"m1"}}),
        json!({"type":"response.output_text.delta","item_id":"m1","delta":"draft"}),
        json!({"type":"response.output_text.done","item_id":"m1","text":"answer"}),
        json!({"type":"response.output_item.done","output_index":0,"item":{"type":"message","id":"m1","content":[{"type":"output_text","text":"answer"}]}}),
        json!({"type":"response.output_item.added","output_index":1,"item":{"type":"mcp_call","id":"t1","name":"send_message","status":"in_progress"}}),
        json!({"type":"response.mcp_call.in_progress","item_id":"t1"}),
        json!({"type":"response.mcp_call.completed","item_id":"t1"}),
        json!({"type":"response.output_item.done","output_index":1,"item":{"type":"mcp_call","id":"t1","name":"send_message","status":"completed"}}),
        json!({"type":"response.future"}),
        json!({"type":"response.completed","response":{"output":[{"type":"message","content":[{"type":"output_text","text":"final answer"}]}]}}),
    ];
    let mut fixture = Fixture::new(
        FakeProviderConfig {
            frames: Some(frames),
            ..Default::default()
        },
        Default::default(),
        4,
    )
    .await;
    assert!(
        matches!(fixture.runner.launch(fixture.request("projection")).await,LaunchOutcome::Accepted {response_id} if response_id=="response-1")
    );
    assert_eq!(
        fixture.terminal().await,
        vec![
            TurnOutcome::Accepted {
                thread_reply: false,
                response_id: "response-1".into()
            },
            TurnOutcome::TextDelta {
                text: "draft".into()
            },
            TurnOutcome::MessageCommitted {
                text: "answer".into()
            },
            TurnOutcome::ToolStarted {
                name: "send_message".into()
            },
            TurnOutcome::ToolFinished {
                name: "send_message".into(),
                ok: true
            },
            TurnOutcome::Completed {
                final_text: Some("final answer".into())
            },
        ]
    );
    fixture.fake.close().await;
}
#[tokio::test]
async fn incomplete_failed_and_oversized_outputs_remain_explicit() {
    for (terminal, kind) in [
        (
            json!({"type":"response.incomplete","incomplete_details":{"reason":"max_output_tokens"}}),
            "incomplete",
        ),
        (
            json!({"type":"response.failed","error":{"code":"rejected","message":"provider rejected turn"}}),
            "failed",
        ),
        (
            json!({"type":"response.output_text.delta","item_id":"m1","delta":"x".repeat(256*1024+1)}),
            "failed",
        ),
    ] {
        let mut fixture = Fixture::new(
            FakeProviderConfig {
                frames: Some(vec![
                    json!({"type":"response.created","response":{"id":"r"}}),
                    terminal,
                ]),
                ..Default::default()
            },
            Default::default(),
            4,
        )
        .await;
        assert!(matches!(
            fixture.runner.launch(fixture.request("terminal")).await,
            LaunchOutcome::Accepted { .. }
        ));
        let outcomes = fixture.terminal().await;
        assert_eq!(outcomes.last().unwrap().kind(), kind);
        fixture.fake.close().await;
    }
}
