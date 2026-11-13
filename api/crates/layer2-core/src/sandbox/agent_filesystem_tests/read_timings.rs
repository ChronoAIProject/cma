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
async fn file_read_timings_are_opt_in_correlated_and_do_not_log_private_data() {
    let (service, _, owner) = fixture().await;
    let output = Output::default();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(output.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let id = AgentId("agt_test".into());
    service
        .read(
            &owner.caller,
            Some(&owner),
            None,
            &id,
            "private-file.txt",
            1024,
        )
        .await
        .unwrap();
    assert!(!String::from_utf8(output.0.lock().unwrap().clone())
        .unwrap()
        .contains("agent file read phase"));
    output.0.lock().unwrap().clear();
    let service = service.with_read_timings(true);
    let diagnostic = crate::diagnostic_context::DiagnosticContext {
        route: "GET /api/v1/agents/{id}/files/content".into(),
        request_hash: "opaque-test-hash".into(),
        ..Default::default()
    };
    diagnostic
        .scope(async {
            service
                .read(
                    &owner.caller,
                    Some(&owner),
                    None,
                    &id,
                    "private-file.txt",
                    1024,
                )
                .await
                .unwrap();
            let stranger = context("private-stranger");
            assert!(service
                .read(
                    &stranger.caller,
                    Some(&stranger),
                    None,
                    &id,
                    "private-file.txt",
                    1024
                )
                .await
                .is_err());
        })
        .await;
    let logs = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
    let phases = logs
        .lines()
        .filter(|line| line.contains("agent file read phase"))
        .collect::<Vec<_>>();
    assert_eq!(phases.len(), 3, "{logs}");
    assert_eq!(
        phases
            .iter()
            .filter(|line| line.contains("outcome=\"error\""))
            .count(),
        1
    );
    assert!(phases
        .iter()
        .any(|line| line.contains("phase=\"final_fence\"")));
    for phase in phases {
        assert!(phase.contains("opaque-test-hash"));
        assert!(phase.contains("elapsed_us="));
        assert!(!phase.contains("private-"));
        assert!(!phase.contains("agt_test"));
    }
}
