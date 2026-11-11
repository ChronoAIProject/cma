use super::*;
use crate::mutation_journal::Record;
use layer3_adapter::runtime_ledger_retirement::{
    ledger_retired_worktree_key, RuntimeLedgerRetirementSnapshot, LEDGER_RETIREMENT_SNAPSHOT_FORMAT,
};
use sha2::{Digest, Sha256};

fn snapshot(fixture: &Fixture) -> RuntimeLedgerRetirementSnapshot {
    RuntimeLedgerRetirementSnapshot {
        format: LEDGER_RETIREMENT_SNAPSHOT_FORMAT.into(),
        sandbox_id: fixture.config.sandbox_id.clone(),
        home_identity: fixture.config.home_identity.clone().unwrap(),
        generation: fixture.config.generation,
        sandbox_revision: 1,
        observed_at: "2026-11-11T00:00:00Z".into(),
        source_sha256: "b".repeat(64),
        agents_observed: 1,
        worktrees_observed: 1,
        deleted_agents: vec![],
        removed_worktrees: vec![ledger_retired_worktree_key("workspace", "agent")],
    }
}

fn write_snapshot(
    fixture: &Fixture,
    snapshot: &RuntimeLedgerRetirementSnapshot,
) -> (PathBuf, String) {
    let bytes = serde_json::to_vec(snapshot).unwrap();
    let path = fixture.root.join("observation.json");
    std::fs::write(&path, &bytes).unwrap();
    (path, format!("{:x}", Sha256::digest(bytes)))
}

fn completed_removal(fixture: &Fixture) -> Record {
    let operation = semantic::remove();
    Record::started(&fixture.config, &operation)
        .unwrap()
        .completed(&layer3_adapter::WorktreeRemoveReceipt {
            operation_id: operation.operation_id,
            request_sha256: operation.request_sha256,
            workspace: "workspace".into(),
            name: "agent".into(),
        })
        .unwrap()
}

#[test]
fn retirement_preview_requires_both_api_and_completed_absence_evidence() {
    for case in 0..8 {
        let fixture = semantic::fixture();
        let mut observation = snapshot(&fixture);
        if case != 1 {
            semantic::authority(&fixture.config, 2, &semantic::remove());
        }
        if case == 2 {
            observation.removed_worktrees.clear();
        }
        if case == 3 {
            observation.removed_worktrees =
                vec![ledger_retired_worktree_key("workspace", "different")];
        }
        if case != 4 {
            let started = Record::started(&fixture.config, &semantic::remove()).unwrap();
            let record = match case {
                5 => started,
                6 | 7 => started
                    .failed(
                        &RuntimeError::rejected("runtime_mutation_outcome_unknown", "uncertain"),
                        case == 7,
                    )
                    .unwrap(),
                _ => completed_removal(&fixture),
            };
            semantic::journal(&fixture.config, &record);
        }
        let before = fixture.scan().unwrap().digest;
        let (path, digest) = write_snapshot(&fixture, &observation);
        let report = retirement::preview(&fixture.config, &path, &digest).unwrap();
        assert_eq!(
            report["candidate_resource_groups"],
            u64::from(case == 0),
            "case {case}"
        );
        assert_eq!(report["source_mutated"], false);
        assert_eq!(report["apply_authority"], false);
        assert_eq!(fixture.scan().unwrap().digest, before);
        let text = report.to_string();
        assert!(!text.contains(&fixture.config.sandbox_id));
        assert!(!text.contains(&fixture.config.home.to_string_lossy().to_string()));
        assert!(!text.contains("worktree:workspace:agent"));
    }
}

#[test]
fn retirement_preview_refuses_wrong_digest_scope_symlink_and_unknown_snapshot_fields() {
    let fixture = semantic::fixture();
    semantic::authority(&fixture.config, 2, &semantic::remove());
    semantic::journal(&fixture.config, &completed_removal(&fixture));
    let before = fixture.scan().unwrap().digest;
    for case in 0..7 {
        let mut observation = snapshot(&fixture);
        match case {
            1 => observation.sandbox_id = "other".into(),
            2 => observation.home_identity = "b".repeat(43),
            3 => observation.generation += 1,
            4 => observation
                .removed_worktrees
                .push(observation.removed_worktrees[0].clone()),
            _ => {}
        }
        let (mut path, mut digest) = write_snapshot(&fixture, &observation);
        if case == 0 {
            digest = "f".repeat(64);
        }
        if case == 5 {
            let link = fixture.root.join("linked-observation.json");
            std::os::unix::fs::symlink(&path, &link).unwrap();
            path = link;
        }
        if case == 6 {
            let mut value = serde_json::to_value(&observation).unwrap();
            value["extra_authority"] = true.into();
            let bytes = serde_json::to_vec(&value).unwrap();
            digest = format!("{:x}", Sha256::digest(&bytes));
            std::fs::write(&path, bytes).unwrap();
        }
        assert!(
            retirement::preview(&fixture.config, &path, &digest).is_err(),
            "case {case}"
        );
        assert_eq!(fixture.scan().unwrap().digest, before);
    }
}

#[test]
fn retirement_preview_keeps_temporaries_and_does_not_reconcile_source() {
    let fixture = semantic::fixture();
    semantic::authority(&fixture.config, 2, &semantic::remove());
    let journal = semantic::journal(&fixture.config, &completed_removal(&fixture));
    let temporary = fixture.write("mutation-authority/.authority-1.tmp", b"partial");
    let before = fixture.scan().unwrap().digest;
    let (path, digest) = write_snapshot(&fixture, &snapshot(&fixture));
    let report = retirement::preview(&fixture.config, &path, &digest).unwrap();
    assert_eq!(report["candidate_resource_groups"], 0);
    assert_eq!(report["source_temporaries_present"], true);
    assert_eq!(std::fs::read(temporary).unwrap(), b"partial");
    assert_eq!(
        std::fs::read(journal).unwrap(),
        completed_removal(&fixture).encode().unwrap()
    );
    assert_eq!(fixture.scan().unwrap().digest, before);
}

#[test]
fn retirement_preview_requires_home_absence_for_skills_and_all_process_generations() {
    use crate::mutation_journal::Operation;
    use layer3_adapter::runtime_ledger_retirement::ledger_retired_agent_key;
    use layer3_adapter::{
        AgentCloseRequest, AgentHomeRemoveRequest, AgentSkill, AgentSkillInstallRequest, AgentSlot,
    };
    for incomplete in [false, true] {
        let fixture = semantic::fixture();
        let mut observation = snapshot(&fixture);
        observation
            .deleted_agents
            .push(ledger_retired_agent_key("agent"));
        observation.removed_worktrees.clear();
        let home = Operation::remove_agent_home(&AgentHomeRemoveRequest::new(
            "agent:agent:home:remove:v1",
            AgentSlot::new("agent"),
        ));
        semantic::authority(&fixture.config, 3, &home);
        if !incomplete {
            let record = Record::started(&fixture.config, &home)
                .unwrap()
                .completed(&layer3_adapter::wire::AgentHomeRemoveReceipt {
                    operation_id: home.operation_id.clone(),
                    request_sha256: home.request_sha256.clone(),
                    agent_id: AgentSlot::new("agent"),
                })
                .unwrap();
            semantic::journal(&fixture.config, &record);
        }
        let skill_request = AgentSkillInstallRequest::new(
            AgentSlot::new("agent"),
            AgentSkill {
                id: "fixture-skill".into(),
                name: None,
                version: Some("1.0".into()),
                archive_sha256: "a".repeat(64),
                source_url: "https://example.com/fixture-skill.tar.gz".into(),
            },
        );
        let skill = Operation::install_agent_skill(&skill_request);
        semantic::authority(&fixture.config, 1, &skill);
        let record = Record::started(&fixture.config, &skill)
            .unwrap()
            .completed(&layer3_adapter::AgentSkillInstallReceipt::from_request(
                &skill_request,
            ))
            .unwrap();
        semantic::journal(&fixture.config, &record);
        let close = Operation::close_agent(&AgentCloseRequest::new(
            "agent:agent:close:v1",
            AgentSlot::new("agent"),
        ));
        for generation in [2, fixture.config.generation] {
            let mut config = fixture.config.clone();
            config.generation = generation;
            semantic::authority(&config, 2, &close);
            let record = Record::started(&config, &close)
                .unwrap()
                .completed(&layer3_adapter::wire::AgentCloseReceipt {
                    operation_id: close.operation_id.clone(),
                    request_sha256: close.request_sha256.clone(),
                    agent_id: AgentSlot::new("agent"),
                })
                .unwrap();
            semantic::journal(&config, &record);
        }
        let before = fixture.scan().unwrap().digest;
        let (path, digest) = write_snapshot(&fixture, &observation);
        let report = retirement::preview(&fixture.config, &path, &digest).unwrap();
        assert_eq!(report["candidate_resource_groups"], u64::from(!incomplete));
        assert_eq!(
            report["candidate_journal_files"],
            if incomplete { 0 } else { 4 }
        );
        assert_eq!(
            report["candidate_current_journal_files"],
            if incomplete { 0 } else { 3 }
        );
        assert_eq!(
            report["candidate_authority_files"],
            if incomplete { 0 } else { 4 }
        );
        assert_eq!(
            report["candidate_current_authority_files"],
            if incomplete { 0 } else { 3 }
        );
        assert_eq!(fixture.scan().unwrap().digest, before);
    }
}
