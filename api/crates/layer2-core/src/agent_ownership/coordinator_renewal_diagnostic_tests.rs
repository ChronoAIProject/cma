use super::*;
use std::sync::Mutex as StdMutex;

#[derive(Clone, Default)]
struct Output(Arc<StdMutex<Vec<u8>>>);
impl std::io::Write for Output {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Output {
    type Writer = Self;
    fn make_writer(&'a self) -> Self {
        self.clone()
    }
}

#[tokio::test(start_paused = true)]
async fn containment_reports_one_typed_transition_without_raw_error_or_identity() {
    for timeout in [false, true] {
        let fixture = Fixture::new(2).await;
        if timeout {
            *fixture.store.delay.lock().unwrap() = Duration::from_secs(10);
        }
        let output = Output::default();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .without_time()
            .with_writer(output.clone())
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);
        fixture.manager.maintain().await.unwrap();
        fixture.manager.maintain().await.unwrap();
        let captured = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
        assert_eq!(captured.lines().count(), 1, "{captured}");
        assert!(captured.contains("WARN"));
        assert!(captured.contains("scope=\"agent\""));
        assert!(captured.contains(if timeout {
            "renewal_call_timeout"
        } else {
            "renewal_store_unavailable"
        }));
        assert!(captured.contains("cancelled_while_live"));
        assert!(captured.contains("agent_sha256="));
        assert!(!captured.contains("agent-000"));
        assert!(!captured.contains("renewal-member"));
        assert!(!captured.contains("synthetic unavailable detail"));
    }
}

#[tokio::test(start_paused = true)]
async fn process_failure_reports_stage_and_kind_without_raw_driver_details() {
    let fixture = Fixture::new(2).await;
    *fixture.store.reply.lock().unwrap() = Reply::Error(OwnershipError::Invalid(
        "synthetic private error canary".into(),
    ));
    let output = Output::default();
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .without_time()
        .with_writer(output.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    assert!(fixture.manager.maintain().await.is_err());
    fixture.assert_failed();
    let captured = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
    assert!(captured.contains("ERROR"));
    assert!(captured.contains("scope=\"process\""));
    assert!(captured.contains("owner_store_renew"));
    assert!(captured.contains("reason=\"invalid\""));
    assert!(!captured.contains("synthetic private error canary"));
}

#[tokio::test(start_paused = true)]
async fn skipped_heartbeat_reports_one_bounded_typed_warning_per_tick() {
    for (mode, reason) in [
        (1, "heartbeat_call_timeout"),
        (7, "heartbeat_store_unavailable"),
    ] {
        let fixture = Fixture::new(2).await;
        *fixture.store.reply.lock().unwrap() = Reply::Renew;
        let output = Output::default();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .without_time()
            .with_writer(output.clone())
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);
        fixture.store.heartbeat_mode.store(mode, Ordering::SeqCst);
        fixture.manager.maintain().await.unwrap();
        fixture.manager.maintain().await.unwrap();
        fixture.store.heartbeat_mode.store(0, Ordering::SeqCst);
        fixture.manager.maintain().await.unwrap();
        fixture.store.heartbeat_mode.store(mode, Ordering::SeqCst);
        fixture.manager.maintain().await.unwrap();
        let captured = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
        let lines: Vec<_> = captured.lines().collect();
        assert_eq!(lines.len(), 3, "{captured}");
        for (line, consecutive) in lines.iter().zip([1, 2, 1]) {
            assert!(line.contains("WARN"), "{line}");
            assert!(line.contains("stage=\"member_heartbeat\""), "{line}");
            assert!(line.contains("scope=\"process\""), "{line}");
            assert!(line.contains(&format!("reason=\"{reason}\"")), "{line}");
            assert!(line.contains("outcome=\"skipped_tick\""), "{line}");
            assert!(
                line.contains(&format!("consecutive={consecutive} ")),
                "{line}"
            );
            assert!(line.contains("member_remaining_ms="), "{line}");
            assert!(line.contains("instance_sha256="), "{line}");
            assert!(!line.contains("renewal-member"), "{line}");
            assert!(!line.contains("heartbeat not committed"), "{line}");
            assert!(!line.contains("exceeded its deadline"), "{line}");
        }
        assert!(!captured.contains("ERROR"), "{captured}");
        assert!(fixture.manager.is_ready());
    }
}
