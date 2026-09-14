use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use layer3_adapter::{WorktreeRef, WorktreeRequest};

use super::*;

fn config(name: &str) -> RuntimeConfig {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    RuntimeConfig::for_test(std::env::temp_dir().join(format!(
        "cma-mutation-authority-{}-{name}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )))
}

fn cleanup(config: &RuntimeConfig) {
    let _ = std::fs::remove_dir_all(&config.home);
    let _ = std::fs::remove_dir_all(&config.state_root);
    let _ = std::fs::remove_dir_all(&config.checkpoint_root);
}

fn add() -> Operation {
    Operation::add_worktree(&WorktreeRequest::new(
        "worktree:workspace:agent:add:v1",
        "workspace",
        "agent",
        "a".repeat(40),
    ))
}

fn remove() -> Operation {
    Operation::remove_worktree(&WorktreeRef::new(
        "worktree:workspace:agent:remove:v1",
        "workspace",
        "agent",
    ))
}

#[test]
fn cleanup_is_terminal_and_dominates_create_at_the_same_fence() {
    let config = config("cleanup-dominates");
    admit(&config, 10, &add()).unwrap();
    admit(&config, 10, &remove()).unwrap();

    let retired = admit(&config, 10, &add()).unwrap_err();
    assert_eq!(retired.code, "mutation_authority_retired");
    let newer = admit(&config, 11, &add()).unwrap_err();
    assert_eq!(newer.code, "mutation_authority_retired");
    assert_eq!(recovery_fence(&config, &add()).unwrap(), None);
    assert_eq!(recovery_fence(&config, &remove()).unwrap(), Some(10));
    cleanup(&config);
}

#[test]
fn exact_retries_advance_the_durable_fence_but_stale_authority_is_rejected() {
    let config = config("exact-retry");
    admit(&config, 7, &add()).unwrap();
    admit(&config, 9, &add()).unwrap();
    assert_eq!(recovery_fence(&config, &add()).unwrap(), Some(9));

    let stale = admit(&config, 8, &remove()).unwrap_err();
    assert_eq!(stale.code, "mutation_authority_stale");
    admit(&config, 9, &remove()).unwrap();
    admit(&config, 12, &remove()).unwrap();
    assert_eq!(recovery_fence(&config, &remove()).unwrap(), Some(12));
    cleanup(&config);
}

#[test]
fn authority_survives_reopening_the_same_runtime_state_root() {
    let config = config("restart");
    admit(&config, 20, &remove()).unwrap();

    let reopened = config.clone();
    assert_eq!(recovery_fence(&reopened, &remove()).unwrap(), Some(20));
    let stale = admit(&reopened, 19, &add()).unwrap_err();
    assert_eq!(stale.code, "mutation_authority_retired");
    cleanup(&config);
}

#[test]
fn zero_fences_and_changed_operation_identity_fail_closed() {
    let config = config("invalid");
    let zero = admit(&config, 0, &add()).unwrap_err();
    assert_eq!(zero.code, "mutation_authority_invalid");

    admit(&config, 1, &add()).unwrap();
    let changed = Operation::add_worktree(&WorktreeRequest::new(
        "worktree:workspace:agent:add:v1",
        "workspace",
        "agent",
        "b".repeat(40),
    ));
    let conflict = admit(&config, 2, &changed).unwrap_err();
    assert_eq!(conflict.code, "mutation_authority_conflict");
    cleanup(&config);
}

#[test]
fn incomplete_temporary_records_never_grant_authority() {
    let config = config("temporary");
    let directory = config.state_root.join("mutation-authority");
    std::fs::create_dir_all(&directory).unwrap();
    let temporary = directory.join(".authority-123-1.tmp");
    std::fs::write(&temporary, b"partial").unwrap();

    assert_eq!(recovery_fence(&config, &add()).unwrap(), None);
    assert!(!temporary.exists());
    cleanup(&config);
}

#[test]
fn corrupt_canonical_records_fence_the_runtime() {
    let config = config("corrupt");
    let directory: PathBuf = config.state_root.join("mutation-authority");
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join(format!("{}.json", "a".repeat(64))), b"{").unwrap();

    let error = recovery_fence(&config, &add()).unwrap_err();
    assert_eq!(error.code, "mutation_authority_corrupt");
    cleanup(&config);
}
