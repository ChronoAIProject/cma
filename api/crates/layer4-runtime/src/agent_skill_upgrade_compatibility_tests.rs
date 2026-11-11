use super::*;
use std::os::unix::fs::PermissionsExt as _;

#[tokio::test]
async fn real_materializers_upgrade_remounted_modes_and_preserve_caches_and_owner_extras() {
    for profile in [true, false] {
        let (config, request) = fixture(profile).await;
        let parent = config.agent_home(request.agent_id.as_str()).join("skills");
        let target = parent.join("research");
        // The recorded kubelet remount changes: 0600 -> 0660 and 0750 -> 0770.
        for (path, mode) in [
            (target.clone(), 0o2770),
            (target.join("scripts"), 0o770),
            (target.join("scripts/helper.py"), 0o660),
            (target.join("SKILL.md"), 0o664),
        ] {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
        }
        let imported = std::process::Command::new("python3")
            .args(["-c", "import sys; sys.pycache_prefix = None; sys.dont_write_bytecode = False; import helper; assert helper.value == 42"])
            .current_dir(target.join("scripts"))
            .env_remove("PYTHONDONTWRITEBYTECODE")
            .env_remove("PYTHONPYCACHEPREFIX")
            .status()
            .unwrap();
        assert!(imported.success());
        let cache = std::fs::read_dir(target.join("scripts/__pycache__"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let cache_bytes = std::fs::read(&cache).unwrap();
        std::fs::write(target.join("owner-notes"), b"keep these notes").unwrap();
        let receipt = install(&config, &reqwest::Client::new(), &request)
            .await
            .unwrap();
        assert!(receipt.matches(&request));
        assert_eq!(
            std::fs::read_to_string(target.join("SKILL.md")).unwrap(),
            "# Research 1.1\n"
        );
        let retained = staging(&parent, &request).join("new");
        assert!(!retained.starts_with(&parent));
        assert_eq!(
            std::fs::read(retained.join(cache.strip_prefix(&target).unwrap())).unwrap(),
            cache_bytes
        );
        assert_eq!(
            std::fs::read(retained.join("owner-notes")).unwrap(),
            b"keep these notes"
        );
        assert_eq!(
            std::fs::metadata(retained.join("scripts/helper.py"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o660
        );
        assert!(!target.join("owner-notes").exists());
        cleanup(&config);
    }
}

#[tokio::test]
async fn modified_archive_content_is_named_and_left_intact() {
    let (config, request) = fixture(true).await;
    let target = config
        .agent_home(request.agent_id.as_str())
        .join("skills/research/SKILL.md");
    std::fs::write(&target, b"owner edits").unwrap();
    let error = install(&config, &reqwest::Client::new(), &request)
        .await
        .unwrap_err();
    assert_eq!(error.code, "agent_skill_upgrade_modified");
    assert!(!error.retryable);
    assert_eq!(std::fs::read(target).unwrap(), b"owner edits");
    cleanup(&config);
}

#[tokio::test]
async fn missing_previous_archive_is_named_without_changing_the_installation() {
    let (config, request) = fixture(true).await;
    std::fs::remove_file(
        config
            .state_root
            .join("warm-skills")
            .join(&request.upgrade.as_ref().unwrap().previous.archive_sha256),
    )
    .unwrap();
    let error = install(&config, &reqwest::Client::new(), &request)
        .await
        .unwrap_err();
    assert_eq!(error.code, "agent_skill_upgrade_previous_unavailable");
    assert!(!error.retryable);
    assert_eq!(
        std::fs::read_to_string(
            config
                .agent_home(request.agent_id.as_str())
                .join("skills/research/SKILL.md")
        )
        .unwrap(),
        "# Research 1.0\n"
    );
    cleanup(&config);
}

#[tokio::test]
async fn retention_bounds_refuse_without_evicting_data_or_swapping() {
    for byte_limit in [true, false] {
        let (config, request) = fixture(false).await;
        let parent = config.agent_home(request.agent_id.as_str()).join("skills");
        let stage = staging(&parent, &request);
        retention::prepare(&stage).unwrap();
        let root = stage.parent().unwrap();
        for n in 0..if byte_limit { 1 } else { 32 } {
            std::fs::create_dir(root.join(n.to_string())).unwrap();
        }
        let kept = root.join("0/owner-data");
        let file = std::fs::File::create(&kept).unwrap();
        file.set_len(if byte_limit { 512 * 1024 * 1024 } else { 42 })
            .unwrap();
        let error = install(&config, &reqwest::Client::new(), &request)
            .await
            .unwrap_err();
        assert_eq!(error.code, "agent_skill_upgrade_retention_full");
        assert!(kept.exists());
        assert_eq!(
            std::fs::read_to_string(parent.join("research/SKILL.md")).unwrap(),
            "# Research 1.0\n"
        );
        assert!(!stage.exists());
        cleanup(&config);
    }
}

#[tokio::test]
async fn remount_modes_after_proof_do_not_block_crash_recovery() {
    for point in ["proof_published", "exchanged"] {
        let (config, request) = fixture(true).await;
        FAULTS
            .lock()
            .unwrap()
            .insert(config.state_root.clone(), point);
        assert_eq!(
            install(&config, &reqwest::Client::new(), &request)
                .await
                .unwrap_err()
                .code,
            "injected_crash"
        );
        let target = config
            .agent_home(request.agent_id.as_str())
            .join("skills/research");
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o770)).unwrap();
        std::fs::set_permissions(
            target.join("scripts/helper.py"),
            std::fs::Permissions::from_mode(0o660),
        )
        .unwrap();
        std::fs::remove_dir_all(config.state_root.join("warm-skills")).unwrap();
        assert!(install(&config, &reqwest::Client::new(), &request)
            .await
            .unwrap()
            .matches(&request));
        cleanup(&config);
    }
}

#[tokio::test]
async fn cache_hit_survives_deleted_previous_version_and_cache_miss_names_the_failure() {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    let requests = Arc::new(AtomicUsize::new(0));
    let observed = requests.clone();
    let app = axum::Router::new().fallback(move || {
        observed.fetch_add(1, Ordering::SeqCst);
        async { axum::http::StatusCode::NOT_FOUND }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    for cached in [true, false] {
        let (mut config, request) = fixture(true).await;
        config.nyxid_url = Some(format!("http://{address}"));
        if !cached {
            std::fs::remove_file(
                config
                    .state_root
                    .join("warm-skills")
                    .join(&request.upgrade.as_ref().unwrap().previous.archive_sha256),
            )
            .unwrap();
        }
        let result = install(&config, &reqwest::Client::new(), &request).await;
        if cached {
            assert!(result.unwrap().matches(&request));
            assert_eq!(requests.load(Ordering::SeqCst), 0);
        } else {
            assert_eq!(
                result.unwrap_err().code,
                "agent_skill_upgrade_previous_unavailable"
            );
            assert_eq!(requests.load(Ordering::SeqCst), 1);
        }
        cleanup(&config);
    }
    server.abort();
}

#[tokio::test]
async fn changed_published_tree_keeps_recovery_pending_instead_of_claiming_the_old_version() {
    let (config, request) = fixture(true).await;
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
    let parent = config.agent_home(request.agent_id.as_str()).join("skills");
    let target = parent.join("research/SKILL.md");
    let published = std::fs::read(&target).unwrap();
    std::fs::write(&target, b"concurrent owner edit").unwrap();
    let error = install(&config, &reqwest::Client::new(), &request)
        .await
        .unwrap_err();
    assert_eq!(error.code, "agent_skill_upgrade_recovery_required");
    assert!(error.retryable);
    assert_eq!(std::fs::read(&target).unwrap(), b"concurrent owner edit");
    assert!(staging(&parent, &request).join("new/SKILL.md").exists());
    std::fs::write(&target, published).unwrap();
    assert!(install(&config, &reqwest::Client::new(), &request)
        .await
        .unwrap()
        .matches(&request));
    cleanup(&config);
}

#[tokio::test]
async fn excessive_tree_depth_is_refused_without_removing_owner_content() {
    let (config, request) = fixture(true).await;
    let target = config
        .agent_home(request.agent_id.as_str())
        .join("skills/research");
    let mut deep = target.clone();
    for _ in 0..66 {
        deep.push("nested");
    }
    std::fs::create_dir_all(&deep).unwrap();
    let note = deep.join("owner-note");
    std::fs::write(&note, b"preserve").unwrap();
    let error = install(&config, &reqwest::Client::new(), &request)
        .await
        .unwrap_err();
    assert_eq!(error.code, "agent_skill_upgrade_conflict");
    assert_eq!(std::fs::read(note).unwrap(), b"preserve");
    assert_eq!(
        std::fs::read_to_string(target.join("SKILL.md")).unwrap(),
        "# Research 1.0\n"
    );
    cleanup(&config);
}
