//! Late transport completions and pre-transport refusals share one receipt path.
use super::{
    fake_http::FakeHttp,
    message,
    support::{self, caller, App},
    unknown,
};
use crate::{delivery::router::ShardedScheduler, http::request_start::RequestStart};
use cmaeg_core::channel::ReplyChoice;
use std::{
    io::Write,
    sync::{Arc, Mutex},
};
use tokio::{sync::mpsc, time::Instant};

#[derive(Clone, Default)]
struct Logs(Arc<Mutex<Vec<u8>>>);
impl Write for Logs {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn late_outcomes_log_only_safe_ids_and_count_each_reply_and_update() {
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let metrics = recorder.handle();
    let logs = Logs::default();
    let writer = logs.clone();
    let _subscriber = tracing::subscriber::set_default(
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_ansi(false)
            .without_time()
            .with_writer(move || writer.clone())
            .finish(),
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .start_paused(true)
        .build()
        .unwrap();
    metrics::with_local_recorder(&recorder, || {
        runtime.block_on(async {
            for destination in [None, Some(ReplyChoice::Default)] {
                let mut app = App::new(true, true, FakeHttp::platforms());
                app.drain().await;
                let (inbound, commands) = mpsc::channel(16);
                app.state.delivery = Arc::new(ShardedScheduler::new(inbound));
                let reference = app.reference(1);
                let receipt = app
                    .state
                    .reply(
                        caller(1),
                        &reference,
                        message("secret late reply"),
                        destination,
                        RequestStart(Instant::now()),
                    )
                    .await
                    .unwrap();
                unknown(&serde_json::to_value(receipt).unwrap());
                let message_ref = app.message_ref(1);
                let receipt = app
                    .state
                    .update(
                        caller(1),
                        &message_ref,
                        message("secret late update"),
                        RequestStart(Instant::now()),
                    )
                    .await
                    .unwrap();
                unknown(&serde_json::to_value(receipt).unwrap());
                app.engine = Some(support::start_engine(
                    commands,
                    &app.state,
                    app.http.clone(),
                ));
                app.finish().await;
            }
        })
    });
    let logs = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
    let late: Vec<_> = logs
        .lines()
        .filter(|line| line.contains("finished after its caller was answered"))
        .collect();
    assert_eq!(late.len(), 4, "{logs}");
    let rejected: Vec<_> = late
        .iter()
        .filter(|line| line.contains("outcome=\"rejected\""))
        .collect();
    assert_eq!(rejected.len(), 1, "{logs}");
    assert!(rejected[0].contains("WARN"));
    assert!(rejected[0].contains("consumer=csm_000000000001"));
    for line in late
        .iter()
        .filter(|line| line.contains("outcome=\"confirmed\""))
    {
        assert!(line.contains("INFO"));
    }
    for secret in [
        "secret late reply",
        "secret late update",
        "route-1",
        "-1009954455667",
        "recording-channel-key",
        "recording-reply-token",
        "77:fixture",
    ] {
        assert!(!logs.contains(secret), "leaked {secret}: {logs}");
    }
    let rendered = metrics.render();
    for expected in [
        "cmaeg_tool_sends_late_total{op=\"reply\",outcome=\"confirmed\"} 1",
        "cmaeg_tool_sends_late_total{op=\"reply\",outcome=\"rejected\"} 1",
        "cmaeg_tool_sends_late_total{op=\"update\",outcome=\"confirmed\"} 2",
    ] {
        assert!(rendered.contains(expected), "{rendered}");
    }
}
