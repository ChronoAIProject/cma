//! Explicit, loopback-only conformance against CMA's disposable command-worker fixture.
use cma_sdk::{
    Client,
    responses::CreateConversationRequest,
    responses_v2::{CreateResponseRequest, ItemOptions, ResponseEventPayload},
};

#[tokio::test]
#[ignore = "requires the disposable official_responses_sdk_server on loopback"]
async fn cma_v2_fixture_completes_two_turns_and_replays_the_same_receipt() {
    let base = std::env::var("CMA_SDK_FIXTURE_ROOT").expect("loopback fixture URL");
    let url = reqwest::Url::parse(&base).unwrap();
    assert_eq!(url.scheme(), "http");
    assert_eq!(url.host_str(), Some("127.0.0.1"));
    assert!(url.username().is_empty() && url.password().is_none());
    assert!(url.query().is_none() && url.fragment().is_none());
    let client = Client::builder(&base)
        .unwrap()
        .bearer_token("local-sdk-conformance")
        .unwrap()
        .build()
        .unwrap();
    let conversation = client
        .responses()
        .create_conversation(&CreateConversationRequest::default())
        .await
        .unwrap();
    let mut previous = None;
    for turn in 0..2 {
        let body = CreateResponseRequest::new(&conversation.id, "sdk-helper-text");
        let key = format!("rust-fixture-{turn}");
        let mut stream = client
            .responses_v2()
            .create(&body, Some(&key))
            .await
            .unwrap();
        let receipt = stream.receipt().clone();
        assert_ne!(previous, Some(receipt.response_id.clone()));
        let mut events = Vec::new();
        while let Some(event) = stream.next().await {
            events.push(event.unwrap());
        }
        assert_eq!(events.len(), 9);
        let ResponseEventPayload::Completed { response } = &events.last().unwrap().payload else {
            panic!("missing completed response");
        };
        assert_eq!(response.output[0].content[0].text, "Hello from CMA");
        assert_eq!(
            client
                .responses_v2()
                .read(&receipt.response_id)
                .await
                .unwrap(),
            *response
        );
        let mut alias = client
            .responses_v2()
            .create_for_agent(&conversation.id, &body, Some(&key))
            .await
            .unwrap();
        assert_eq!(alias.receipt(), &receipt);
        for event in &events {
            assert_eq!(alias.next().await.unwrap().unwrap(), *event);
        }
        assert!(alias.next().await.is_none());
        let mut replay = client
            .responses_v2()
            .replay(&receipt.response_id, Some(4))
            .await
            .unwrap();
        for event in &events[5..] {
            assert_eq!(replay.next().await.unwrap().unwrap(), *event);
        }
        assert!(replay.next().await.is_none());
        previous = Some(receipt.response_id);
    }
    let items = client
        .responses_v2()
        .list_items(&conversation.id, &ItemOptions::default())
        .await
        .unwrap();
    assert_eq!(items.data.len(), 4);
}
