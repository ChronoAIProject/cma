use super::*;
use sha2::{Digest as _, Sha256};
use std::{
    collections::BTreeMap,
    sync::{LazyLock, Mutex},
};
pub(super) static FAULTS: LazyLock<Mutex<BTreeMap<PathBuf, &'static str>>> =
    LazyLock::new(|| Mutex::new(BTreeMap::new()));

fn skill(config: &RuntimeConfig, version: &str) -> layer3_adapter::AgentSkill {
    let mut bytes = std::io::Cursor::new(Vec::new());
    let mut zip = zip::ZipWriter::new(&mut bytes);
    zip.start_file(
        "SKILL.md",
        zip::write::SimpleFileOptions::default().unix_permissions(0o644),
    )
    .unwrap();
    zip.write_all(format!("# Research {version}\n").as_bytes())
        .unwrap();
    zip.start_file(
        "scripts/helper.py",
        zip::write::SimpleFileOptions::default().unix_permissions(0o600),
    )
    .unwrap();
    zip.write_all(b"value = 42\n").unwrap();
    zip.finish().unwrap();
    let bytes = bytes.into_inner();
    let sha256 = format!("{:x}", Sha256::digest(&bytes));
    crate::skills::cache_archive(
        config,
        &layer3_adapter::SkillArchive {
            sha256: sha256.clone(),
            bytes,
        },
    )
    .unwrap();
    layer3_adapter::AgentSkill {
        id: "research".into(),
        name: Some("Research".into()),
        version: Some(version.into()),
        archive_sha256: sha256,
        source_url: "https://unavailable.invalid/archive".into(),
    }
}
async fn fixture(profile: bool) -> (RuntimeConfig, AgentSkillInstallRequest) {
    let base = std::env::temp_dir()
        .canonicalize()
        .unwrap()
        .join(format!("cma-skill-upgrade-{}", uuid::Uuid::new_v4()));
    let config = RuntimeConfig::for_test(base);
    let slot = layer3_adapter::AgentSlot::new("agent-one");
    let previous = skill(&config, "1.0");
    let next = skill(&config, "1.1");
    let home = config.agent_home(slot.as_str());
    std::fs::create_dir_all(&config.home).unwrap();
    crate::agent_home::materialize(
        &config,
        &reqwest::Client::new(),
        &layer3_adapter::AgentHomePlan {
            operation_id: "agent:agent-one:home:create:v1".into(),
            request_sha256: "a".repeat(64),
            agent_id: slot.clone(),
            files: vec![layer3_adapter::HomeFile {
                path: "AGENTS.md".into(),
                bytes: b"unchanged".to_vec(),
            }],
            symlinks: vec![],
            skills: if profile {
                vec![previous.clone()]
            } else {
                vec![]
            },
        },
    )
    .await
    .unwrap();
    std::fs::create_dir_all(home.join("skills")).unwrap();
    let receipt = if profile {
        None
    } else {
        Some(
            crate::agent_skill_install::install(
                &config,
                &reqwest::Client::new(),
                &AgentSkillInstallRequest::new(slot.clone(), previous.clone()),
            )
            .await
            .unwrap(),
        )
    };
    (
        config,
        AgentSkillInstallRequest::upgrade(slot, next, previous, receipt, 2),
    )
}
fn cleanup(config: &RuntimeConfig) {
    for path in [&config.home, &config.state_root] {
        if path.exists() {
            std::fs::remove_dir_all(path).unwrap();
        }
    }
}
#[tokio::test]
async fn upgrades_recover_every_crash_boundary_for_profile_and_live_skills() {
    for profile in [true, false] {
        for fault in [
            "previous_extracted",
            "new_extracted",
            "receipt_staged",
            "proof_published",
            "exchanged",
            "exchange_synced",
            "previous_retained",
        ] {
            let (config, request) = fixture(profile).await;
            std::fs::write(
                config
                    .agent_home(request.agent_id.as_str())
                    .join("skills/research/owner-notes"),
                b"retain through crash",
            )
            .unwrap();
            FAULTS
                .lock()
                .unwrap()
                .insert(config.state_root.clone(), fault);
            let error = install(&config, &reqwest::Client::new(), &request)
                .await
                .unwrap_err();
            assert_eq!(error.code, "injected_crash");
            let target = config
                .agent_home(request.agent_id.as_str())
                .join("skills/research/SKILL.md");
            let text = std::fs::read_to_string(&target).unwrap();
            assert!(text == "# Research 1.0\n" || text == "# Research 1.1\n");
            if [
                "proof_published",
                "exchanged",
                "exchange_synced",
                "previous_retained",
            ]
            .contains(&fault)
            {
                std::fs::remove_dir_all(config.state_root.join("warm-skills")).unwrap();
            }
            let receipt = install(&config, &reqwest::Client::new(), &request)
                .await
                .unwrap();
            assert!(receipt.matches(&request));
            assert_eq!(
                std::fs::read_to_string(&target).unwrap(),
                "# Research 1.1\n"
            );
            let retained = staging(target.parent().unwrap().parent().unwrap(), &request);
            assert_eq!(
                std::fs::read_to_string(retained.join("new/SKILL.md")).unwrap(),
                "# Research 1.0\n"
            );
            assert!(!retained.join("previous").exists());
            assert_eq!(
                std::fs::read(retained.join("new/owner-notes")).unwrap(),
                b"retain through crash"
            );
            assert_eq!(
                install(&config, &reqwest::Client::new(), &request)
                    .await
                    .unwrap(),
                receipt
            );
            assert_eq!(
                std::fs::read(
                    config
                        .agent_home(request.agent_id.as_str())
                        .join("AGENTS.md")
                )
                .unwrap(),
                b"unchanged"
            );
            cleanup(&config);
        }
    }
}
#[tokio::test]
async fn upgrade_refuses_unexpected_contents_marker_symlinks_and_checksum_without_changing_old() {
    for case in [0, 1, 2, 3, 4, 7, 8, 9] {
        let (config, request) = fixture(case == 1).await;
        let parent = config.agent_home(request.agent_id.as_str()).join("skills");
        let target = parent.join("research");
        match case {
            0 => std::fs::write(target.join("SKILL.md"), b"owner modification").unwrap(),
            1 => std::fs::write(target.join(MARKER), b"unexpected marker").unwrap(),
            2 => {
                std::fs::remove_file(target.join("SKILL.md")).unwrap();
                std::os::unix::fs::symlink("/etc/passwd", target.join("SKILL.md")).unwrap();
            }
            3 => {
                std::fs::rename(&parent, parent.with_extension("original")).unwrap();
                std::os::unix::fs::symlink(parent.with_extension("original"), &parent).unwrap();
            }
            4 => std::fs::write(
                config
                    .state_root
                    .join("warm-skills")
                    .join(&request.skill.archive_sha256),
                b"wrong checksum",
            )
            .unwrap(),
            7 => {
                std::fs::rename(&target, parent.join("original")).unwrap();
                std::os::unix::fs::symlink("original", &target).unwrap();
            }
            8 => std::fs::remove_file(target.join("SKILL.md")).unwrap(),
            _ => std::fs::write(target.join(MARKER), b"owner modified receipt").unwrap(),
        }
        let error = install(&config, &reqwest::Client::new(), &request)
            .await
            .unwrap_err();
        assert!(
            matches!(
                error.code.as_str(),
                "agent_skill_upgrade_conflict"
                    | "agent_skill_upgrade_modified"
                    | "agent_skill_install_conflict"
                    | "skill_checksum_mismatch"
            ),
            "{}",
            error.code
        );
        if case != 0 && case != 2 && case != 8 {
            assert_eq!(
                std::fs::read_to_string(target.join("SKILL.md")).unwrap(),
                "# Research 1.0\n"
            );
        }
        cleanup(&config);
    }
}

#[tokio::test]
async fn upgrade_ledger_replay_is_fenced_and_home_removal_dominates_both_store_formats() {
    use crate::{
        mutation_journal::Operation,
        mutation_store::{Begin, Store},
    };
    use std::sync::Arc;
    for indexed in [false, true] {
        let (config, request) = fixture(false).await;
        let ledger = if indexed {
            Some(Arc::new(
                crate::mutation_ledger::Ledger::open(
                    config.state_root.join("indexed"),
                    config.clone(),
                    true,
                    32768,
                )
                .await
                .unwrap(),
            ))
        } else {
            None
        };
        let store = Store::new(config.clone(), ledger.clone());
        let home = Operation::create_agent_home(&layer3_adapter::AgentHomePlan {
            operation_id: "agent:agent-one:home:create:v1".into(),
            request_sha256: "a".repeat(64),
            agent_id: request.agent_id.clone(),
            files: vec![],
            symlinks: vec![],
            skills: vec![],
        });
        store.admit(1, &[home]).await.unwrap();
        let original = AgentSkillInstallRequest::new(
            request.agent_id.clone(),
            request.upgrade.as_ref().unwrap().previous.clone(),
        );
        let original_op = Operation::install_agent_skill(&original);
        store
            .admit(2, std::slice::from_ref(&original_op))
            .await
            .unwrap();
        let Begin::New(ticket) = store
            .begin::<AgentSkillInstallReceipt>(&original_op)
            .await
            .unwrap()
        else {
            panic!("new original")
        };
        ticket
            .complete(&AgentSkillInstallReceipt::from_request(&original))
            .await
            .unwrap();
        let operation = Operation::install_agent_skill(&request);
        store
            .admit(3, std::slice::from_ref(&operation))
            .await
            .unwrap();
        let Begin::New(crashed_ticket) = store
            .begin::<AgentSkillInstallReceipt>(&operation)
            .await
            .unwrap()
        else {
            panic!("new upgrade")
        };
        FAULTS
            .lock()
            .unwrap()
            .insert(config.state_root.clone(), "exchanged");
        assert_eq!(
            install(&config, &reqwest::Client::new(), &request)
                .await
                .unwrap_err()
                .code,
            "injected_crash"
        );
        drop(crashed_ticket);
        assert_eq!(store.recovery_fence(&operation).await.unwrap(), Some(3));
        // Lost response, new locator: immutable content still owns the same operation.
        let mut replay = request.clone();
        replay.skill.source_url = "https://new.invalid/changed-locator".into();
        replay.upgrade.as_mut().unwrap().previous.source_url =
            "https://new.invalid/previous".into();
        let replay_op = Operation::install_agent_skill(&replay);
        store
            .admit(4, std::slice::from_ref(&replay_op))
            .await
            .unwrap();
        let Begin::Started(ticket) = store
            .begin::<AgentSkillInstallReceipt>(&replay_op)
            .await
            .unwrap()
        else {
            panic!("recover upgrade")
        };
        let receipt = install(&config, &reqwest::Client::new(), &replay)
            .await
            .unwrap();
        ticket.complete(&receipt).await.unwrap();
        assert!(
            matches!(store.begin::<AgentSkillInstallReceipt>(&operation).await.unwrap(), Begin::Completed(r) if r == receipt)
        );
        let next = AgentSkillInstallRequest::upgrade(
            request.agent_id.clone(),
            skill(&config, "1.2"),
            request.skill.clone(),
            Some(receipt),
            3,
        );
        let next_op = Operation::install_agent_skill(&next);
        store
            .admit(5, std::slice::from_ref(&next_op))
            .await
            .unwrap();
        let Begin::New(ticket) = store
            .begin::<AgentSkillInstallReceipt>(&next_op)
            .await
            .unwrap()
        else {
            panic!("next upgrade")
        };
        let next_receipt = install(&config, &reqwest::Client::new(), &next)
            .await
            .unwrap();
        ticket.complete(&next_receipt).await.unwrap();
        assert!(store
            .admit(4, std::slice::from_ref(&next_op))
            .await
            .is_err());
        assert!(store
            .admit(6, std::slice::from_ref(&operation))
            .await
            .is_err());
        for (sequence, fence, code) in [
            (2, 6, "mutation_authority_conflict"),
            (3, 6, "mutation_authority_conflict"),
            (4, 4, "mutation_authority_stale"),
            (4, 5, "mutation_authority_conflict"),
        ] {
            let different = AgentSkillInstallRequest::upgrade(
                request.agent_id.clone(),
                skill(&config, "different"),
                next.skill.clone(),
                Some(AgentSkillInstallReceipt::from_request(&next)),
                sequence,
            );
            let different = Operation::install_agent_skill(&different);
            assert_eq!(
                store.admit(fence, &[different]).await.unwrap_err().code,
                code
            );
        }
        assert_eq!(store.recovery_fence(&operation).await.unwrap(), None);
        let remove = Operation::remove_agent_home(&layer3_adapter::AgentHomeRemoveRequest::new(
            "agent:agent-one:home:remove:v1",
            request.agent_id.clone(),
        ));
        store.admit(7, &[remove]).await.unwrap();
        crate::agent_home::remove(&config, request.agent_id.as_str()).unwrap();
        assert!(!config.agent_home(request.agent_id.as_str()).exists());
        assert_eq!(
            store
                .admit(8, std::slice::from_ref(&next_op))
                .await
                .unwrap_err()
                .code,
            "mutation_authority_retired"
        );
        assert_eq!(store.recovery_fence(&next_op).await.unwrap(), None);
        drop(store);
        if let Some(ledger) = ledger {
            Arc::try_unwrap(ledger).ok().unwrap().close().await.unwrap();
        }
        cleanup(&config);
    }
}

#[path = "agent_skill_upgrade_compatibility_tests.rs"]
mod compatibility;

#[path = "agent_skill_upgrade_recovery_tests.rs"]
mod recovery;
