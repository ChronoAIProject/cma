//! Trace-level source mutation logs contain bounded identity fields, never secrets.
use super::source_support::*;
use base64::Engine as _;

#[derive(Clone, Default)]
struct Capture(Arc<std::sync::Mutex<Vec<u8>>>);
impl std::io::Write for Capture {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[tokio::test(flavor = "current_thread")]
async fn source_mutations_emit_record_and_source_ids_without_credentials_at_trace() {
    let capture = Capture::default();
    let writer = capture.clone();
    let _logs = tracing::subscriber::set_default(
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish(),
    );
    let gateway = start().await;
    let (id, record, _) = gateway.seed(|_| {});
    let hmac = base64::engine::general_purpose::STANDARD.encode(b"source-secret-for-privacy-test");
    attach(
        &gateway,
        &id,
        request(
            1,
            custom("private-hmac"),
            json!({
                "channel_key":"fixture-channel-key","telegram_bot_token":"77:fixture-private",
                "telegram_webhook_secret":"fixture-private-webhook",
                "hmac_secrets":[{"key_id":"private-hmac","secret":hmac}]
            }),
            Some("event_subject"),
        ),
    )
    .await
    .unwrap();
    detach(&gateway, &id, "custom", 2).await.unwrap();
    let logs = String::from_utf8(capture.0.lock().unwrap().clone()).unwrap();
    for required in [
        "TRACE",
        "channel source attached",
        "channel source detached",
        "source_id=custom",
        record.as_str(),
    ] {
        assert!(
            logs.contains(required),
            "missing bounded source log field {required}"
        );
    }
    for secret in [
        "fixture-channel-key",
        "77:fixture-private",
        "fixture-private-webhook",
        hmac.as_str(),
        id.as_str(),
    ] {
        assert!(
            !logs.contains(secret),
            "source mutation logged a credential or channel id"
        );
    }
    gateway.close().await;
}
