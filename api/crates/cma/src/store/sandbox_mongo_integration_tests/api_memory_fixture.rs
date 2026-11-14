//! Offline synthetic BSON export for the #2056 same-source API allocator comparison.
//! No runtime, Mongo connection, authentication provider or application task is started.
use super::*;
use std::io::Write;

#[test]
#[ignore = "offline fixture export requires a new CMA_MEMORY_FIXTURE_OUTPUT file"]
fn export_finite_api_memory_dataset() {
    let output = std::env::var("CMA_MEMORY_FIXTURE_OUTPUT").unwrap();
    let now = "2026-11-14T00:00:00Z".to_owned();
    let mut parent = sandbox("sbx_memory_rca");
    parent.work = SandboxWorkState::new(SandboxWorkStep::None, now.clone());
    parent.created_at = now.clone();
    parent.updated_at = now.clone();
    parent.last_activity_at = now.clone();
    let credential = parent.credential.current.as_mut().unwrap();
    credential.issued_at = now.clone();
    credential.expires_at = "2099-01-01T00:00:00Z".into();
    let mut workspace = workspace(&parent, "wks_atomic", "memory-rca");
    workspace.status = WorkspaceStatus::Ready;
    workspace.work = WorkspaceWorkState::new(WorkspaceWorkStep::None, now.clone());
    workspace.created_at = now.clone();
    workspace.ready_at = Some(now.clone());
    workspace.next_worktree_number = 2;
    let (worktree, mut agent) = worktree_and_agent(&parent.id);
    agent.status = AgentStatus::Stopped;
    agent.work = AgentWorkState::new(AgentWorkStep::None, now.clone());
    agent.created_at = now.clone();
    agent.updated_at = now.clone();
    agent.ready_at = Some(now.clone());
    agent.stopped_at = Some(now.clone());
    agent.stop_reason = Some(AgentStopReason::Owner);
    agent.conversation.conversation_id = Some("synthetic-memory-conversation".into());
    agent.conversation.runtime_generation = Some(1);
    let mut kinds = vec![
        AgentLogKind::Created {
            workspace_id: workspace.id.clone(),
            worktree_id: worktree.id.clone(),
        },
        AgentLogKind::ConversationOpened {
            conversation_id: "synthetic-memory-conversation".into(),
            runtime_generation: 1,
        },
    ];
    for round in 0..128 {
        let turn_sequence = kinds.len() as u64 + 1;
        kinds.extend([
            AgentLogKind::TurnStarted {
                model: None,
                turn_sequence,
                origin_key: None,
            },
            AgentLogKind::OwnerMessage {
                content: format!("synthetic finite read {round}"),
            },
            AgentLogKind::AgentMessage {
                content: "synthetic-memory-payload ".repeat(683),
            },
            AgentLogKind::TurnCompleted { turn_sequence },
        ]);
    }
    kinds.push(AgentLogKind::Stopped {
        reason: "owner".into(),
    });
    agent.conversation.last_seq = kinds.len() as u64;
    let logs: Vec<_> = kinds
        .into_iter()
        .enumerate()
        .map(|(index, kind)| AgentLogEntry {
            agent_id: agent.id.clone(),
            seq: index as u64 + 1,
            appended_at: now.clone(),
            kind,
        })
        .collect();
    super::super::agent_log::verify_complete_log(&agent.id, agent.conversation.last_seq, &logs)
        .unwrap();
    layer2_core::sandbox::AgentLogCheckpoint::validate_retained_semantics(&agent, &logs).unwrap();
    let parent_doc = super::super::sandboxes::sandbox_document(&parent, 0).unwrap();
    let workspace_doc = super::super::workspaces::workspace_document(&workspace).unwrap();
    let worktree_doc = super::super::worktrees::worktree_document(&worktree).unwrap();
    let agent_doc = super::super::agents::agent_document(&agent).unwrap();
    assert_eq!(
        super::super::sandboxes::read_sandbox(&parent_doc).unwrap(),
        parent
    );
    assert_eq!(
        super::super::workspaces::read_workspace(&workspace_doc).unwrap(),
        workspace
    );
    assert_eq!(
        super::super::worktrees::read_worktree(&worktree_doc).unwrap(),
        worktree
    );
    assert_eq!(super::super::agents::read_agent(&agent_doc).unwrap(), agent);
    let canonical = |value: Document| Bson::Document(value).into_canonical_extjson();
    let log_docs: Vec<_> = logs
        .iter()
        .map(|entry| {
            let document = log_document(entry).unwrap();
            assert_eq!(
                super::super::agent_log::read_entry(&document).unwrap(),
                *entry
            );
            canonical(document)
        })
        .collect();
    let fixture = serde_json::json!({
        "environment": "isolated-local", "synthetic_identity": true, "schema_version": 1,
        "subject": agent.owner.as_str(), "agent_id": agent.id.as_str(),
        "head": agent.conversation.last_seq, "turns": 128,
        "collections": {
            "sandboxes": [canonical(parent_doc)], "workspaces": [canonical(workspace_doc)],
            "worktrees": [canonical(worktree_doc)], "agents": [canonical(agent_doc)],
            "agent_log": log_docs
        }
    });
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(output)
        .unwrap()
        .write_all(&serde_json::to_vec(&fixture).unwrap())
        .unwrap();
    println!("synthetic fixture exported: agents=1 turns=128 events=515 pending_work=0");
}
