use std::io::Write as _;

use axum::routing::get;
use sha2::Digest as _;

use super::*;

fn zip_bytes() -> Vec<u8> {
    let mut bytes = Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut bytes);
        writer
            .start_file::<_, ()>(
                "SKILL.md",
                zip::write::SimpleFileOptions::default().unix_permissions(0o644),
            )
            .unwrap();
        writer.write_all(b"# Verified\n").unwrap();
        writer.finish().unwrap();
    }
    bytes.into_inner()
}

async fn server(bytes: Vec<u8>) -> (String, tokio::task::JoinHandle<()>) {
    let app = axum::Router::new().route(
        "/api/v1/proxy/s/ornn-api/api/v1/skills/review/versions/1/download",
        get(move || {
            let bytes = bytes.clone();
            async move { bytes }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{address}"), task)
}

#[tokio::test]
async fn archive_is_fetched_only_through_the_loopback_route_and_verified() {
    let archive = zip_bytes();
    let digest = hex(&Sha256::digest(&archive));
    let (proxy, server) = server(archive).await;
    let root = std::env::temp_dir().join(format!("cma-skill-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    install(
        &reqwest::Client::new(),
        Some(&proxy),
        &AgentSkill {
            id: "review".into(),
            name: None,
            version: Some("1".into()),
            archive_sha256: digest,
            source_url: "https://ornn.invalid/api/v1/skills/review/versions/1/download".into(),
        },
        &root.join("review"),
    )
    .await
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(root.join("review/SKILL.md")).unwrap(),
        "# Verified\n"
    );
    server.abort();
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn checksum_mismatch_never_extracts_the_archive() {
    let (proxy, server) = server(zip_bytes()).await;
    let root = std::env::temp_dir().join(format!("cma-bad-skill-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let destination = root.join("review");
    let error = install(
        &reqwest::Client::new(),
        Some(&proxy),
        &AgentSkill {
            id: "review".into(),
            name: None,
            version: None,
            archive_sha256: "0".repeat(64),
            source_url: "/api/v1/skills/review/versions/1/download".into(),
        },
        &destination,
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, "skill_checksum_mismatch");
    assert!(!destination.exists());
    server.abort();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn recursively_encoded_skill_paths_are_refused() {
    for source in [
        "/api/v1/skills/%252e%252e/secret",
        "/api/v1/skills/review%252fsecret",
    ] {
        let error = proxied_url("http://127.0.0.1:9092", source).unwrap_err();
        assert_eq!(error.code, "skill_source_invalid", "accepted {source}");
    }
}

#[tokio::test]
#[ignore = "requires CMA_SKILL_ARCHIVE, CMA_SKILL_SHA256, and CMA_SKILL_MANIFEST"]
async fn pinned_external_archive_materializes_through_the_runtime_installer() {
    let source = std::env::var("CMA_SKILL_ARCHIVE").expect("archive path");
    let digest = std::env::var("CMA_SKILL_SHA256").expect("pinned archive digest");
    let manifest = std::env::var("CMA_SKILL_MANIFEST").expect("relative skill manifest");
    let manifest = safe_relative(&manifest, "skill manifest").unwrap();
    let archive = std::fs::read(source).unwrap();
    assert_eq!(hex(&Sha256::digest(&archive)), digest);
    let (proxy, server) = server(archive).await;
    let root = std::env::temp_dir().join(format!(
        "cma-external-skill-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&root).unwrap();
    let destination = root.join("selected-skill");
    install(
        &reqwest::Client::new(),
        Some(&proxy),
        &AgentSkill {
            id: "selected-skill".into(),
            name: None,
            version: None,
            archive_sha256: digest,
            source_url: "/api/v1/skills/review/versions/1/download".into(),
        },
        &destination,
    )
    .await
    .unwrap();
    let manifest = destination.join(manifest);
    assert!(std::fs::metadata(&manifest).unwrap().is_file());
    assert!(!std::fs::read_to_string(manifest).unwrap().trim().is_empty());
    server.abort();
    std::fs::remove_dir_all(root).unwrap();
}
