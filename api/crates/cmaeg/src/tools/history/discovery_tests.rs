use super::*;
use cmaeg_core::{
    channel::{LarkRegion, NativeLarkIdentity, NativeLarkReply, Platform},
    seal::ReplyAddress,
};
use std::sync::atomic::Ordering;

async fn authorized(app: &App) -> (AgentCaller, Arc<ResolvedChannel>) {
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        "authorization",
        format!("Bearer {}", app.key).parse().unwrap(),
    );
    let channel = app
        .state
        .resolver
        .resolve_current(app.event.channel_id.as_str())
        .await
        .unwrap();
    (AgentCaller::from_headers(&headers).unwrap(), channel)
}
#[tokio::test]
async fn other_platform_hints_perform_no_storage_or_native_reads_when_history_is_enabled() {
    let app = App::new(0, false).await;
    let (caller, channel) = authorized(&app).await;
    let before = app.backend.gets.load(Ordering::SeqCst);
    app.backend.get_delay_ms.store(500, Ordering::SeqCst);
    for reply in [
        ReplyAddress::None,
        ReplyAddress::Sync {
            waiter: "custom".into(),
        },
        ReplyAddress::ResultWebhook {
            url: "https://result.example".into(),
            hmac_key_id: "fixture".into(),
        },
        ReplyAddress::Telegram {
            bot_id: "77".into(),
            chat_id: "98".into(),
            message_id: "700".into(),
            thread_id: None,
        },
        ReplyAddress::Whatsapp {
            app_id: "11".into(),
            waba_id: "12".into(),
            phone_number_id: "13".into(),
            recipient_id: "14".into(),
            message_id: "wamid.fixture".into(),
        },
        ReplyAddress::NativeLark(NativeLarkReply {
            mentioned: false,
            identity: NativeLarkIdentity {
                region: LarkRegion::Lark,
                app_id: "cli_fixture".into(),
                tenant_key: "tenant".into(),
                bot_open_id: "ou_bot".into(),
            },
            chat_id: "oc_chat".into(),
            message_id: "om_message".into(),
            thread_id: None,
            revision: "a".repeat(64),
        }),
        ReplyAddress::NyxidRelay {
            thread_reply: false,
            message_id: "message".into(),
            route_id: "route".into(),
            platform: Platform::Lark,
            reply_token: Secret::new("fixture"),
            reply_token_expires_at: now() + 600,
        },
    ] {
        let mut event = app.event.clone();
        event.reply = reply;
        assert!(app
            .state
            .history_capabilities(caller, &event, &channel)
            .await
            .is_none());
    }
    assert_eq!(app.backend.gets.load(Ordering::SeqCst), before);
    assert!(app.memory.lock().unwrap().calls.is_empty());
}
#[tokio::test]
async fn absent_consent_avoids_enrichment_io_and_slow_eligible_discovery_is_optional() {
    let app = App::new(0, false).await;
    let (caller, channel) = authorized(&app).await;
    let mut no_consent = (*channel).clone();
    if let Source::Discord {
        conversation_history,
        ..
    } = &mut no_consent.definition.sources[0]
    {
        *conversation_history = false;
    }
    let before = app.backend.gets.load(Ordering::SeqCst);
    assert!(app
        .state
        .history_capabilities(caller, &app.event, &no_consent)
        .await
        .is_none());
    assert_eq!(app.backend.gets.load(Ordering::SeqCst), before);
    app.backend.get_delay_ms.store(500, Ordering::SeqCst);
    let started = Instant::now();
    assert!(app
        .state
        .history_capabilities(caller, &app.event, &channel)
        .await
        .is_none());
    assert!(started.elapsed() < Duration::from_millis(300));
    assert!(app.memory.lock().unwrap().calls.is_empty());
    assert_eq!(
        app.state.history_client().unwrap().budget,
        Duration::from_secs(8)
    );
}
