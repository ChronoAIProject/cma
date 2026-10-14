use super::*;
use sha2::{Digest, Sha256};
fn fixture() -> (RuntimeConfig, AgentSkillInstallRequest) {
    let base = std::env::temp_dir().join(format!("cma-live-skill-{}", uuid::Uuid::new_v4()));
    let config = RuntimeConfig::for_test(base);
    let mut bytes = std::io::Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut bytes);
        zip.start_file("SKILL.md", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"# Added skill\n").unwrap();
        zip.finish().unwrap();
    }
    let bytes = bytes.into_inner();
    let archive = layer3_adapter::SkillArchive {
        sha256: format!("{:x}", Sha256::digest(&bytes)),
        bytes,
    };
    crate::skills::cache_archive(&config, &archive).unwrap();
    let request = AgentSkillInstallRequest::new(
        layer3_adapter::AgentSlot::new("agent-one"),
        layer3_adapter::AgentSkill {
            id: "research".into(),
            name: Some("Research".into()),
            version: Some("1.0".into()),
            archive_sha256: archive.sha256,
            source_url: "https://unreachable.invalid/api/v1/skills/research/versions/1.0/download"
                .into(),
        },
    );
    let home = config.agent_home(request.agent_id.as_str());
    std::fs::create_dir_all(home.join("skills/existing")).unwrap();
    std::fs::write(home.join("skills/existing/SKILL.md"), "existing content").unwrap();
    std::fs::write(home.join("AGENTS.md"), "preserve instructions").unwrap();
    (config, request)
}
fn cleanup(config: &RuntimeConfig) {
    std::fs::remove_dir_all(&config.home).unwrap();
    std::fs::remove_dir_all(&config.state_root).unwrap();
}

#[tokio::test]
async fn live_skill_publishes_additively_and_recovers_lost_confirmation() {
    let (config, request) = fixture();
    preflight(&config, &request).unwrap();
    let operation = crate::mutation_journal::Operation::install_agent_skill(&request);
    let crate::mutation_journal::Begin::New(_ticket) =
        crate::mutation_journal::begin::<AgentSkillInstallReceipt>(&config, &operation).unwrap()
    else {
        panic!("new intent")
    };
    let receipt = install(&config, &reqwest::Client::new(), &request)
        .await
        .unwrap();
    // Crash after rename, before journal completion: recover the exact marker.
    let crate::mutation_journal::Begin::Started(ticket) =
        crate::mutation_journal::begin::<AgentSkillInstallReceipt>(&config, &operation).unwrap()
    else {
        panic!("durable intent")
    };
    assert_eq!(recover(&config, &request).unwrap(), Some(receipt.clone()));
    ticket.complete(&receipt).unwrap();
    assert_eq!(
        install(&config, &reqwest::Client::new(), &request)
            .await
            .unwrap(),
        receipt
    );
    let home = config.agent_home(request.agent_id.as_str());
    assert_eq!(
        std::fs::read_to_string(home.join("skills/research/SKILL.md")).unwrap(),
        "# Added skill\n"
    );
    assert_eq!(
        std::fs::read_to_string(home.join("skills/existing/SKILL.md")).unwrap(),
        "existing content"
    );
    assert_eq!(
        std::fs::read_to_string(home.join("AGENTS.md")).unwrap(),
        "preserve instructions"
    );
    let mut altered = request.clone();
    altered.skill.archive_sha256 = "b".repeat(64);
    altered.request_sha256 = altered.computed_sha256();
    assert!(recover(&config, &altered).is_err());
    cleanup(&config);
}

#[tokio::test]
async fn live_skill_refuses_existing_entries_and_symlinked_parents() {
    let (config, request) = fixture();
    let home = config.agent_home(request.agent_id.as_str());
    let target = home.join("skills/research");
    std::fs::create_dir(&target).unwrap();
    assert!(preflight(&config, &request).is_err());
    assert!(install(&config, &reqwest::Client::new(), &request)
        .await
        .is_err());
    let staging = home.join("skills/staged");
    std::fs::create_dir(&staging).unwrap();
    assert!(rename_new(&staging, &target).is_err());
    assert!(staging.exists());
    std::fs::rename(home.join("skills"), home.join("saved-skills")).unwrap();
    std::os::unix::fs::symlink(home.join("saved-skills"), home.join("skills")).unwrap();
    assert!(preflight(&config, &request).is_err());
    cleanup(&config);
}

#[tokio::test]
async fn live_skill_checksum_failure_leaves_other_files_untouched() {
    let (config, request) = fixture();
    std::fs::write(
        config
            .state_root
            .join("warm-skills")
            .join(&request.skill.archive_sha256),
        b"tampered",
    )
    .unwrap();
    let error = install(&config, &reqwest::Client::new(), &request)
        .await
        .unwrap_err();
    assert_eq!(error.code, "skill_checksum_mismatch");
    let home = config.agent_home(request.agent_id.as_str());
    assert!(!home.join("skills/research").exists());
    assert_eq!(
        std::fs::read_to_string(home.join("AGENTS.md")).unwrap(),
        "preserve instructions"
    );
    cleanup(&config);
}

#[test]
fn live_skill_authority_is_separate_per_skill_and_home_cleanup_dominates_replay() {
    use crate::mutation_journal::Operation;
    let (config, request) = fixture();
    let home = Operation::create_agent_home(&layer3_adapter::AgentHomePlan {
        operation_id: "agent:agent-one:home:create:v1".into(),
        request_sha256: "a".repeat(64),
        agent_id: request.agent_id.clone(),
        files: Vec::new(),
        symlinks: Vec::new(),
        skills: Vec::new(),
    });
    crate::mutation_authority::admit(&config, 1, &home).unwrap();
    let first = Operation::install_agent_skill(&request);
    crate::mutation_authority::admit(&config, 2, &first).unwrap();
    let mut other = request.skill.clone();
    other.id = "review".into();
    let second = Operation::install_agent_skill(&AgentSkillInstallRequest::new(
        request.agent_id.clone(),
        other,
    ));
    crate::mutation_authority::admit(&config, 3, &second).unwrap();
    crate::mutation_authority::admit(&config, 4, &first).unwrap();
    let remove = Operation::remove_agent_home(&layer3_adapter::AgentHomeRemoveRequest::new(
        "agent:agent-one:home:remove:v1",
        request.agent_id.clone(),
    ));
    crate::mutation_authority::admit(&config, 5, &remove).unwrap();
    assert!(crate::mutation_authority::admit(&config, 6, &first).is_err());
    assert_eq!(
        crate::mutation_authority::recovery_fence(&config, &first).unwrap(),
        None
    );
    cleanup(&config);
}
