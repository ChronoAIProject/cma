//! Supervisor fault regressions adapted from the independent PR 1463 probes.
use super::*;
use crate::ledger_storage::fault_vfs::{Event, Vfs};
use crate::mutation_journal::Status;

#[path = "supervisor_ledger_recovery_fixture.rs"]
mod cases;
use cases::Case;

async fn recovery(case: Case, inject: bool, startup: bool) {
    let config = indexed_config();
    let (supervisor, ledger) = indexed(&config, 32_768).await;
    let operation = cases::prepare(case, &supervisor, &ledger).await;
    ledger.admit(2, vec![operation.clone()]).await.unwrap();
    ledger.begin(operation.clone()).await.unwrap();
    close_store(supervisor, ledger).await;

    let vfs = Vfs::new();
    let ledger = Arc::new(
        Ledger::open_fault_vfs(
            config.state_root.join("indexed-test"),
            config.clone(),
            vfs.name(),
        )
        .await
        .unwrap(),
    );
    let mut supervisor = Supervisor::new(config.clone(), SupervisorBoundary::default()).unwrap();
    Arc::get_mut(&mut supervisor).unwrap().mutations =
        Store::new(config.clone(), Some(Arc::clone(&ledger)));
    if inject {
        vfs.state.arm(Event::DatabaseWrite);
    }
    let outcome = if startup {
        Supervisor::finish_startup(Arc::clone(&supervisor))
            .await
            .map(|_| ())
    } else {
        match supervisor
            .dispatch(RequestFrame::Credential(CredentialFrame {
                operation_id: "credential-after-ledger-recovery".into(),
                value: RuntimeCredential::new("synthetic-recovery-value"),
                custody_generation: Some(1),
                github_route: None,
            }))
            .await
        {
            ResponseFrame::Credential(_) => Ok(()),
            ResponseFrame::Failure(failure) => Err(RuntimeError::recorded(
                failure.code,
                failure.detail,
                failure.retryable,
            )),
            other => panic!("unexpected response: {other:?}"),
        }
    };
    let events = vfs.state.events();
    let remaining = ledger.started_page(0, String::new()).await.unwrap();
    let record = ledger.test_record(operation.clone()).await.unwrap();
    let reserve = ledger.snapshot().await.unwrap().emergency_remaining_bytes;
    let credential_installed = supervisor.credential.current().await.is_some();
    let effect_preserved = cases::effect_preserved(case, &config);
    close_store(supervisor, ledger).await;
    cleanup(&config);

    assert!(
        effect_preserved,
        "{case:?}: expected effect state must survive failed receipt persistence"
    );
    if inject {
        assert!(events.contains(&Event::DatabaseWrite));
        assert_eq!(
            remaining.len(),
            1,
            "{case:?}: failed terminal commit must retain Started"
        );
        assert_eq!(remaining[0].operation(), operation);
        assert_eq!(record.status(), Status::Started);
        assert_eq!(reserve, 0);
        let error = outcome.expect_err("recovery must not swallow failed terminal persistence");
        assert_eq!(error.code, "mutation_ledger_storage_hold");
        assert!(!error.retryable);
        assert!(
            !credential_installed,
            "following credential effect must not execute"
        );
    } else {
        outcome.unwrap();
        assert!(remaining.is_empty());
        assert_eq!(record.status(), case.status());
        assert!(reserve > 0);
        assert_eq!(credential_installed, !startup);
    }
}

#[tokio::test]
async fn indexed_recovery_healthy_controls_record_terminal_results() {
    for case in Case::ALL {
        recovery(case, false, false).await;
    }
}

#[tokio::test]
async fn indexed_close_recovery_propagates_terminal_storage_hold() {
    recovery(Case::Close, true, false).await;
}

#[tokio::test]
async fn indexed_remove_worktree_recovery_propagates_terminal_storage_hold() {
    recovery(Case::Remove, true, false).await;
}

#[tokio::test]
async fn indexed_add_worktree_recovery_propagates_terminal_storage_hold() {
    recovery(Case::Add, true, false).await;
}

#[tokio::test]
async fn indexed_skill_recovery_propagates_terminal_storage_hold() {
    recovery(Case::Skill, true, false).await;
}

#[tokio::test]
async fn indexed_failed_recovery_requires_durable_terminal_result() {
    for case in [Case::AddFailed, Case::SkillFailed] {
        recovery(case, true, false).await;
    }
}

#[tokio::test]
async fn indexed_indeterminate_recovery_requires_durable_terminal_result() {
    recovery(Case::AddIndeterminate, true, false).await;
}

#[tokio::test]
async fn indexed_startup_recovery_propagates_terminal_storage_hold() {
    for case in Case::ALL {
        recovery(case, false, true).await;
        recovery(case, true, true).await;
    }
}
