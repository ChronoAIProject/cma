//! Exercise the unmodified predecessor reader, not the new folding validator.
use super::*;
use crate::mutation_authority as current;
use layer3_adapter::{WorktreeRef, WorktreeRequest};

#[test]
fn predecessor_reader_accepts_folded_high_water_identities_and_tombstones() {
    let config = RuntimeConfig::for_test(
        std::env::temp_dir().join(format!("cma-authority-v1-compat-{}", std::process::id())),
    );
    let _ = std::fs::remove_dir_all(&config.state_root);
    let add = Operation::add_worktree(&WorktreeRequest::new(
        "worktree:project:agent:add:v1",
        "project",
        "agent",
        "a".repeat(40),
    ));
    let remove = Operation::remove_worktree(&WorktreeRef::new(
        "worktree:project:agent:remove:v1",
        "project",
        "agent",
    ));
    for fence in [1, 2, 10, u64::MAX / 2] {
        current::admit(&config, fence, &add).unwrap();
    }
    let first_absent = u64::MAX / 2;
    for fence in [first_absent, first_absent + 1, u64::MAX] {
        current::admit(&config, fence, &remove).unwrap();
    }
    let old = records(&config).unwrap();
    assert_eq!(old.len(), 3);
    assert_eq!(
        old.iter()
            .filter(|r| r.disposition == Disposition::Present)
            .count(),
        1
    );
    assert!(old
        .iter()
        .any(|r| r.disposition == Disposition::Absent && r.authority_fence == first_absent));
    assert_eq!(recovery_fence(&config, &remove).unwrap(), Some(u64::MAX));
    assert_eq!(recovery_fence(&config, &add).unwrap(), None);
    assert_eq!(
        admit(&config, u64::MAX - 1, &remove).unwrap_err().code,
        "mutation_authority_stale"
    );
    assert_eq!(
        admit(&config, u64::MAX, &add).unwrap_err().code,
        "mutation_authority_retired"
    );
    // Old exact retries are readable too; rollback is not capacity prevention.
    admit(&config, u64::MAX, &remove).unwrap();
    std::fs::remove_dir_all(&config.state_root).unwrap();
}
