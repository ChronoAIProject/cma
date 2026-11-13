use super::*;
use std::io::Write;
use std::sync::Mutex;

#[derive(Clone, Default)]
struct Logs(Arc<Mutex<Vec<u8>>>);
impl Write for Logs {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Logs {
    type Writer = Self;
    fn make_writer(&'a self) -> Self {
        self.clone()
    }
}

#[tokio::test]
async fn preparation_diagnostics_are_bounded_and_never_format_private_errors_or_commands() {
    let (store, agent, parent) = fixture(AgentStatus::Paused).await;
    let command = admit(
        &store,
        &agent,
        &parent,
        "diagnostics",
        AgentCommand::message("private-input-marker"),
    )
    .await;
    let logs = Logs::default();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(logs.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let result = observe_command_preparation(
        &command,
        "prerequisite",
        async { Err::<(), _>("private-upstream-marker") },
        |_| "unavailable",
    )
    .await;
    assert_eq!(result, Err("private-upstream-marker"));
    let result = tokio::time::timeout(
        Duration::from_millis(1),
        observe_command_preparation(
            &command,
            "scheduling",
            std::future::pending::<Result<(), ()>>(),
            |_| "unused",
        ),
    )
    .await;
    assert!(result.is_err());
    observe_command_preparation(
        &command,
        "success_boundary",
        async { Ok::<_, ()>(()) },
        |_| "unused",
    )
    .await
    .unwrap();
    let output = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
    assert_eq!(output.lines().count(), 2);
    for expected in [
        command.receipt.envelope.command_id.as_str(),
        agent.id.as_str(),
        "prerequisite",
        "unavailable",
        "scheduling",
        "cancelled",
    ] {
        assert!(output.contains(expected), "missing {expected}");
    }
    for private in [
        "private-input-marker",
        "private-upstream-marker",
        "success_boundary",
    ] {
        assert!(!output.contains(private));
    }
}

#[tokio::test]
async fn prerequisite_refusals_log_precise_causes_but_record_only_legacy_codes() {
    for (code, cause) in [
        (
            CommandFailureCode::CredentialSetupRequired,
            "CredentialSetupRequired",
        ),
        (
            CommandFailureCode::ConnectionsRequired,
            "ConnectionsRequired",
        ),
    ] {
        let (store, agent, parent) = fixture(AgentStatus::Paused).await;
        let mut command = admit(
            &store,
            &agent,
            &parent,
            "refusal",
            AgentCommand::message("private-input-marker"),
        )
        .await;
        let logs = Logs::default();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_writer(logs.clone())
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);
        command
            .reject_unclaimed(command.revision, code, crate::now_rfc3339())
            .unwrap();
        let output = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
        assert_eq!(output.lines().count(), 1);
        assert!(output.contains("event=\"command_refusal\""));
        assert!(output.contains(&format!("error_kind={cause}")));
        assert!(output.contains(command.receipt.envelope.command_id.as_str()));
        assert!(!output.contains("private-input-marker"));
        assert_eq!(
            serde_json::to_value(&command).unwrap()["work"]["failure"]["code"],
            "execution_failed"
        );
        assert_eq!(command.status, AgentResponseStatus::Failed);
        assert!(command.work.dispatch.is_none());
    }
}
