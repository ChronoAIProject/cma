use layer3_adapter::wire::{
    AgentCloseReceipt, AgentHomeReceipt, AgentHomeRemoveReceipt, AgentOpenReceipt,
};
use layer3_adapter::{
    AgentCloseRequest, AgentHomePlan, AgentHomeRemoveRequest, AgentLaunch, AgentSlot, HomeFile,
    WorktreeReceipt, WorktreeRef, WorktreeRemoveReceipt, WorktreeRequest,
};

use super::*;

fn config(name: &str) -> RuntimeConfig {
    let home = std::env::temp_dir().join(format!(
        "cma-mutation-journal-{}-{name}",
        std::process::id()
    ));
    let config = RuntimeConfig::for_test(home);
    let _ = std::fs::remove_dir_all(&config.home);
    let _ = std::fs::remove_dir_all(&config.state_root);
    std::fs::create_dir_all(&config.home).unwrap();
    config
}

fn request() -> WorktreeRequest {
    WorktreeRequest::new(
        "worktree:project:agent-1:add:v1",
        "project",
        "agent-1",
        "0".repeat(40),
    )
}

fn begin_ticket<T: serde::de::DeserializeOwned>(
    config: &RuntimeConfig,
    operation: &Operation,
) -> Ticket {
    match begin::<T>(config, operation).unwrap() {
        Begin::New(ticket) => ticket,
        _ => panic!("new runtime mutation did not create a journal ticket"),
    }
}

fn receipt(config: &RuntimeConfig, request: &WorktreeRequest) -> WorktreeReceipt {
    WorktreeReceipt {
        operation_id: request.operation_id.clone(),
        request_sha256: request.request_sha256.clone(),
        workspace: request.workspace.clone(),
        name: request.name.clone(),
        path: crate::worktree::expected_path(&config.home, &request.workspace, &request.name)
            .to_string_lossy()
            .into_owned(),
        branch: format!("cma/{}", request.name),
        base_commit: "0".repeat(40),
    }
}

fn cleanup(config: &RuntimeConfig) {
    let _ = std::fs::remove_dir_all(&config.home);
    let _ = std::fs::remove_dir_all(&config.state_root);
}

#[test]
fn exact_completed_replay_succeeds_and_changed_payload_is_refused() {
    let config = config("replay");
    let request = request();
    let operation = Operation::add_worktree(&request);
    begin_ticket::<WorktreeReceipt>(&config, &operation)
        .complete(&receipt(&config, &request))
        .unwrap();
    assert!(matches!(
        begin::<WorktreeReceipt>(&config, &operation).unwrap(),
        Begin::Completed(found) if found == receipt(&config, &request)
    ));

    let mut changed_request = request.clone();
    changed_request.base = "f".repeat(40);
    changed_request.request_sha256 = changed_request.computed_sha256();
    let changed = Operation::add_worktree(&changed_request);
    let error = begin::<WorktreeReceipt>(&config, &changed).unwrap_err();
    assert_eq!(error.code, "runtime_mutation_replay_mismatch");
    cleanup(&config);
}

#[test]
fn journal_scan_rejects_renamed_entries() {
    let config = config("renamed");
    let operation = Operation::add_worktree(&request());
    let ticket = begin_ticket::<WorktreeReceipt>(&config, &operation);
    let renamed = ticket
        .path
        .with_file_name(format!("{}.json", "f".repeat(64)));
    std::fs::rename(&ticket.path, renamed).unwrap();
    let error = validate_all(&config).unwrap_err();
    assert_eq!(error.code, "mutation_journal_corrupt");
    assert!(error.detail.contains("canonical"));
    cleanup(&config);
}

#[cfg(unix)]
#[test]
fn journal_scan_never_follows_a_canonical_symlink() {
    let config = config("no-follow");
    let operation = Operation::add_worktree(&request());
    let ticket = begin_ticket::<WorktreeReceipt>(&config, &operation);
    let outside = config.state_root.join("outside.json");
    std::fs::rename(&ticket.path, &outside).unwrap();
    std::os::unix::fs::symlink(&outside, &ticket.path).unwrap();
    let error = validate_all(&config).unwrap_err();
    assert!(matches!(
        error.code.as_str(),
        "mutation_journal_corrupt" | "mutation_journal_io_failed"
    ));
    cleanup(&config);
}

#[test]
fn durable_terminal_temp_is_promoted_after_a_crash_before_rename() {
    let config = config("temp-recovery");
    let request = request();
    let operation = Operation::add_worktree(&request);
    let ticket = begin_ticket::<WorktreeReceipt>(&config, &operation);
    let mut terminal = ticket.record.clone();
    terminal.status = Status::Completed;
    terminal.receipt = Some(serde_json::to_value(receipt(&config, &request)).unwrap());
    let temporary = ticket.path.with_file_name(".mutation-journal-crash.tmp");
    std::fs::write(&temporary, encode_record(&terminal).unwrap()).unwrap();

    assert!(started(&config).unwrap().is_empty());
    assert!(!temporary.exists());
    assert!(matches!(
        begin::<WorktreeReceipt>(&config, &operation).unwrap(),
        Begin::Completed(found) if found == receipt(&config, &request)
    ));
    cleanup(&config);
}

#[test]
fn semantic_receipt_corruption_is_refused_before_replay() {
    let config = config("receipt-corrupt");
    let request = request();
    let operation = Operation::add_worktree(&request);
    let mut wrong = receipt(&config, &request);
    wrong.path.push_str("-other");
    begin_ticket::<WorktreeReceipt>(&config, &operation)
        .complete(&wrong)
        .unwrap();
    let error = validate_all(&config).unwrap_err();
    assert_eq!(error.code, "mutation_journal_corrupt");
    assert!(error.detail.contains("receipt"));
    cleanup(&config);
}

#[test]
fn scan_stops_at_the_directory_entry_bound() {
    let config = config("entry-bound");
    let operation = Operation::add_worktree(&request());
    let ticket = begin_ticket::<WorktreeReceipt>(&config, &operation);
    let bytes = std::fs::read(&ticket.path).unwrap();
    let directory = ticket.path.parent().unwrap();
    for index in 0..MAX_JOURNAL_ENTRIES {
        std::fs::write(
            directory.join(format!(".mutation-journal-{index}.tmp")),
            &bytes,
        )
        .unwrap();
    }
    let error = validate_all(&config).unwrap_err();
    assert_eq!(error.code, "mutation_ledger_capacity");
    assert_eq!(error.detail, crate::mutation_capacity::DETAIL);
    cleanup(&config);
}

#[test]
fn every_typed_terminal_receipt_replays_exactly() {
    let config = config("all-receipts-replay");
    let add = request();
    let add_receipt = receipt(&config, &add);
    complete_and_replay(&config, Operation::add_worktree(&add), &add_receipt);

    let remove = WorktreeRef::new("worktree:project:agent-1:remove:v1", "project", "agent-1");
    let remove_receipt = WorktreeRemoveReceipt {
        operation_id: remove.operation_id.clone(),
        request_sha256: remove.request_sha256.clone(),
        workspace: remove.workspace.clone(),
        name: remove.name.clone(),
    };
    complete_and_replay(
        &config,
        Operation::remove_worktree(&remove),
        &remove_receipt,
    );

    let plan = home_plan();
    let home_receipt = AgentHomeReceipt {
        operation_id: plan.operation_id.clone(),
        request_sha256: plan.request_sha256.clone(),
        agent_id: plan.agent_id.clone(),
        path: config
            .agent_home(plan.agent_id.as_str())
            .to_string_lossy()
            .into_owned(),
    };
    complete_and_replay(&config, Operation::create_agent_home(&plan), &home_receipt);

    let launch = launch(&config);
    let open_receipt = AgentOpenReceipt {
        operation_id: launch.operation_id.clone(),
        request_sha256: launch.request_sha256.clone(),
        agent_id: launch.agent_id.clone(),
        cwd: launch.cwd.clone(),
        home: launch.home.clone(),
        conversation_id: None,
    };
    complete_and_replay(&config, Operation::open_agent(&launch), &open_receipt);

    let close = AgentCloseRequest::new("agent:agent-1:close:v1", AgentSlot::new("agent-1"));
    let close_receipt = AgentCloseReceipt {
        operation_id: close.operation_id.clone(),
        request_sha256: close.request_sha256.clone(),
        agent_id: close.agent_id.clone(),
    };
    complete_and_replay(&config, Operation::close_agent(&close), &close_receipt);

    let remove_home =
        AgentHomeRemoveRequest::new("agent:agent-1:home:remove:v1", AgentSlot::new("agent-1"));
    let remove_home_receipt = AgentHomeRemoveReceipt {
        operation_id: remove_home.operation_id.clone(),
        request_sha256: remove_home.request_sha256.clone(),
        agent_id: remove_home.agent_id.clone(),
    };
    complete_and_replay(
        &config,
        Operation::remove_agent_home(&remove_home),
        &remove_home_receipt,
    );
    cleanup(&config);
}

#[test]
fn changed_rendered_home_and_launch_payloads_are_refused() {
    let config = config("rendered-payload-replay");
    let plan = home_plan();
    let operation = Operation::create_agent_home(&plan);
    let home_receipt = AgentHomeReceipt {
        operation_id: plan.operation_id.clone(),
        request_sha256: plan.request_sha256.clone(),
        agent_id: plan.agent_id.clone(),
        path: config
            .agent_home(plan.agent_id.as_str())
            .to_string_lossy()
            .into_owned(),
    };
    begin_ticket::<AgentHomeReceipt>(&config, &operation)
        .complete(&home_receipt)
        .unwrap();
    let mut changed_plan = plan;
    changed_plan.files[0].bytes = b"changed".to_vec();
    let error = begin::<AgentHomeReceipt>(&config, &Operation::create_agent_home(&changed_plan))
        .unwrap_err();
    assert_eq!(error.code, "runtime_mutation_replay_mismatch");

    let launch = launch(&config);
    let operation = Operation::open_agent(&launch);
    let open_receipt = AgentOpenReceipt {
        operation_id: launch.operation_id.clone(),
        request_sha256: launch.request_sha256.clone(),
        agent_id: launch.agent_id.clone(),
        cwd: launch.cwd.clone(),
        home: launch.home.clone(),
        conversation_id: None,
    };
    begin_ticket::<AgentOpenReceipt>(&config, &operation)
        .complete(&open_receipt)
        .unwrap();
    let mut changed_launch = launch;
    changed_launch.args.push("changed".into());
    let error =
        begin::<AgentOpenReceipt>(&config, &Operation::open_agent(&changed_launch)).unwrap_err();
    assert_eq!(error.code, "runtime_mutation_replay_mismatch");
    cleanup(&config);
}

#[test]
fn semantic_worktree_remove_receipt_corruption_is_refused() {
    let config = config("remove-receipt-corrupt");
    let request = WorktreeRef::new("worktree:project:agent-1:remove:v1", "project", "agent-1");
    let operation = Operation::remove_worktree(&request);
    begin_ticket::<WorktreeRemoveReceipt>(&config, &operation)
        .complete(&WorktreeRemoveReceipt {
            operation_id: request.operation_id,
            request_sha256: request.request_sha256,
            workspace: "other".into(),
            name: request.name,
        })
        .unwrap();
    assert_receipt_corrupt(&config);
    cleanup(&config);
}

#[test]
fn semantic_agent_home_receipt_corruption_is_refused() {
    let config = config("home-receipt-corrupt");
    let plan = home_plan();
    let operation = Operation::create_agent_home(&plan);
    begin_ticket::<AgentHomeReceipt>(&config, &operation)
        .complete(&AgentHomeReceipt {
            operation_id: plan.operation_id,
            request_sha256: plan.request_sha256,
            agent_id: plan.agent_id,
            path: config
                .home
                .join("wrong-home")
                .to_string_lossy()
                .into_owned(),
        })
        .unwrap();
    assert_receipt_corrupt(&config);
    cleanup(&config);
}

#[test]
fn semantic_agent_open_receipt_corruption_is_refused() {
    let config = config("open-receipt-corrupt");
    let launch = launch(&config);
    let operation = Operation::open_agent(&launch);
    begin_ticket::<AgentOpenReceipt>(&config, &operation)
        .complete(&AgentOpenReceipt {
            operation_id: launch.operation_id,
            request_sha256: launch.request_sha256,
            agent_id: launch.agent_id,
            cwd: launch.cwd,
            home: launch.home,
            conversation_id: Some("unexpected".into()),
        })
        .unwrap();
    assert_receipt_corrupt(&config);
    cleanup(&config);
}

#[test]
fn semantic_agent_close_receipt_corruption_is_refused() {
    let config = config("close-receipt-corrupt");
    let request = AgentCloseRequest::new("agent:agent-1:close:v1", AgentSlot::new("agent-1"));
    let operation = Operation::close_agent(&request);
    begin_ticket::<AgentCloseReceipt>(&config, &operation)
        .complete(&AgentCloseReceipt {
            operation_id: request.operation_id,
            request_sha256: request.request_sha256,
            agent_id: AgentSlot::new("agent-2"),
        })
        .unwrap();
    assert_receipt_corrupt(&config);
    cleanup(&config);
}

#[test]
fn semantic_agent_home_remove_receipt_corruption_is_refused() {
    let config = config("home-remove-receipt-corrupt");
    let request =
        AgentHomeRemoveRequest::new("agent:agent-1:home:remove:v1", AgentSlot::new("agent-1"));
    let operation = Operation::remove_agent_home(&request);
    begin_ticket::<AgentHomeRemoveReceipt>(&config, &operation)
        .complete(&AgentHomeRemoveReceipt {
            operation_id: request.operation_id,
            request_sha256: request.request_sha256,
            agent_id: AgentSlot::new("agent-2"),
        })
        .unwrap();
    assert_receipt_corrupt(&config);
    cleanup(&config);
}

fn home_plan() -> AgentHomePlan {
    AgentHomePlan {
        operation_id: "agent:agent-1:home:create:v1".into(),
        request_sha256: "a".repeat(64),
        agent_id: AgentSlot::new("agent-1"),
        files: vec![HomeFile {
            path: "config.toml".into(),
            bytes: b"original".to_vec(),
        }],
        symlinks: Vec::new(),
        skills: Vec::new(),
    }
}

fn launch(config: &RuntimeConfig) -> AgentLaunch {
    AgentLaunch {
        operation_id: "agent:agent-1:open:v1".into(),
        request_sha256: "b".repeat(64),
        agent_id: AgentSlot::new("agent-1"),
        cwd: config
            .home
            .join("project/.worktrees/agent-1")
            .to_string_lossy()
            .into_owned(),
        home: config.agent_home("agent-1").to_string_lossy().into_owned(),
        program: "/bin/sh".into(),
        args: vec!["-c".into(), "exit 0".into()],
        env: Default::default(),
    }
}

fn complete_and_replay<T>(config: &RuntimeConfig, operation: Operation, receipt: &T)
where
    T: serde::Serialize + serde::de::DeserializeOwned + std::fmt::Debug + PartialEq,
{
    begin_ticket::<T>(config, &operation)
        .complete(receipt)
        .unwrap();
    assert!(matches!(
        begin::<T>(config, &operation).unwrap(),
        Begin::Completed(found) if found == *receipt
    ));
}

fn assert_receipt_corrupt(config: &RuntimeConfig) {
    let error = validate_all(config).unwrap_err();
    assert_eq!(error.code, "mutation_journal_corrupt");
    assert!(error.detail.contains("receipt"));
}
