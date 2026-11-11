use super::*;
use crate::{
    mutation_journal::Operation,
    mutation_store::{Begin, Store},
};

#[tokio::test]
async fn restart_settles_each_post_proof_state_without_discarding_conflicting_evidence() {
    for case in [
        "new_receipt",
        "proved_pair",
        "changed_target",
        "missing_candidate",
        "changed_candidate",
        "new_receipt_without_stage",
        "new_receipt_bad_proof",
    ] {
        let (config, request) = fixture(true).await;
        let published = case.starts_with("new_receipt");
        let parent = config.agent_home(request.agent_id.as_str()).join("skills");
        let stage = staging(&parent, &request);
        let target = parent.join("research");
        let operation = Operation::install_agent_skill(&request);
        let store = Store::new(config.clone(), None);
        store
            .admit(1, std::slice::from_ref(&operation))
            .await
            .unwrap();
        let Begin::New(ticket) = store
            .begin::<AgentSkillInstallReceipt>(&operation)
            .await
            .unwrap()
        else {
            panic!("new operation")
        };
        FAULTS.lock().unwrap().insert(
            config.state_root.clone(),
            if published {
                "exchanged"
            } else {
                "proof_published"
            },
        );
        assert_eq!(
            install(&config, &reqwest::Client::new(), &request)
                .await
                .unwrap_err()
                .code,
            "injected_crash"
        );
        drop(ticket);
        if case != "proved_pair" && case != "changed_candidate" {
            std::fs::write(target.join("SKILL.md"), b"owner edit after proof").unwrap();
        }
        if case == "new_receipt" {
            std::fs::write(stage.join("new/owner-note"), b"keep old-tree additions").unwrap();
        }
        if case == "missing_candidate" {
            std::fs::remove_dir_all(stage.join("new")).unwrap();
        }
        if case == "changed_candidate" {
            std::fs::write(stage.join("new/SKILL.md"), b"candidate edit").unwrap();
        }
        if case == "new_receipt_without_stage" {
            std::fs::remove_dir_all(&stage).unwrap();
        }
        let evidence = proof_path(&config, &request).unwrap();
        if case == "new_receipt_bad_proof" {
            std::fs::write(&evidence, b"changed proof").unwrap();
        }
        let proof_bytes = std::fs::read(&evidence).unwrap();
        std::fs::remove_dir_all(config.state_root.join("warm-skills")).unwrap();
        drop(store);
        // Real startup runs journal recovery before serving any new request.
        let supervisor = crate::supervisor::Supervisor::start(
            config.clone(),
            crate::writer_fence::SupervisorBoundary::default(),
        )
        .await
        .unwrap();
        let store = Store::new(config.clone(), None);
        let outcome = store
            .begin::<AgentSkillInstallReceipt>(&operation)
            .await
            .unwrap();
        if published || case == "proved_pair" {
            assert!(matches!(outcome, Begin::Completed(receipt) if receipt.matches(&request)));
            assert!(tree::receipt_matches(
                &target,
                &AgentSkillInstallReceipt::from_request(&request)
            )
            .unwrap());
        } else {
            assert!(
                matches!(outcome, Begin::Failed(error) if error.code == "agent_skill_upgrade_recovery_conflict" && !error.retryable)
            );
            assert!(stage.join("previous/SKILL.md").exists());
            if case == "changed_target" {
                assert!(stage.join("new/SKILL.md").exists());
            }
            if case == "changed_candidate" {
                assert_eq!(
                    std::fs::read(stage.join("new/SKILL.md")).unwrap(),
                    b"candidate edit"
                );
            }
        }
        assert_eq!(std::fs::read(&evidence).unwrap(), proof_bytes);
        if case != "proved_pair" && case != "changed_candidate" {
            assert_eq!(
                std::fs::read(target.join("SKILL.md")).unwrap(),
                b"owner edit after proof"
            );
        }
        if case == "new_receipt" {
            assert_eq!(
                std::fs::read(stage.join("new/owner-note")).unwrap(),
                b"keep old-tree additions"
            );
        }
        assert!(store
            .started(&mut Default::default())
            .await
            .unwrap()
            .is_empty());
        drop(store);
        drop(supervisor);
        cleanup(&config);
    }
}

#[tokio::test]
async fn preparation_attempts_survive_restart_and_exhaust_before_any_exchange() {
    let (config, request) = fixture(true).await;
    let next_archive = config
        .state_root
        .join("warm-skills")
        .join(&request.skill.archive_sha256);
    let archive = std::fs::read(&next_archive).unwrap();
    std::fs::remove_file(&next_archive).unwrap();
    let operation = Operation::install_agent_skill(&request);
    let store = Store::new(config.clone(), None);
    store
        .admit(1, std::slice::from_ref(&operation))
        .await
        .unwrap();
    let Begin::New(ticket) = store
        .begin::<AgentSkillInstallReceipt>(&operation)
        .await
        .unwrap()
    else {
        panic!("new operation")
    };
    for attempt in 1..=layer3_adapter::MAX_AGENT_SKILL_UPGRADE_PREPARATION_ATTEMPTS {
        let error = install(&config.clone(), &reqwest::Client::new(), &request)
            .await
            .unwrap_err();
        if attempt < layer3_adapter::MAX_AGENT_SKILL_UPGRADE_PREPARATION_ATTEMPTS {
            assert!(error.retryable);
        } else {
            assert_eq!(error.code, "agent_skill_upgrade_preparation_exhausted");
            assert!(!error.retryable);
        }
    }
    // Lose the terminal reply before its journal commit. Startup must still fail
    // this exact operation, even if the missing dependency has since recovered.
    drop(ticket);
    drop(store);
    std::fs::write(next_archive, archive).unwrap();
    let supervisor = crate::supervisor::Supervisor::start(
        config.clone(),
        crate::writer_fence::SupervisorBoundary::default(),
    )
    .await
    .unwrap();
    let store = Store::new(config.clone(), None);
    assert!(
        matches!(store.begin::<AgentSkillInstallReceipt>(&operation).await.unwrap(), Begin::Failed(error) if error.code == "agent_skill_upgrade_preparation_exhausted")
    );
    assert!(store
        .started(&mut Default::default())
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        std::fs::read_to_string(
            config
                .agent_home(request.agent_id.as_str())
                .join("skills/research/SKILL.md")
        )
        .unwrap(),
        "# Research 1.0\n"
    );
    drop(store);
    drop(supervisor);
    let mut retry = request.clone();
    retry.upgrade.as_mut().unwrap().sequence += 1;
    retry.operation_id = retry.expected_operation_id();
    retry.request_sha256 = retry.computed_sha256();
    assert!(install(&config, &reqwest::Client::new(), &retry)
        .await
        .unwrap()
        .matches(&retry));
    cleanup(&config);
}
