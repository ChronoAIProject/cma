use cma_sdk::cmaeg::{Cmaeg, EventOptions, GatewayEvent, WebhookAuth, types::*};
use serde_json::{Value, json};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn loopback(value: &str) -> String {
    let url = reqwest::Url::parse(value).unwrap();
    assert!(url.scheme() == "http" && url.host_str() == Some("127.0.0.1"));
    assert!(
        url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
    );
    url.as_str().trim_end_matches('/').into()
}
async fn get(http: &reqwest::Client, url: String) -> Value {
    let bytes = http
        .get(url)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .bytes()
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}
async fn reference(http: &reqwest::Client, root: &str, event_id: &str) -> String {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let value = get(http, format!("{root}/event-ref/{event_id}")).await;
            if let Some(reference) = value["event_ref"].as_str() {
                return reference.to_owned();
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
#[ignore = "requires the isolated CMAEG SDK HTTP fixture"]
async fn gateway_management_custom_events_and_tools_interoperate() {
    let root =
        loopback(&std::env::var("CMAEG_FIXTURE_ROOT").expect("CMAEG_FIXTURE_ROOT is required"));
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let config = get(&http, format!("{root}/config")).await;
    let base = loopback(config["gateway"].as_str().unwrap());
    let creator = Cmaeg::builder(&base)
        .unwrap()
        .bearer_token(config["creator"].as_str().unwrap())
        .unwrap()
        .build()
        .unwrap();
    let tools = Cmaeg::builder(&base)
        .unwrap()
        .bearer_token(config["agent_key"].as_str().unwrap())
        .unwrap()
        .build()
        .unwrap();
    let body: ChannelCreateRequest = serde_json::from_value(config["create"].clone()).unwrap();
    let created = creator
        .create_channel(&body, Some("rust-sdk-fixture"))
        .await
        .unwrap();
    let id = &created.channel_id;
    let recovered = creator
        .recover_channel(body.record_id.as_deref().unwrap())
        .await
        .unwrap();
    assert!(recovered.channel_id == *id);
    let current = creator.get_channel(id).await.unwrap();
    let mut update = config["create"].clone();
    update.as_object_mut().unwrap().remove("record_id");
    update.as_object_mut().unwrap().remove("credentials");
    update["name"] = json!("SDK updated policy");
    update["expected_version"] = json!(current.version);
    let updated = creator
        .update_channel(id, &serde_json::from_value(update).unwrap())
        .await
        .unwrap();
    assert!(updated.definition.name.as_deref() == Some("SDK updated policy"));
    creator.test_channel(id).await.unwrap();
    let sequence = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let text = "é".repeat(16 * 1024);
    let mut event: CloudEvent = serde_json::from_value(json!({"specversion":"1.0","id":format!("rust-{sequence}"),"source":"urn:sdk:rust","type":"sdk.test","data":{"text":text}})).unwrap();
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        .to_string();
    let options = EventOptions {
        webhook: Some(WebhookAuth {
            secret: config["signing"].as_str().unwrap(),
            id: "rust-webhook",
            timestamp: &timestamp,
        }),
    };
    let accepted = creator.post_event(id, &event, &options).await.unwrap();
    assert!(accepted.event_id == event.id);
    let reference_id = reference(&http, &root, &event.id).await;
    assert!(reference_id.len() <= 4096);
    let context = tools.read_event_context(&reference_id).await.unwrap();
    let original = serde_json::to_value(&context).unwrap();
    assert!(
        matches!(context.activity.kind,ActivityKind::Event { ce, .. } if ce.data.as_ref().unwrap()["text"] == text)
    );
    let replaced = http
        .post(format!("{root}/restart"))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .bytes()
        .await
        .unwrap();
    let replaced: Value = serde_json::from_slice(&replaced).unwrap();
    assert_eq!(replaced["replaced"], true);
    let recovered = tools.read_event_context(&reference_id).await.unwrap();
    assert_eq!(serde_json::to_value(recovered).unwrap(), original);
    assert!(matches!(
        tools.send_typing(&reference_id).await.unwrap().outcome,
        TypingOutcome::Unsupported
    ));
    event.id.push_str("-stream");
    let mut stream = creator.stream_event(id, &event, &options).await.unwrap();
    assert!(matches!(stream.next().await, Some(Ok(_))));
    let reference_id = reference(&http, &root, &event.id).await;
    let message: OutboundMessage =
        serde_json::from_value(json!({"text":"SDK explicit reply"})).unwrap();
    let receipt = tools.reply_to_event(&reference_id, &message).await.unwrap();
    assert!(matches!(
        receipt.delivery,
        DeliveryOutcome::Confirmed { .. }
    ));
    let mut replied = false;
    let mut completed = false;
    while let Some(frame) = stream.next().await {
        match frame.unwrap() {
            GatewayEvent::Message(value) => {
                replied |= value.message.as_ref().and_then(|m| m.text.as_deref())
                    == Some("SDK explicit reply")
            }
            GatewayEvent::Completed(_) => completed = true,
            GatewayEvent::Failed(_) | GatewayEvent::Backpressure => panic!("fixture turn failed"),
            GatewayEvent::TextDelta(_) => {}
        }
    }
    assert!(replied && completed);
    let current = creator.get_channel(id).await.unwrap();
    creator.delete_channel(id, current.version).await.unwrap();
    assert!(
        matches!(creator.get_channel(id).await,Err(cma_sdk::Error::Api { code:Some(code),.. }) if code == "channel_not_found")
    );
}
