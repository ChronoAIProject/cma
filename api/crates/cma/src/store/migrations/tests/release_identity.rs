use super::*;

fn image(component: &str) -> String {
    format!(
        "registry.example/release/{component}@sha256:{}",
        "b".repeat(64)
    )
}

#[test]
fn migration_identity_accepts_published_and_legacy_agent_manager_images() {
    for component in ["cma-agent-manager", "cma-api"] {
        assert!(
            ledger::Identity::new(&"a".repeat(40), &image(component)).is_ok(),
            "immutable {component} image must identify an Agent Manager release"
        );
    }
}

#[test]
fn migration_identity_keeps_release_digest_and_service_boundaries() {
    for component in [
        "cma-event-gateway",
        "cma-image-manager",
        "cma-runtime",
        "cma-web",
        "other-cma-agent-manager",
        "cma-agent-manager-extra",
    ] {
        assert!(ledger::Identity::new(&"a".repeat(40), &image(component)).is_err());
    }
    for component in ["cma-agent-manager", "cma-api"] {
        for source in ["development".into(), "a".repeat(39), "G".repeat(40)] {
            assert!(ledger::Identity::new(&source, &image(component)).is_err());
        }
        for invalid_image in [
            format!("registry.example/release/{component}:current"),
            format!(
                "registry.example/release/{component}@sha256:{}",
                "b".repeat(63)
            ),
            format!(
                "registry.example/release/{component}@sha256:{}",
                "G".repeat(64)
            ),
            format!("{component}@sha256:{}", "b".repeat(64)),
            format!("{}:current", image(component)),
        ] {
            assert!(ledger::Identity::new(&"a".repeat(40), &invalid_image).is_err());
        }
    }
}

#[tokio::test]
#[ignore = "requires disposable replica-set MongoDB"]
async fn published_release_resumes_legacy_attempt_and_preserves_both_identities() {
    let legacy_source = "a".repeat(40);
    let legacy_image = image("cma-api");
    let current_source = "c".repeat(40);
    let current_image = image("cma-agent-manager");
    let legacy = ledger::Identity::new(&legacy_source, &legacy_image).unwrap();
    let current = ledger::Identity::new(&current_source, &current_image).unwrap();
    let (_, database, ledger) = fixture().await;
    ledger.initialize().await.unwrap();
    let abandoned = ledger.claim(&legacy).await.unwrap();
    activity_indexes::create(&database, activity_indexes::indexes()[0].clone())
        .await
        .unwrap();
    expire(&database).await;

    let result = apply_indexes(&ledger, &database, &current).await.unwrap();
    assert_eq!(result["outcome"], "completed");
    assert_eq!(result["created_indexes"], 1);
    assert!(ledger.finish(&abandoned, Ok(2)).await.is_err());
    assert!(activity_indexes::inspect(&database)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        apply_indexes(&ledger, &database, &current).await.unwrap()["outcome"],
        "already_completed"
    );

    let status = ledger.status().await.unwrap();
    assert_eq!(status["completed"], true);
    let attempts = status["recent_attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 2);
    assert!(attempts.iter().any(|row| row["source"] == legacy_source
        && row["image"] == legacy_image
        && row["outcome"] == "running"));
    assert!(attempts.iter().any(|row| row["source"] == current_source
        && row["image"] == current_image
        && row["outcome"] == "completed"
        && row["created_indexes"] == 1));
    database.drop().await.unwrap();
}
