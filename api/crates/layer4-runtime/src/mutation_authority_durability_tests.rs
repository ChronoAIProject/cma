use super::*;
use crate::mutation_journal::{self as journal, Begin};
use layer3_adapter::wire::AgentCloseReceipt;
use layer3_adapter::{AgentCloseRequest, AgentSlot};
use std::path::{Path, PathBuf};

fn retained(home: PathBuf) -> RuntimeConfig {
    let mut config = RuntimeConfig::for_test(home);
    config.home_identity = Some("a".repeat(43));
    config
}

fn close(agent: &str) -> Operation {
    Operation::close_agent(&AgentCloseRequest::new(
        format!("agent:{agent}:close:v1"),
        AgentSlot::new(agent),
    ))
}

fn authority_path(config: &RuntimeConfig, operation: &Operation) -> PathBuf {
    io::record_path(
        &crate::mutation_scope::root(config, &operation.subject),
        &Record::new(config, 1, operation),
    )
}

fn interrupted_prune(name: &str) -> RuntimeConfig {
    let config = retained(config(name).home);
    let _ = std::fs::remove_dir_all(&config.home);
    std::fs::create_dir_all(&config.home).unwrap();
    crate::persistent_home::initialize(&config).unwrap();
    for fence in 1..=4094 {
        seed(&config, fence, &add());
    }
    seed(&config, 1, &close("old-process"));
    crate::fs_durability::sync_tree(&config.state_root).unwrap();
    assert_eq!(records(&config).unwrap().len(), 4095);

    // The fault seam observes only fold directory barriers. Survivor file and
    // directory syncs still execute normally before this final barrier fails.
    let directory = config.state_root.join("mutation-authority");
    let _fault = fold::fault::arm(
        Some((directory.clone(), 1)),
        authority_path(&config, &close("new-process")),
    );
    let error = admit(&config, 4095, &add()).unwrap_err();
    assert_eq!(error.code, "mutation_authority_unavailable");
    assert_eq!(records(&config).unwrap().len(), 2);
    assert_eq!(recovery_fence(&config, &add()).unwrap(), Some(4094));
    let events = fold::fault::events();
    assert!(events
        .iter()
        .any(|event| event.path == directory && !event.succeeded));
    config
}

fn verify_cross_root_retry(config: &RuntimeConfig) {
    let directory = config.state_root.join("mutation-authority");
    let operation = close("new-process");
    let new_path = authority_path(config, &operation);
    let process_directory = new_path.parent().unwrap();
    let before = records(config).unwrap();
    {
        // Even after the visible deletions, a failed retry barrier holds the
        // other root's new authority and receipt before either is admitted.
        let _fault = fold::fault::arm(Some((directory.clone(), 1)), new_path.clone());
        let error = admit(config, 1, &operation).unwrap_err();
        assert_eq!(error.code, "mutation_authority_unavailable");
        assert!(!new_path.exists());
        assert!(!journal::admitted(config, &operation).unwrap());
        assert_eq!(records(config).unwrap(), before);
        let events = fold::fault::events();
        assert!(events
            .iter()
            .any(|event| event.path == directory && !event.succeeded));
        assert!(events.iter().all(|event| !event.watched_exists));
    }
    {
        let _fault = fold::fault::arm(None, new_path.clone());
        admit(config, 1, &operation).unwrap();
        let Begin::New(ticket) = journal::begin::<AgentCloseReceipt>(config, &operation).unwrap()
        else {
            panic!("new cleanup must have its own journal");
        };
        let receipt = AgentCloseReceipt {
            operation_id: operation.operation_id.clone(),
            request_sha256: operation.request_sha256.clone(),
            agent_id: AgentSlot::new("new-process"),
        };
        ticket.complete(&receipt).unwrap();
        let Begin::Completed(replayed) =
            journal::begin::<AgentCloseReceipt>(config, &operation).unwrap()
        else {
            panic!("the acknowledged cleanup must replay");
        };
        assert_eq!(replayed, receipt);
        let events = fold::fault::events();
        for expected in [directory.as_path(), process_directory] {
            let first = events
                .iter()
                .find(|event| event.path == *expected)
                .expect("every populated authority root needs a barrier");
            assert!(first.succeeded, "the directory must actually sync");
            assert!(
                !first.watched_exists,
                "the barrier must precede other-root publication"
            );
        }
        assert!(events.iter().any(|event| event.watched_exists));
        assert_eq!(records(config).unwrap().len(), 3);
        assert!(journal::started(config).unwrap().is_empty());
    }
}

fn cleanup(config: &RuntimeConfig) {
    std::fs::remove_dir_all(&config.state_root).unwrap();
    std::fs::remove_dir_all(&config.home).unwrap();
}

#[test]
fn final_sync_failure_holds_other_root_until_its_deletions_are_durable() {
    struct Fixture(RuntimeConfig);
    impl Drop for Fixture {
        fn drop(&mut self) {
            cleanup(&self.0);
        }
    }
    let fixture = Fixture(interrupted_prune("cross-root-directory-barrier"));
    verify_cross_root_retry(&fixture.0);
}

#[test]
fn interrupted_prune_restart_restores_barriers_before_other_root_receipt() {
    const HOME_ENV: &str = "CMA_TEST_AUTHORITY_RESTART_HOME";
    if let Some(home) = std::env::var_os(HOME_ENV) {
        let config = retained(Path::new(&home).to_path_buf());
        crate::persistent_home::verify(&config).unwrap();
        assert_eq!(records(&config).unwrap().len(), 2);
        verify_cross_root_retry(&config);
        return;
    }
    let config = interrupted_prune("restart-directory-barrier");
    // A fresh OS process has no knowledge of the failed prune. It must derive
    // the barriers from the retained records, not a remembered failure flag.
    let test_name = format!(
        "{}::interrupted_prune_restart_restores_barriers_before_other_root_receipt",
        module_path!().split_once("::").unwrap().1
    );
    let result = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", &test_name, "--test-threads=1"])
        .env(HOME_ENV, &config.home)
        .output()
        .unwrap();
    cleanup(&config);
    assert!(
        result.status.success(),
        "child failed: {}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(
        String::from_utf8_lossy(&result.stdout).contains("1 passed"),
        "child must execute the restart test"
    );
}
