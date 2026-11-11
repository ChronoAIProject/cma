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
    assert!(!result.to_string().contains("private"));
}
