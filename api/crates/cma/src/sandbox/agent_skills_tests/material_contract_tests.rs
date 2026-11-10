use super::*;

#[tokio::test]
async fn anonymous_mode_resolves_public_material_without_authorization() {
    let registry = Registry::start().await;
    registry.reply(
        "skl_public",
        "1.0",
        Reply::json(material("skl_public", "1.0", &"e".repeat(64))),
    );

    let resolved = resolver(&registry, None)
        .resolve(&sandbox(), &[reference("skl_public", Some("1.0"))])
        .await
        .unwrap();

    assert_eq!(resolved.len(), 1);
    assert_eq!(registry.requests()[0].authorization, None);
}

#[tokio::test]
async fn selected_baseline_skills_resolve_and_retain_exact_archive_pins() {
    let registry = Registry::start().await;
    let ids = ["nyxid", "ornn-agent-manual-cli"];
    for id in ids {
        registry.reply(id, "", Reply::json(material(id, "2.0", &"b".repeat(64))));
    }
    let resolver = resolver(&registry, Some(TestCredentials::present()));
    let references = ids.map(|id| reference(id, None));
    let resolved = resolver.resolve(&sandbox(), &references).await.unwrap();
    validate_skill_selection(&references, &resolved).unwrap();
    let runtime = resolver.runtime_skills(&resolved).unwrap();
    for (id, skill) in ids.iter().zip(runtime) {
        assert_eq!(skill.id, *id);
        assert_eq!(skill.name.as_deref(), Some(*id));
        assert_eq!(skill.version.as_deref(), Some("2.0"));
        assert_eq!(skill.archive_sha256, "b".repeat(64));
        assert!(skill
            .source_url
            .ends_with(&format!("/{id}/versions/2.0/download")));
    }
}

#[tokio::test]
async fn malformed_runtime_ids_fail_before_authority_is_read() {
    let registry = Registry::start().await;
    let credentials = TestCredentials::present();
    let too_long = "x".repeat(65);
    for id in [
        "../escape",
        ".hidden",
        "has.dot",
        "has/slash",
        "has space",
        too_long.as_str(),
    ] {
        let error = resolver(&registry, Some(Arc::clone(&credentials)))
            .resolve(&sandbox(), &[reference(id, Some("1.0"))])
            .await
            .unwrap_err();
        assert!(matches!(error, AgentSkillResolutionError::Invalid(_)));
    }
    assert_eq!(credentials.calls.load(Ordering::Relaxed), 0);
    assert!(registry.requests().is_empty());
}

#[test]
fn production_constructor_requires_a_canonical_https_origin() {
    for invalid in [
        "http://platform.example",
        "https://platform.example/",
        "https://user@platform.example",
        "https://platform.example/path",
        "https://platform.example?query=1",
    ] {
        assert!(
            OrnnAgentSkillResolver::anonymous(invalid).is_err(),
            "{invalid}"
        );
    }
    assert!(OrnnAgentSkillResolver::anonymous(LOCATOR_ORIGIN).is_ok());
}
