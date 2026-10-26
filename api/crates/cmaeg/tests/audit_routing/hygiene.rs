//! Output hygiene: the report and every log line, at trace level, carry only
//! record IDs, channel hashes and classifications.
use super::{
    fake_cma::FakeCma,
    fake_provider::{FakeProvider, FakeProviderConfig},
    routing_fixture::{agent_key, NYX, REVIEWER},
    support::*,
};
use base64::{engine::general_purpose::STANDARD_NO_PAD, Engine as _};
use cmaeg::channels::audit_routing::{audit_routing, RoutingAuditMode};
use std::{
    collections::BTreeMap,
    sync::{atomic::Ordering, Arc, Mutex},
};
use tracing_subscriber::{fmt, layer::SubscriberExt, Layer};

/// Distinctive profile text that must never reach the output.
const PROMPT: &str = "routing-audit-system-prompt-fixture";

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);
impl std::io::Write for Capture {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The fakes run on this current-thread runtime, so a thread-scoped
/// subscriber captures every log line without touching global state. It
/// applies the gateway's own filter at `RUST_LOG=trace`.
#[tokio::test(flavor = "current_thread")]
async fn no_secret_profile_owner_or_channel_id_reaches_the_output_or_logs() {
    let capture = Capture::default();
    let writer = capture.clone();
    let _logs = tracing::subscriber::set_default(
        tracing_subscriber::registry().with(
            fmt::layer()
                .with_ansi(false)
                .with_writer(move || writer.clone())
                .with_filter(cmaeg::log_layer_filter(Some("trace"))),
        ),
    );
    let cma = FakeCma::start(false).await;
    let provider = FakeProvider::start(FakeProviderConfig {
        unbind_status: BTreeMap::from([("binding-nyx".to_owned(), 500)]),
        ..FakeProviderConfig::default()
    })
    .await;
    let keys = key_ring();
    let mut value = routed_to_default();
    value["profile"]["context"]["system_prompt"] = PROMPT.into();
    let (blocked, _) = seed(&cma, &keys, value.clone(), true);
    value["routing"]["consumers"][1]["provider"]["binding_id"] = "binding-other".into();
    let (cleared, channel) = seed(&cma, &keys, value, true);
    let blob = STANDARD_NO_PAD.encode(&cma.records()[cleared.record_id.as_str()].blob);
    cma.put_conflicts.store(1, Ordering::SeqCst);
    let mut output = String::new();
    for mode in [
        RoutingAuditMode::Report,
        RoutingAuditMode::Clear {
            allow_default_widening: true,
        },
    ] {
        let report = audit_routing(
            &records(&cma),
            &keys,
            Some(&providers(&provider)),
            &non_human(),
            mode,
        )
        .await
        .unwrap();
        output.push_str(&report.lines().join("\n"));
    }
    let logs = String::from_utf8(capture.0.lock().unwrap().clone()).unwrap();
    assert!(logs.contains("routing extensions cleared"), "{logs}");
    assert!(
        logs.contains("consumer binding could not be removed"),
        "{logs}"
    );
    assert!(logs.contains(&hash(&channel)), "{logs}");
    assert!(output.contains(blocked.record_id.as_str()));
    let channel_id = channel.unwrap().as_str().to_owned();
    for text in [&output, &logs] {
        for leaked in [
            OWNER,
            PROMPT,
            "cma-service",
            "sha256:fixture",
            "fixture-channel-key",
            "77:fixture",
            "fixture-webhook",
            "Zml4dHVyZQ==",
            &agent_key("default"),
            &agent_key(REVIEWER),
            &agent_key(NYX),
            &channel_id,
            &blob[..32],
        ] {
            assert!(!text.contains(leaked), "{leaked} leaked");
        }
    }
    provider.close().await;
    cma.close().await;
}
