use super::report::summary;
use super::*;
use std::os::unix::fs::{symlink, PermissionsExt};

#[test]
fn census_request_is_bounded_exact_and_rejects_duplicate_or_foreign_shapes() {
    let mut request = Request {
        format: "runtime-ledger-census-request-v1".into(),
        agents: vec![Member {
            agent_id: "fixture-agent".into(),
            sandbox_id: "fixture-parent".into(),
        }],
    };
    request.validate().unwrap();
    request.agents.push(request.agents[0].clone());
    assert!(request.validate().is_err());
    request.agents.pop();
    request.agents[0].agent_id = " padded ".into();
    assert!(request.validate().is_err());
    request.agents.clear();
    assert!(request.validate().is_err());
    request.agents = (0..=MAX_AGENTS)
        .map(|i| Member {
            agent_id: format!("fixture-{i}"),
            sandbox_id: "parent".into(),
        })
        .collect();
    assert!(request.validate().is_err());
    request.agents.pop();
    request.validate().unwrap();
    request.format = "runtime-ledger-census-request-v2".into();
    assert!(request.validate().is_err());
    assert!(serde_json::from_str::<Request>(
        r#"{"format":"runtime-ledger-census-request-v1","agents":[],"apply":true}"#
    )
    .is_err());
}

#[test]
fn census_private_artifacts_never_overwrite_or_follow_file_aliases() {
    let root = std::env::temp_dir().join(format!("cma-census-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&root).unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    let file = root.join("request.json");
    files::publish(&file, b"private fixture").unwrap();
    assert_eq!(files::read(&file).unwrap(), b"private fixture");
    assert!(files::publish(&file, b"replacement").is_err());
    let alias = root.join("alias.json");
    symlink(&file, &alias).unwrap();
    assert!(files::read(&alias).is_err());
    assert!(files::publish(&alias, b"replacement").is_err());
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(files::read(&file).is_err());
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
    std::fs::write(&file, vec![0; MAX_REQUEST as usize + 1]).unwrap();
    assert!(files::read(&file).is_err());
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(files::publish(&root.join("output.json"), b"private").is_err());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn census_summary_contains_only_counts_and_hashes() {
    let census = Census {
        format: "runtime-ledger-census-v1",
        observed_at: "fixture".into(),
        request_sha256: "a".repeat(64),
        source_sha256: "b".repeat(64),
        rows: vec![Row {
            member: Member {
                agent_id: "private-agent".into(),
                sandbox_id: "private-parent".into(),
            },
            outcome: "agent_missing_preserve",
            assessment: None,
            source_sha256: "c".repeat(64),
        }],
        writes: false,
        recovery_authorized: false,
    };
    let result = summary(&census, b"private artifact");
    assert_eq!(result["unknown"], 1);
    assert_eq!(result["requested"], 1);
    assert_eq!(result["recovery_authorized"], false);
    assert_eq!(result["outcomes"]["agent_missing_preserve"], 1);
    assert_eq!(result["preservation"]["unresolved_member_preserve_all"], 1);
    assert!(!result.to_string().contains("private"));
}

#[test]
fn census_summary_counts_joint_cohorts_without_losing_unknown_or_changed_members() {
    use layer2_core::sandbox::ledger_census::LedgerFailureCause;
    use layer2_core::sandbox::{AgentStatus, AgentWorkStep, SandboxStatus};
    let assessment = LedgerCensusAssessment {
        cause: LedgerFailureCause::JournalLifetimeBound,
        agent_state: AgentStatus::Failed,
        parent_state: SandboxStatus::Retiring,
        agent_revision: 11111,
        parent_revision: 22222,
        scheduled_step: AgentWorkStep::RemoveWorktree,
        cleanup_scheduled: true,
        cleanup_debt: 1,
        lease_present: false,
        conversation_present: false,
        input_state_present: true,
        attempted_or_recorded_work: true,
        retained_home_present: true,
        runtime_generation: Some(33333),
        recovery_route: "owner_disposition_then_supported_workload_migration".into(),
        preserve: vec![
            "complete_home_backup".into(),
            "agent_history_and_operation_keys".into(),
        ],
        filesystem_evidence: "not_observed".into(),
        provider_effects: "absence_not_proven".into(),
        recovery_authorized: false,
    };
    let mut changed = assessment.clone();
    changed.agent_state = AgentStatus::Deleted;
    changed.parent_state = SandboxStatus::Active;
    changed.cleanup_scheduled = false;
    changed.recovery_route = "changed_agent_state_requires_reclassification".into();
    let census = Census {
        format: "runtime-ledger-census-v1",
        observed_at: "private-time".into(),
        request_sha256: "a".repeat(64),
        source_sha256: "b".repeat(64),
        rows: [
            Some(assessment.clone()),
            Some(assessment),
            Some(changed),
            None,
        ]
        .into_iter()
        .enumerate()
        .map(|(i, assessment)| Row {
            member: Member {
                agent_id: format!("private-agent-{i}"),
                sandbox_id: "private-parent".into(),
            },
            outcome: if assessment.is_some() {
                "observed_preserve"
            } else {
                "parent_missing_preserve"
            },
            assessment,
            source_sha256: "c".repeat(64),
        })
        .collect(),
        writes: false,
        recovery_authorized: false,
    };
    let result = summary(&census, b"private-source");
    assert_eq!(result["requested"], 4);
    assert_eq!(result["assessed"], 3);
    assert_eq!(result["unknown"], 1);
    assert_eq!(result["parent_states"]["retiring"], 2);
    assert_eq!(result["agent_states"]["deleted"], 1);
    assert_eq!(result["cohorts"].as_array().unwrap().len(), 2);
    assert_eq!(
        result["cohorts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["count"].as_u64().unwrap())
            .sum::<u64>(),
        3
    );
    assert_eq!(result["preservation"]["complete_home_backup"], 3);
    assert_eq!(result["preservation"]["unresolved_member_preserve_all"], 1);
    assert_eq!(result["evidence_counts"]["input_state_present"], 3);
    assert_eq!(result["evidence_counts"]["cleanup_scheduled"], 2);
    assert_eq!(result["writes"], false);
    assert_eq!(result["recovery_authorized"], false);
    let output = result.to_string();
    for private in ["private", "11111", "22222", "33333", &"c".repeat(64)] {
        assert!(!output.contains(private));
    }
    let mut reordered = census;
    reordered.rows.reverse();
    assert_eq!(summary(&reordered, b"private-source"), result);
}
