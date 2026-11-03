use layer3_adapter::{
    WorkspaceReceipt, WorkspaceRemoveReceipt, WorkspaceRemoveRequest, WorkspaceRequest,
    WorkspaceSource,
};

use super::*;
use crate::config::RuntimeConfig;

fn config(name: &str) -> RuntimeConfig {
    let home = std::env::temp_dir().join(format!(
        "cma-workspace-journal-{}-{name}",
        std::process::id()
    ));
    let config = RuntimeConfig::for_test(home);
    let _ = std::fs::remove_dir_all(&config.home);
    let _ = std::fs::remove_dir_all(&config.state_root);
    std::fs::create_dir_all(&config.home).unwrap();
    config
}

fn request() -> WorkspaceRequest {
    WorkspaceRequest::new(
        "workspace:wks_journal:prepare:v1",
        "wks_journal",
        "project",
        WorkspaceSource::Init,
    )
}

fn begin_ticket(config: &RuntimeConfig, request: &WorkspaceRequest) -> Ticket {
    match begin::<WorkspaceReceipt, _>(
        &config.state_root,
        &config.home,
        config.allow_local_git,
        &config.sandbox_id,
        &request.workspace_id,
        "prepare",
        &request.operation_id,
        &request.request_sha256,
        request,
    )
    .unwrap()
    {
        Begin::New(ticket) => ticket,
        _ => panic!("new operation did not create a journal ticket"),
    }
}

fn receipt(config: &RuntimeConfig, request: &WorkspaceRequest) -> WorkspaceReceipt {
    WorkspaceReceipt {
        operation_id: request.operation_id.clone(),
        request_sha256: request.request_sha256.clone(),
        workspace_id: request.workspace_id.clone(),
        name: request.name.clone(),
        path: config
            .home
            .join(&request.name)
            .to_string_lossy()
            .into_owned(),
        resolved_commit: "0".repeat(40),
        source_commit: None,
    }
}

fn scan(config: &RuntimeConfig) -> Result<Vec<Pending>> {
    started(
        &config.state_root,
        &config.sandbox_id,
        &config.home,
        config.allow_local_git,
    )
}

fn cleanup(config: &RuntimeConfig) {
    let _ = std::fs::remove_dir_all(&config.home);
    let _ = std::fs::remove_dir_all(&config.state_root);
}

#[test]
fn every_durable_removal_state_blocks_new_and_recovering_preparations() {
    for status in [
        Status::Started,
        Status::Completed,
        Status::Failed,
        Status::Indeterminate,
    ] {
        let config = config(&format!("remove-barrier-{status:?}"));
        let prepare = request();
        let prepare_ticket = begin_ticket(&config, &prepare);
        let prepare_path = prepare_ticket.path.clone();
        let remove = WorkspaceRemoveRequest::new(
            "workspace:wks_journal:remove:v1",
            "wks_journal",
            "project",
        );
        let Begin::New(ticket) = begin::<WorkspaceRemoveReceipt, _>(
            &config.state_root,
            &config.home,
            config.allow_local_git,
            &config.sandbox_id,
            &remove.workspace_id,
            "remove",
            &remove.operation_id,
            &remove.request_sha256,
            &remove,
        )
        .unwrap() else {
            panic!("expected a new removal intent")
        };
        match status {
            Status::Started => (),
            Status::Completed => ticket
                .complete(&WorkspaceRemoveReceipt {
                    operation_id: remove.operation_id,
                    request_sha256: remove.request_sha256,
                    workspace_id: remove.workspace_id,
                })
                .unwrap(),
            Status::Failed => ticket
                .fail(&RuntimeError::rejected(
                    "workspace_invalid",
                    "cannot inspect existing directory",
                ))
                .unwrap(),
            Status::Indeterminate => {
                ticket
                    .indeterminate("removal outcome needs recovery")
                    .unwrap();
            }
        }

        let pending = scan(&config).unwrap();
        assert!(pending.iter().all(|entry| entry.kind == "remove"));
        assert_eq!(pending.len(), usize::from(status == Status::Started));
        let stored = io::read_record(&prepare_path).unwrap().unwrap();
        assert_eq!(stored.status, Status::Failed);
        assert_eq!(stored.failure.unwrap().code, "workspace_removed");
        let blocked = begin::<WorkspaceReceipt, _>(
            &config.state_root,
            &config.home,
            config.allow_local_git,
            &config.sandbox_id,
            &prepare.workspace_id,
            "prepare",
            &prepare.operation_id,
            &prepare.request_sha256,
            &prepare,
        );
        assert!(
            matches!(blocked, Err(error) if error.code == "workspace_removed" && !error.retryable)
        );
        let successor = WorkspaceRequest::new(
            "workspace:wks_next:prepare:v1",
            "wks_next",
            "project",
            WorkspaceSource::Init,
        );
        let _ = begin_ticket(&config, &successor);
        cleanup(&config);
    }
}

#[test]
fn journal_scan_rejects_a_valid_record_under_a_renamed_or_duplicate_filename() {
    let config = config("renamed");
    let request = request();
    let ticket = begin_ticket(&config, &request);
    let renamed = ticket
        .path
        .with_file_name(format!("{}.json", "f".repeat(64)));
    std::fs::rename(&ticket.path, &renamed).unwrap();

    let error = scan(&config).unwrap_err();
    assert_eq!(error.code, "workspace_journal_corrupt");
    assert!(error.detail.contains("not canonical"));
    cleanup(&config);
}

#[cfg(unix)]
#[test]
fn journal_scan_never_follows_a_canonical_entry_symlink() {
    let config = config("no-follow");
    let request = request();
    let ticket = begin_ticket(&config, &request);
    let outside = config.state_root.join("outside.json");
    std::fs::rename(&ticket.path, &outside).unwrap();
    std::os::unix::fs::symlink(&outside, &ticket.path).unwrap();

    let error = scan(&config).unwrap_err();
    assert!(matches!(
        error.code.as_str(),
        "workspace_journal_corrupt" | "workspace_journal_io_failed"
    ));
    cleanup(&config);
}

#[test]
fn durable_terminal_temp_is_promoted_after_a_crash_before_rename() {
    let config = config("temp-recovery");
    let request = request();
    let ticket = begin_ticket(&config, &request);
    let mut terminal = ticket.record.clone();
    terminal.status = Status::Completed;
    terminal.receipt = Some(serde_json::to_value(receipt(&config, &request)).unwrap());
    let temporary = ticket.path.with_file_name(".workspace-journal-crash.tmp");
    std::fs::write(&temporary, encode_record(&terminal).unwrap()).unwrap();

    assert!(scan(&config).unwrap().is_empty());
    assert!(!temporary.exists());
    let replay = begin::<WorkspaceReceipt, _>(
        &config.state_root,
        &config.home,
        config.allow_local_git,
        &config.sandbox_id,
        &request.workspace_id,
        "prepare",
        &request.operation_id,
        &request.request_sha256,
        &request,
    )
    .unwrap();
    assert!(matches!(replay, Begin::Completed(found) if found == receipt(&config, &request)));
    cleanup(&config);
}

#[test]
fn semantic_receipt_corruption_is_refused_before_replay() {
    let config = config("receipt-corrupt");
    let request = request();
    let ticket = begin_ticket(&config, &request);
    let mut wrong = receipt(&config, &request);
    wrong.path = config
        .home
        .join("another-workspace")
        .to_string_lossy()
        .into_owned();
    ticket.complete(&wrong).unwrap();

    let error = scan(&config).unwrap_err();
    assert_eq!(error.code, "workspace_journal_corrupt");
    assert!(error.detail.contains("request or receipt"));
    cleanup(&config);
}

#[test]
fn retryable_failure_cannot_be_frozen_as_a_terminal_record() {
    let config = config("retryable");
    let request = request();
    let ticket = begin_ticket(&config, &request);
    let error = ticket
        .fail(&RuntimeError::unavailable(
            "workspace_io_failed",
            "temporary filesystem failure",
        ))
        .unwrap_err();
    assert_eq!(error.code, "workspace_journal_corrupt");
    assert_eq!(scan(&config).unwrap().len(), 1);
    cleanup(&config);
}

#[test]
fn scan_stops_at_the_directory_entry_bound() {
    let config = config("entry-bound");
    let request = request();
    let ticket = begin_ticket(&config, &request);
    let bytes = std::fs::read(&ticket.path).unwrap();
    let directory = ticket.path.parent().unwrap();
    for index in 0..MAX_JOURNAL_ENTRIES {
        std::fs::write(
            directory.join(format!(".workspace-journal-{index}.tmp")),
            &bytes,
        )
        .unwrap();
    }
    let error = scan(&config).unwrap_err();
    assert_eq!(error.code, "workspace_journal_corrupt");
    assert!(error.detail.contains("too many entries"));
    cleanup(&config);
}
