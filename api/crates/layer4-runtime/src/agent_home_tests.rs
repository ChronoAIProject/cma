use layer3_adapter::{AgentSlot, HomeFile, HomeSymlink};

use super::*;

fn config(name: &str) -> RuntimeConfig {
    let root = std::env::temp_dir().join(format!("cma-agent-home-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    RuntimeConfig::for_test(root)
}

#[tokio::test]
async fn files_and_safe_links_publish_as_one_agent_home() {
    let config = config("materialize");
    std::fs::create_dir_all(config.home.join(".cma/provider/codex")).unwrap();
    std::fs::write(config.home.join(".cma/provider/codex/auth.json"), "{}").unwrap();
    let home = materialize(
        &config,
        &reqwest::Client::new(),
        &AgentHomePlan {
            operation_id: "agent:agent-1:home:create:v1".into(),
            request_sha256: "a".repeat(64),
            agent_id: AgentSlot::new("agent-1"),
            files: vec![HomeFile {
                path: "config.toml".into(),
                bytes: b"model = \"test\"\n".to_vec(),
            }],
            symlinks: vec![HomeSymlink {
                path: "auth.json".into(),
                target: config
                    .home
                    .join(".cma/provider/codex/auth.json")
                    .to_string_lossy()
                    .into_owned(),
            }],
            skills: Vec::new(),
        },
    )
    .await
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(home.join("config.toml")).unwrap(),
        "model = \"test\"\n"
    );
    assert!(std::fs::symlink_metadata(home.join("auth.json"))
        .unwrap()
        .file_type()
        .is_symlink());
    assert!(remove(&config, "agent-1").unwrap());
    assert!(!home.exists());
    std::fs::remove_dir_all(config.home).unwrap();
}

#[tokio::test]
async fn traversal_and_symlink_parent_conflicts_leave_no_home() {
    let config = config("refuse");
    let plan = AgentHomePlan {
        operation_id: "agent:agent-1:home:create:v1".into(),
        request_sha256: "b".repeat(64),
        agent_id: AgentSlot::new("agent-1"),
        files: vec![HomeFile {
            path: "skills/link/owned".into(),
            bytes: vec![1],
        }],
        symlinks: vec![HomeSymlink {
            path: "skills/link".into(),
            target: "/tmp".into(),
        }],
        skills: Vec::new(),
    };
    assert!(materialize(&config, &reqwest::Client::new(), &plan)
        .await
        .is_err());
    assert!(!config.agent_home("agent-1").exists());
    std::fs::remove_dir_all(config.home).unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn a_symlinked_agent_root_cannot_redirect_materialization() {
    let config = config("symlink-root");
    let outside = config.home.with_extension("outside");
    let _ = std::fs::remove_dir_all(&outside);
    std::fs::create_dir_all(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, config.home.join(".cma")).unwrap();
    let error = materialize(
        &config,
        &reqwest::Client::new(),
        &AgentHomePlan {
            operation_id: "agent:agent-1:home:create:v1".into(),
            request_sha256: "c".repeat(64),
            agent_id: AgentSlot::new("agent-1"),
            files: vec![HomeFile {
                path: "config.toml".into(),
                bytes: b"protected".to_vec(),
            }],
            symlinks: Vec::new(),
            skills: Vec::new(),
        },
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, "agent_home_path_conflict");
    assert!(!outside.join("agents/agent-1/home/config.toml").exists());
    std::fs::remove_file(config.home.join(".cma")).unwrap();
    std::fs::remove_dir_all(config.home).unwrap();
    std::fs::remove_dir_all(outside).unwrap();
}
