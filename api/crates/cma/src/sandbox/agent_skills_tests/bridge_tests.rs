use super::*;

struct NoAmbientTokens;
#[async_trait::async_trait]
impl crate::sandbox::credential::OwnerTokenSource for NoAmbientTokens {
    async fn token_for(
        &self,
        _: &layer2_core::SubjectId,
    ) -> Result<Option<crate::auth::persistence::SecretValue>, String> {
        panic!("Bridge preflight must never look up ambient owner credentials")
    }
}

#[tokio::test]
async fn bridge_public_skill_preflight_uses_only_explicit_human_lease_and_refuses_private_or_changed_pin(
) {
    let registry = Registry::start().await;
    let resolver =
        Arc::new(resolver(&registry, None)).with_warmup_tokens(Arc::new(NoAmbientTokens));
    let references = [reference("skill", Some("0.1"))];
    let mut public = material("skill", "0.1", &"a".repeat(64));
    public["data"]["isPrivate"] = json!(false);
    registry.reply("skill", "0.1", Reply::json(public.clone()));
    let result = resolver
        .resolve_public_with_credential("explicit-current-human-oauth", &references)
        .await
        .unwrap();
    assert_eq!(result[0].version, "0.1");
    assert_eq!(result[0].archive_sha256, "a".repeat(64));
    for field in ["private", "version", "hash"] {
        let mut wrong = public.clone();
        match field {
            "private" => wrong["data"]["isPrivate"] = json!(true),
            "version" => wrong["data"]["version"] = json!("0.2"),
            _ => wrong["data"]["skillHash"] = json!("invalid"),
        }
        registry.reply("skill", "0.1", Reply::json(wrong));
        assert!(
            resolver
                .resolve_public_with_credential("explicit-current-human-oauth", &references)
                .await
                .is_err(),
            "{field}"
        );
    }
    assert_eq!(registry.requests().len(), 4);
    assert!(registry
        .requests()
        .iter()
        .all(|request| request.authorization.as_deref()
            == Some("Bearer explicit-current-human-oauth")));
    let runtime = resolver.runtime_skills(&result).unwrap();
    assert!(!serde_json::to_string(&runtime)
        .unwrap()
        .contains("explicit-current-human-oauth"));
}
