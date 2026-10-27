use super::*;
use crate::{
    interactions::whatsapp_native_fixture::Fixture, tools::message_effects::lark_fixture::App,
};
#[tokio::test]
async fn preparation_and_status_deadlines_include_fresh_resolution() {
    let app = App::new(cmaeg_core::channel::Platform::Lark).await;
    let f = Fixture::new(false).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let records = crate::records::RecordClient::new(
        &format!("http://{}", listener.local_addr().unwrap()),
        "fixture",
    )
    .unwrap()
    .with_service_auth(
        crate::service_auth::ServiceAuth::account(
            &format!("http://{}", listener.local_addr().unwrap()),
            crate::config::ServiceAccountConfig {
                client_id: "fixture-client".into(),
                client_secret: cmaeg_core::channel::Secret::new("fixture-secret"),
            },
        )
        .unwrap(),
    );
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            axum::Router::new()
                .fallback(|| async { std::future::pending::<axum::http::StatusCode>().await }),
        )
        .await
        .unwrap();
    });
    let keys = Arc::new(crate::ingress::routing_plan::fixture::keys());
    let mut pointer = f.channel.pointer.clone();
    pointer.record_url = records
        .record_url(pointer.record_id.as_str())
        .unwrap()
        .to_string();
    let mut event = f.event.clone();
    event.channel_id = pointer.seal_id(&keys).unwrap();
    let reference = EventRef::seal(&event, &keys).unwrap();
    let resolver = crate::channels::resolver::ChannelResolver::new(
        Arc::new(records.clone()),
        keys.clone(),
        crate::ChannelCacheConfig {
            restore_timeout: Duration::from_secs(10),
            ..Default::default()
        },
    );
    let forms = forms::service::Forms::new(
        records,
        keys.clone(),
        resolver.clone(),
        f.store.as_ref().clone(),
        crate::whatsapp::Client::production().unwrap(),
        true,
        true,
    );
    let mut state = app.state.clone();
    state.resolver = resolver;
    state.keys = keys;
    state.native_whatsapp_forms = Some(Arc::new(forms));
    let caller = AgentCaller {
        key_hash: f.channel.agent_key_hash,
    };
    let message:OutboundMessage=serde_json::from_value(serde_json::json!({"blocks":[{"type":"form","id":"f","fields":[{"id":"x","label":"X","required":true,"kind":{"type":"text"}}]}]})).unwrap();
    for prepare in [true, false] {
        let started = std::time::Instant::now();
        let error = if prepare {
            state
                .prepare_form_reply(caller, &reference, message.clone(), "stable")
                .await
                .unwrap_err()
        } else {
            state
                .read_form_preparation(caller, &reference)
                .await
                .unwrap_err()
        };
        assert_eq!(error.status, 503);
        assert!(started.elapsed() >= Duration::from_millis(1900));
        assert!(started.elapsed() < Duration::from_millis(2800));
    }
    server.abort();
}
