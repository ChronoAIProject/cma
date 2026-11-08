use super::*;
use crate::config::RuntimeConfig;
use crate::mutation_journal::Operation;
use crate::mutation_store::{Begin, Store};
use layer3_adapter::{wire::AgentCloseReceipt, AgentCloseRequest, AgentSlot};

struct Fixture(RuntimeConfig);
impl Fixture {
    fn new() -> Self {
        Self(RuntimeConfig::for_test(
            std::env::temp_dir().canonicalize().unwrap().join(format!(
                "cma-recovery-confirmation-{}",
                uuid::Uuid::new_v4()
            )),
        ))
    }
    async fn ticket(&self, agent: &str) -> Ticket {
        let operation = Operation::close_agent(&AgentCloseRequest::new(
            format!("agent:{agent}:close:v1"),
            AgentSlot::new(agent),
        ));
        let store = Store::new(self.0.clone(), None);
        store
            .admit(1, std::slice::from_ref(&operation))
            .await
            .unwrap();
        match store.begin::<AgentCloseReceipt>(&operation).await.unwrap() {
            Begin::New(ticket) | Begin::Started(ticket) => ticket,
            other => panic!("expected pending ticket: {other:?}"),
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        for path in [&self.0.home, &self.0.state_root, &self.0.checkpoint_root] {
            let _ = std::fs::remove_dir_all(path);
        }
    }
}

#[tokio::test]
async fn uncommitted_ticket_preserves_ledger_format_worker_and_unknown_failures() {
    let fixture = Fixture::new();
    for code in [
        "mutation_ledger_storage_hold",
        "mutation_ledger_format_hold",
        "mutation_ledger_busy",
        "mutation_ledger_writer_held",
        "mutation_ledger_capacity",
        "mutation_ledger_invalid",
        "mutation_journal_corrupt",
        "unexpected_permanent_failure",
    ] {
        let (ticket, terminal) = fixture.ticket("pending").await.track_terminal();
        drop(ticket);
        let result: Result<()> = Err(RuntimeError::rejected(code, "fixed test detail"));
        let error = terminal.absorb(result).unwrap_err();
        assert_eq!(error.code, code);
        assert_eq!(error.detail, "fixed test detail");
        assert!(!error.retryable);
    }
    // Every prior attempt still returned this Started operation, with no receipt.
    drop(fixture.ticket("pending").await);
}

#[tokio::test]
async fn durable_failure_confirmation_belongs_only_to_its_exact_ticket() {
    let fixture = Fixture::new();
    let (finished, confirmed) = fixture.ticket("finished").await.track_terminal();
    let (pending, unconfirmed) = fixture.ticket("pending").await.track_terminal();
    let error = RuntimeError::rejected("effect_failed", "durable effect failure");
    finished.fail(&error).await.unwrap();
    confirmed.absorb::<()>(Err(error.clone())).unwrap();
    drop(pending);
    assert_eq!(
        unconfirmed.absorb::<()>(Err(error)).unwrap_err().code,
        "effect_failed"
    );
    drop(fixture.ticket("pending").await);
}

#[tokio::test]
async fn rejected_terminal_encoding_does_not_confirm_a_recovery_result() {
    let fixture = Fixture::new();
    let (ticket, terminal) = fixture.ticket("pending").await.track_terminal();
    let error = RuntimeError::unavailable("effect_retry", "not a terminal result");
    let failure = ticket.fail(&error).await.unwrap_err();
    let code = failure.code.clone();
    assert_eq!(terminal.absorb::<()>(Err(failure)).unwrap_err().code, code);
    drop(fixture.ticket("pending").await);
}
