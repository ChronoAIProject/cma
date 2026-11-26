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

#[tokio::test]
async fn legacy_turn_activity_preserves_provider_native_identity_without_extra_requests() {
    use cmaeg::{activity::Change, turns::observer::ActivityObserver};
    use cmaeg_core::ids::{EventId, RecordId};
    use futures_util::future::BoxFuture;
    use std::sync::{Arc, Mutex};
    #[derive(Default)]
    struct Observed(Mutex<Vec<Change>>);
    impl ActivityObserver for Observed {
        fn checkpoint<'a>(
            &'a self,
            _: &'a RecordId,
            _: &'a EventId,
            change: Change,
        ) -> BoxFuture<'a, ()> {
            Box::pin(async move {
                self.0.lock().unwrap().push(change);
            })
        }
    }
    for native in [Some("agt_provider_native"), None] {
        let observed = Arc::new(Observed::default());
        let mut fixture = Fixture::new(FakeProviderConfig {
            frames: Some(vec![
                json!({"type":"response.created","response":{"id":"r","conversation":{"id":native}}}),
                json!({"type":"response.completed","response":{"output":[]}}),
            ]),
            ..Default::default()
        }, Default::default(), 4).await;
        fixture.runner = fixture.runner.with_activity(observed.clone());
        let request = fixture.request("native-identity");
        let alias = request.conversation_id.to_string();
        assert_ne!(native, Some(alias.as_str()));
        assert!(matches!(
            fixture.runner.launch(request).await,
            LaunchOutcome::Accepted { .. }
        ));
        assert_eq!(fixture.terminal().await.last().unwrap().kind(), "completed");
        let identities = observed
            .0
            .lock()
            .unwrap()
            .iter()
            .filter_map(|change| change.agent_id.clone())
            .collect::<Vec<_>>();
        assert_eq!(
            identities,
            native.into_iter().map(str::to_owned).collect::<Vec<_>>()
        );
        assert_eq!(
            fixture
                .fake
                .log()
                .iter()
                .map(|request| request.method.as_str())
                .collect::<Vec<_>>(),
            ["POST"]
        );
        fixture.fake.close().await;
    }
}

#[tokio::test]
async fn a_terminal_answer_before_acceptance_is_delivered_once_and_acknowledged() {
    for (terminal, kind) in [
        (
            json!({"type":"response.failed","error":{"code":"rejected","message":"provider rejected turn"}}),
            "failed",
        ),
        (
            json!({"type":"response.completed","response":{"output":[{"type":"message","content":[{"type":"output_text","text":"done"}]}]}}),
            "completed",
        ),
    ] {
        let mut fixture = Fixture::new(
            FakeProviderConfig {
                frames: Some(vec![terminal]),
                ..Default::default()
            },
            Default::default(),
            4,
        )
        .await;
        // The answer reaches delivery, so the source must not be told to
        // redeliver an event whose answer its user already has.
        let outcome = fixture.runner.launch(fixture.request("unaccepted")).await;
        assert!(
            matches!(outcome, LaunchOutcome::Deferred),
            "{kind}: {outcome:?}"
        );
        let outcomes = fixture.terminal().await;
        assert_eq!(outcomes.len(), 1, "{kind}: {outcomes:?}");
        assert_eq!(outcomes[0].kind(), kind);
        fixture.fake.close().await;
    }
}

#[tokio::test]
async fn a_terminal_answer_before_acceptance_is_refused_when_delivery_cannot_take_it() {
    let mut fixture = Fixture::new(
        FakeProviderConfig {
            frames: Some(vec![
                json!({"type":"response.failed","error":{"code":"rejected","message":"provider rejected turn"}}),
            ]),
            ..Default::default()
        },
        Default::default(),
        4,
    )
    .await;
    let (_, closed) = tokio::sync::mpsc::channel(1);
    drop(std::mem::replace(&mut fixture.outcomes, closed));
    match fixture
        .runner
        .launch(fixture.request("undeliverable"))
        .await
    {
        LaunchOutcome::Refused(problem) => assert_eq!(problem.status, 503),
        other => panic!("expected a retryable refusal, got {other:?}"),
    }
    fixture.fake.close().await;
}
