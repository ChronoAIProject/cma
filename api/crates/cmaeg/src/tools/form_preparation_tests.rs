use super::*;
use crate::{
    interactions::whatsapp_native_fixture::Fixture, tools::message_effects::lark_fixture::App,
};
#[tokio::test(start_paused = true)]
async fn preparation_and_status_deadlines_include_fresh_resolution() {
    let app = App::new(cmaeg_core::channel::Platform::Lark).await;
    let f = Fixture::new(false).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let request_started = Arc::new(tokio::sync::Notify::new());
    let notify = request_started.clone();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            axum::Router::new().route(
                "/oauth/token",
                axum::routing::post(move || async move {
                    notify.notify_one();
                    std::future::pending::<axum::http::StatusCode>().await
                }),
            ),
        )
        .await
        .unwrap();
    });
    let keys = Arc::new(crate::ingress::routing_plan::fixture::keys());
    let caller = AgentCaller {
        key_hash: f.channel.agent_key_hash,
    };
    let message:OutboundMessage=serde_json::from_value(serde_json::json!({"blocks":[{"type":"form","id":"f","fields":[{"id":"x","label":"X","required":true,"kind":{"type":"text"}}]}]})).unwrap();
    for prepare in [true, false] {
        // A cancelled exchange retains an auth retry guard; each deadline needs fresh state.
        let records = crate::records::RecordClient::new(&origin, "fixture")
            .unwrap()
            .with_service_auth(
                crate::service_auth::ServiceAuth::account(
                    &origin,
                    crate::config::ServiceAccountConfig {
                        client_id: "fixture-client".into(),
                        client_secret: cmaeg_core::channel::Secret::new("fixture-secret"),
                    },
                )
                .unwrap(),
            );
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
        state.keys = keys.clone();
        state.native_whatsapp_forms = Some(Arc::new(forms));
        let started = tokio::time::Instant::now();
        let operation = async {
            if prepare {
                state
                    .prepare_form_reply(caller, &reference, message.clone(), "stable")
                    .await
            } else {
                state.read_form_preparation(caller, &reference).await
            }
        };
        tokio::pin!(operation);
        loop {
            tokio::select! {
                biased;
                result = &mut operation => panic!("prepare={prepare} completed before token exchange: {result:?}"),
                _ = request_started.notified() => break,
                // Keep virtual time fixed while real socket I/O reaches the stalled handler.
                _ = tokio::task::yield_now() => {}
            }
        }
        assert_eq!(started.elapsed(), Duration::ZERO);
        tokio::time::advance(Duration::from_millis(1999)).await;
        assert!(
            futures_util::poll!(&mut operation).is_pending(),
            "prepare={prepare}"
        );
        tokio::time::advance(Duration::from_millis(1)).await;
        let error = operation.await.unwrap_err();
        assert_eq!(
            started.elapsed(),
            Duration::from_secs(2),
            "prepare={prepare}"
        );
        assert_eq!(error.status, 503);
        assert_eq!(error.code, ProblemCode::ProviderUnavailable);
        assert_eq!(
            error.detail.as_deref(),
            Some(if prepare {
                "whatsapp_form_admission_unconfirmed_retry_same_key"
            } else {
                "whatsapp_form_status_unavailable"
            }),
        );
    }
    server.abort();
}
