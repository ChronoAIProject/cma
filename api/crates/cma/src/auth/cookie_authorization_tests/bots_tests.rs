use super::*;

#[tokio::test]
async fn bot_admission_requires_the_actual_renewable_login_and_current_permission() {
    let f = Fixture::new().await;
    *f.control.introspection.write().await =
        Reply::json(json!({"active":true,"sub":"owner","permissions":["cma:bot:manage"]}));
    let original = f.seed().await;
    let mut request = f.request(original.clone()).await.unwrap();
    assert!(f.auth.authorize_bot_request(&request).await.unwrap());
    assert!(f.auth.bot_owner_token(&original.subject).await.is_ok());
    request.delegated_credential = Some(format!("nyx_{}", uuid::Uuid::new_v4().simple()));
    assert!(!f.auth.authorize_bot_request(&request).await.unwrap());
    request.delegated_credential = Some("another-human-login".into());
    assert!(!f.auth.authorize_bot_request(&request).await.unwrap());
    let request = f.request(original.clone()).await.unwrap();
    let mut replacement = original.clone();
    replacement.tokens.as_mut().unwrap().refresh_token = None;
    f.store.put_subject(replacement).await.unwrap();
    assert!(!f.auth.authorize_bot_request(&request).await.unwrap());
    assert!(f.auth.bot_owner_token(&original.subject).await.is_err());
    f.store.put_subject(original.clone()).await.unwrap();
    *f.control.introspection.write().await =
        Reply::json(json!({"active":true,"sub":"owner","permissions":[]}));
    assert!(f.auth.bot_owner_token(&original.subject).await.is_err());
    f.assert_unchanged(&original).await;
}
