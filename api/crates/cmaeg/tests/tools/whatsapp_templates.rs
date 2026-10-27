use super::support::App;
use cmaeg::auth::AgentCaller;
use cmaeg_core::{channel::Platform, seal::ReplyAddress};
use reqwest::header::HeaderMap;

#[tokio::test]
async fn template_discovery_requires_rollout_and_the_sealed_whatsapp_destination() {
    let mut app = App::new().await;
    // Native rollout cannot enable the relay capability.
    app.state.native_whatsapp_templates_enabled = true;
    let mut headers = HeaderMap::new();
    headers.insert(
        "authorization",
        format!("Bearer {}", app.key).parse().unwrap(),
    );
    let mut event = app
        .event_ref
        .open(&app.state.keys, super::relay_support::now())
        .unwrap()
        .payload;
    for platform in [
        Platform::Telegram,
        Platform::Lark,
        Platform::Discord,
        Platform::Whatsapp,
    ] {
        let ReplyAddress::NyxidRelay {
            platform: destination,
            ..
        } = &mut event.reply
        else {
            unreachable!()
        };
        *destination = platform;
        let reference = cmaeg_core::seal::EventRef::seal(&event, &app.state.keys).unwrap();
        for enabled in [false, true] {
            app.state.whatsapp_templates_enabled = enabled;
            let expected = (enabled && platform == Platform::Whatsapp).then_some(true);
            let context = app
                .state
                .context(AgentCaller::from_headers(&headers).unwrap(), &reference)
                .await
                .unwrap();
            let verified = app
                .state
                .verify_context(
                    AgentCaller::from_headers(&headers).unwrap(),
                    &reference,
                    app.activity.clone(),
                )
                .await
                .unwrap();
            for value in [context, verified] {
                assert_eq!(value.capabilities.whatsapp_templates, expected);
                if expected.is_none() {
                    assert!(serde_json::to_value(value.capabilities)
                        .unwrap()
                        .get("whatsapp_templates")
                        .is_none());
                }
            }
        }
    }
}
