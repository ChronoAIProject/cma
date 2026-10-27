use super::*;
use crate::channels::definition::fixture as definition_fixture;
use cmaeg_core::channel::Secret;

#[tokio::test]
async fn native_media_keeps_immutable_original_consumer_provider_and_message_authority() {
    use crate::native_lark::{message, test_support as native};
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    let mut source_request = fixture::request(
        |v| {
            v["sources"] = json!([native::source()]);
            let c = native::credentials();
            v["secrets"]["native_lark"] = json!({"app_secret":c.app_secret.as_str(),"encrypt_key":c.encrypt_key.as_str(),"verification_token":c.verification_token.as_str()});
            v["routing"]["default_targets"] = json!(["default"]);
        },
        fixture::message("unused"),
    );
    let Source::NativeLark(source) = native::source() else {
        panic!()
    };
    let mut raw = native::text("om_original", "", now);
    raw["event"]["message"]["message_type"] = "image".into();
    raw["event"]["message"]["content"] = json!({"image_key":"img_original"}).to_string().into();
    let message = message::extract(&source, &raw, now).unwrap().unwrap();
    source_request = message::launch(
        source_request.channel.clone(),
        &source,
        &message,
        None,
        &fixture::keys(),
        std::time::Instant::now(),
        now,
    )
    .unwrap();
    let server = Server::start_realtime().await;
    let keys = Arc::new(fixture::keys());
    let (activity, event) =
        bind_received_message_fixture(server.records.clone(), keys.clone(), &source_request, None)
            .await;
    let caller = AgentCaller { key_hash: [7; 32] };
    assert!(current_received_message(
        server.records.clone(),
        keys.clone(),
        &authority(source_request.channel.clone()),
        caller,
        &event,
        &activity
    )
    .await
    .is_ok());
    for defect in ["provider", "source", "credential", "consumer"] {
        let mut channel = (*source_request.channel).clone();
        let mut event = event.clone();
        match defect {
            "provider" => definition_fixture::replace_default_binding(
                &mut channel.definition,
                "other-provider".parse().unwrap(),
            ),
            "source" => channel.definition.sources.clear(),
            "credential" => {
                channel
                    .definition
                    .secrets
                    .native_lark
                    .as_mut()
                    .unwrap()
                    .app_secret = Secret::new("fixture-rotated-native-secret")
            }
            _ => event.consumer = Some(fixture::id(fixture::REVIEWER)),
        }
        assert!(
            current_received_message(
                server.records.clone(),
                keys.clone(),
                &authority(Arc::new(channel)),
                caller,
                &event,
                &activity
            )
            .await
            .is_err(),
            "{defect}"
        );
    }
}
