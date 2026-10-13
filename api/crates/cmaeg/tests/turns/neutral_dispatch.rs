//! Product commands and callbacks use the same provider boundary as conversation.
use super::support::Fixture;
use cmaeg::{
    ingress::launch::{LaunchOutcome, TurnLauncher},
    turns::outcomes::TurnOutcome,
};
use cmaeg_core::activity::{ActivityKind, InteractionResult};
use serde_json::{json, Value};

#[tokio::test]
async fn every_message_command_and_interaction_reaches_the_provider_unchanged() {
    let mut fixture = Fixture::new(Default::default(), Default::default(), 4).await;
    let mut cases: Vec<(ActivityKind, String)> = [
        "hi",
        "How's the weather today?",
        "Help me create a Telegram bot",
        "create",
        "{\"agent\":{}}",
        "/setup",
        "/setup create",
        "/start setup",
        "/start link_fixture-123",
        "link_fixture-123",
    ]
    .into_iter()
    .map(|text| {
        (
            ActivityKind::Message {
                text: text.into(),
                attachments: vec![],
                mentions_bot: false,
                reply_to: None,
                album_id: None,
            },
            text.to_owned(),
        )
    })
    .collect();
    for (name, args) in [("setup", "create"), ("start", "link_fixture-123")] {
        cases.push((
            ActivityKind::Command {
                name: name.into(),
                args: args.into(),
            },
            format!("/{name} {args}"),
        ));
    }
    for block_id in ["preferences", "bst_0123456789abcdef0123456789abcdef.1"] {
        cases.push((
            ActivityKind::Interaction(InteractionResult {
                block_id: block_id.into(),
                action_id: "create".into(),
                values: json!({"label":"Create", "organization":"fixture-org"})
                    .as_object()
                    .unwrap()
                    .clone(),
            }),
            format!("The user chose \"Create\" (block {block_id})."),
        ));
    }
    for (index, (kind, expected_text)) in cases.into_iter().enumerate() {
        let request = fixture.request_with_kind(&format!("neutral-{index}"), Some(kind));
        let expected_activity = serde_json::to_value(&request.activity).unwrap();
        let expected_ref = request.event_ref.as_str().to_owned();
        let expected_conversation = request.conversation_id.to_string();
        assert!(matches!(
            fixture.runner.launch(request).await,
            LaunchOutcome::Accepted { response_id } if response_id == "resp_fake"
        ));
        let outcomes = fixture.terminal().await;
        assert!(matches!(
            outcomes.last(),
            Some(TurnOutcome::Completed { final_text: Some(text) }) if text == "hello"
        ));
        let requests = fixture.fake.requests();
        assert_eq!(requests.len(), index + 1);
        let upstream = &requests[index];
        assert_eq!(upstream.method, "POST");
        assert_eq!(upstream.path, "/api/v2/responses");
        let body: Value = serde_json::from_str(&upstream.body).unwrap();
        assert_eq!(body["input"][0]["content"][0]["text"], expected_text);
        assert_eq!(body["event_context"]["activity"], expected_activity);
        assert_eq!(body["event_context"]["event_ref"], expected_ref);
        assert_eq!(
            body["event_context"]["conversation_id"],
            expected_conversation
        );
    }
    fixture.fake.close().await;
}
