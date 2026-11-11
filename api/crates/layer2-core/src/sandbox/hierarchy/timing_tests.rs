use super::*;
use std::{io::Write, sync::Mutex};

#[derive(Clone, Default)]
struct Output(Arc<Mutex<Vec<u8>>>);
impl Write for Output {
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

#[tokio::test]
async fn timings_are_opt_in_allowlisted_and_preserve_errors_and_cancellation() {
    use crate::sandbox::store::InMemorySandboxStore;
    let output = Output::default();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(output.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let service = HierarchyReadService::new(Arc::new(InMemorySandboxStore::default()));
    service.detail_phase(SandboxDetailPhase::Auth).complete();
    assert!(output.0.lock().unwrap().is_empty());
    let service = service.with_detail_timings(true);
    let span = tracing::info_span!("private_request", subject = "private-subject");
    let _entered = span.enter();
    for phase in [
        SandboxDetailPhase::Auth,
        SandboxDetailPhase::Hierarchy,
        SandboxDetailPhase::Colors,
        SandboxDetailPhase::Names,
        SandboxDetailPhase::FinalFence,
    ] {
        assert_eq!(
            service
                .detail_phase(phase)
                .result(async { Ok::<_, &str>(42) })
                .await,
            Ok(42)
        );
    }
    assert_eq!(
        service
            .detail_phase(SandboxDetailPhase::Hierarchy)
            .result(async { Err::<(), _>("private-error") })
            .await,
        Err("private-error")
    );
    let cancelled = tokio::time::timeout(
        std::time::Duration::from_millis(1),
        service
            .detail_phase(SandboxDetailPhase::FinalFence)
            .result(std::future::pending::<Result<(), ()>>()),
    );
    assert!(cancelled.await.is_err());
    let logs = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
    assert_eq!(logs.lines().count(), 7);
    for phase in ["auth", "hierarchy", "colors", "names", "final_fence"] {
        assert!(logs.contains(&format!("phase=\"{phase}\"")), "{logs}");
    }
    assert_eq!(logs.matches("outcome=\"completed\"").count(), 5);
    assert_eq!(logs.matches("outcome=\"error\"").count(), 1);
    assert_eq!(logs.matches("outcome=\"cancelled\"").count(), 1);
    assert_eq!(logs.matches("elapsed_us=").count(), 7);
    assert!(!logs.contains("private"));
}
