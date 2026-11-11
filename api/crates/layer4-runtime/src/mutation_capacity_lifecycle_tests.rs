use super::*;
use layer3_adapter::wire::{
    AgentCloseReceipt, AgentHomeReceipt, AgentHomeRemoveReceipt, AgentOpenReceipt,
};
use layer3_adapter::{AgentCloseRequest, AgentHomeRemoveRequest, AgentSlot, WorktreeReceipt};

fn lifecycle(config: &RuntimeConfig, index: usize) -> Vec<(Operation, serde_json::Value)> {
    let agent = format!("a-{index}");
    let cwd = crate::worktree::expected_path(&config.home, "project", &agent)
        .to_string_lossy()
        .into_owned();
    let home = config.agent_home(&agent).to_string_lossy().into_owned();
    let add = add(index);
    let create = Operation {
        operation_id: format!("agent:{agent}:home:create:v1"),
        request_sha256: "a".repeat(64),
        payload_sha256: "b".repeat(64),
        subject: Subject::CreateAgentHome {
            agent_id: agent.clone(),
        },
    };
    let open = Operation {
        operation_id: format!("agent:{agent}:open:v1"),
        request_sha256: "c".repeat(64),
        payload_sha256: "d".repeat(64),
        subject: Subject::OpenAgent {
            agent_id: agent.clone(),
            cwd: cwd.clone(),
            home: home.clone(),
        },
    };
    let close = Operation::close_agent(&AgentCloseRequest::new(
        format!("agent:{agent}:close:v1"),
        AgentSlot::new(&agent),
    ));
    let remove_home = Operation::remove_agent_home(&AgentHomeRemoveRequest::new(
        format!("agent:{agent}:home:remove:v1"),
        AgentSlot::new(&agent),
    ));
    let remove = remove(index);
    vec![
        (
            add.clone(),
            serde_json::to_value(WorktreeReceipt {
                operation_id: add.operation_id,
                request_sha256: add.request_sha256,
                workspace: "project".into(),
                name: agent.clone(),
                path: cwd.clone(),
                branch: format!("cma/{agent}"),
                base_commit: "a".repeat(40),
            })
            .unwrap(),
        ),
        (
            create.clone(),
            serde_json::to_value(AgentHomeReceipt {
                operation_id: create.operation_id,
                request_sha256: create.request_sha256,
                agent_id: AgentSlot::new(&agent),
                path: home.clone(),
            })
            .unwrap(),
        ),
        (
            open.clone(),
            serde_json::to_value(AgentOpenReceipt {
                operation_id: open.operation_id,
                request_sha256: open.request_sha256,
                agent_id: AgentSlot::new(&agent),
                cwd,
                home,
                conversation_id: None,
            })
            .unwrap(),
        ),
        (
            close.clone(),
            serde_json::to_value(AgentCloseReceipt {
                operation_id: close.operation_id,
                request_sha256: close.request_sha256,
                agent_id: AgentSlot::new(&agent),
            })
            .unwrap(),
        ),
        (
            remove_home.clone(),
            serde_json::to_value(AgentHomeRemoveReceipt {
                operation_id: remove_home.operation_id,
                request_sha256: remove_home.request_sha256,
                agent_id: AgentSlot::new(&agent),
            })
            .unwrap(),
        ),
        (remove.clone(), terminal(config, &remove).receipt.unwrap()),
    ]
}

#[test]
fn many_completed_agent_lifecycles_stop_new_work_without_losing_history() {
    let config = config("lifecycles");
    // Seed realistic typed terminal history, then exercise admission at the edge.
    for index in 0..168 {
        for (operation, receipt) in lifecycle(&config, index) {
            let mut row = record(&config, &operation);
            row.status = Status::Completed;
            row.receipt = Some(receipt);
            write_record(&config, &row);
        }
    }
    for index in 168..170 {
        for (operation, receipt) in lifecycle(&config, index) {
            authority::admit(&config, 1, &operation).unwrap();
            let Begin::New(ticket) = begin::<serde_json::Value>(&config, &operation).unwrap()
            else {
                panic!()
            };
            ticket.complete(&receipt).unwrap();
        }
        assert!(active_agents(&config).unwrap().is_empty());
    }
    let next = lifecycle(&config, 170);
    authority::admit(&config, 1, &next[0].0).unwrap();
    held(authority::admit(&config, 1, &next[1].0).unwrap_err());
    // Partial creation retains its reservation, even though its effect never ran.
    authority::admit(&config, 2, &remove(170)).unwrap();
    let Begin::New(ticket) = begin::<WorktreeRemoveReceipt>(&config, &remove(170)).unwrap() else {
        panic!()
    };
    ticket
        .complete(&terminal(&config, &remove(170)).receipt.unwrap())
        .unwrap();
    for index in [0, 85, 169] {
        for (operation, receipt) in lifecycle(&config, index) {
            assert!(
                matches!(begin::<serde_json::Value>(&config, &operation).unwrap(), Begin::Completed(found) if found == receipt)
            );
        }
    }
    assert_eq!(inventory(&config).unwrap().len(), 1021);
    assert!(started(&config).unwrap().is_empty());
    cleanup(&config);
}

#[test]
fn historical_skill_reserves_parent_home_cleanup_and_tombstone_dominates() {
    let config = config("skill-parent");
    fill(&config, 1021);
    let request = layer3_adapter::AgentSkillInstallRequest::new(
        AgentSlot::new("skilled"),
        layer3_adapter::AgentSkill {
            id: "research".into(),
            name: None,
            version: Some("1.0".into()),
            archive_sha256: "a".repeat(64),
            source_url: "https://skills.invalid/api/v1/skills/research/versions/1.0/download"
                .into(),
        },
    );
    let skill = Operation::install_agent_skill(&request);
    authority::admit(&config, 1, &skill).unwrap();
    held(authority::admit(&config, 1, &remove(9000)).unwrap_err());
    let cleanup_home = Operation::remove_agent_home(&AgentHomeRemoveRequest::new(
        "agent:skilled:home:remove:v1",
        AgentSlot::new("skilled"),
    ));
    authority::admit(&config, 2, &cleanup_home).unwrap();
    assert_eq!(
        authority::admit(&config, 3, &skill).unwrap_err().code,
        "mutation_authority_retired"
    );
    // Authority alone does not invent a process closure or terminal receipt.
    assert!(!admitted(&config, &cleanup_home).unwrap());
    cleanup(&config);
}

#[test]
fn changed_payload_on_reserved_authority_does_not_spend_or_free_capacity() {
    let config = config("reserved-conflict");
    fill(&config, 1021);
    let operation = add(5000);
    authority::admit(&config, 1, &operation).unwrap();
    let changed = Operation::add_worktree(&WorktreeRequest::new(
        &operation.operation_id,
        "project",
        "a-5000",
        "b".repeat(40),
    ));
    assert_eq!(
        authority::admit(&config, 2, &changed).unwrap_err().code,
        "mutation_authority_conflict"
    );
    held(authority::admit(&config, 3, &remove(6000)).unwrap_err());
    assert_eq!(
        authority::recovery_fence(&config, &operation).unwrap(),
        Some(1)
    );
    cleanup(&config);
}

#[test]
fn retained_cleanup_journal_cannot_be_used_to_recreate_missing_authority() {
    let config = config("journal-cleanup-barrier");
    write_record(&config, &terminal(&config, &remove(5000)));
    assert_eq!(
        authority::admit(&config, 9, &add(5000)).unwrap_err().code,
        "mutation_authority_retired"
    );
    assert!(authority::records(&config).unwrap().is_empty());
    authority::admit(&config, 10, &remove(5000)).unwrap();
    assert!(matches!(
        begin::<WorktreeRemoveReceipt>(&config, &remove(5000)).unwrap(),
        Begin::Completed(_)
    ));
    cleanup(&config);
}
