use super::*;
use crate::auth::persistence::SecretValue;
use crate::sandbox::credential::OwnerTokenSource;

struct Publisher;
#[async_trait::async_trait]
impl OwnerTokenSource for Publisher {
    async fn token_for(&self, _: &layer2_core::SubjectId) -> Result<Option<SecretValue>, String> {
        Ok(Some(SecretValue::new("test-publisher-token".into())))
    }
}

#[tokio::test]
async fn warm_skills_require_explicit_public_visibility_and_verified_archive_bytes() {
    use sha2::{Digest, Sha256};
    let registry = Registry::start().await;
    let resolver = Arc::new(resolver(&registry, None)).with_warmup_tokens(Arc::new(Publisher));
    let owner = layer2_core::SubjectId::new("publisher");
    let bytes = b"public archive bytes";
    let hash = format!("{:x}", Sha256::digest(bytes));
    let mut public = material("skill", "0.1", &hash);
    public["data"]["isPrivate"] = json!(false);
    registry.reply("skill", "0.1", Reply::json(public.clone()));
    let selected = resolver
        .resolve_warm(&owner, &[reference("skill", Some("0.1"))])
        .await
        .unwrap();
    assert_eq!(selected[0].archive_sha256, hash);
    registry.reply("skill", "0.1", Reply::json(public.clone()));
    let url = skill_download_url(
        &reqwest::Url::parse(&registry.origin).unwrap(),
        "skill",
        "0.1",
    )
    .unwrap();
    let target = url
        .query()
        .map_or_else(|| url.path().to_string(), |q| format!("{}?{q}", url.path()));
    registry
        .state
        .replies
        .lock()
        .unwrap()
        .entry(target.clone())
        .or_default()
        .push_back(Reply {
            status: StatusCode::OK,
            body: bytes.to_vec(),
        });
    let archives = resolver.warm_archives(&owner, &selected).await.unwrap();
    assert_eq!(archives[0].bytes, bytes);
    registry.reply("skill", "0.1", Reply::json(public));
    registry
        .state
        .replies
        .lock()
        .unwrap()
        .entry(target)
        .or_default()
        .push_back(Reply {
            status: StatusCode::OK,
            body: b"tampered".to_vec(),
        });
    assert!(resolver.warm_archives(&owner, &selected).await.is_err());
    for visibility in [None, Some(true)] {
        let mut private = material("skill", "0.1", &hash);
        if let Some(value) = visibility {
            private["data"]["isPrivate"] = json!(value);
        }
        registry.reply("skill", "0.1", Reply::json(private));
        assert!(resolver
            .resolve_warm(&owner, &[reference("skill", Some("0.1"))])
            .await
            .is_err());
    }
    assert!(registry
        .requests()
        .iter()
        .all(|r| r.authorization.as_deref() == Some("Bearer test-publisher-token")));
}
