//! Existing encrypted credential decoders behind a port that cannot write.
use std::sync::Arc;

use super::*;

pub(crate) async fn open(
    client: &Client,
    database: &str,
    deployment: &str,
    encryption_key: &AuthEncryptionKey,
) -> Result<Arc<dyn AuthStore>, StoreError> {
    let database = client.database(database);
    let names = database
        .list_collection_names()
        .await
        .map_err(|_| closed())?;
    if [BROWSER_LOGINS, SUBJECTS]
        .iter()
        .any(|name| !names.iter().any(|n| n == name))
    {
        return Err(closed());
    }
    // Handle construction performs no DDL, index repair or credential mutation.
    // The concrete writable adapter is private and never escapes this wrapper.
    Ok(Arc::new(ReadOnlyAuthStore(MongoAuthStore {
        client: client.clone(),
        browser_logins: majority_collection(&database, BROWSER_LOGINS),
        login_attempts: majority_collection(&database, LOGIN_ATTEMPTS),
        subjects: majority_collection(&database, SUBJECTS),
        api_bearers: majority_collection(&database, API_BEARERS),
        actor_links: majority_collection(&database, ACTOR_LINKS),
        proxy_grants: majority_collection(&database, PROXY_GRANTS),
        cipher: AuthCipher::new(deployment, encryption_key)?,
    })))
}

struct ReadOnlyAuthStore(MongoAuthStore);

fn closed() -> StoreError {
    StoreError::Unavailable("inert authentication cannot mutate credential custody".into())
}

#[async_trait::async_trait]
impl AuthStore for ReadOnlyAuthStore {
    async fn browser_login(&self, id: &SecretDigest) -> Result<Option<BrowserLogin>, StoreError> {
        self.0.browser_login(id).await
    }

    async fn browser_session(
        &self,
        id: &SecretDigest,
    ) -> Result<Option<(BrowserLogin, Option<SubjectAuthState>)>, StoreError> {
        self.0.browser_session(id).await
    }

    async fn subject(&self, subject: &SubjectId) -> Result<Option<SubjectAuthState>, StoreError> {
        self.0.subject(subject).await
    }

    async fn insert_api_bearer(&self, _: ApiBearerCustody) -> Result<bool, StoreError> {
        Err(closed())
    }
    async fn api_bearer(&self, _: &SecretDigest) -> Result<Option<ApiBearerCustody>, StoreError> {
        Err(closed())
    }
    async fn put_login_attempt(&self, _: LoginAttempt) -> Result<(), StoreError> {
        Err(closed())
    }
    async fn take_login_attempt(
        &self,
        _: &SecretDigest,
        _: &SecretDigest,
    ) -> Result<Option<LoginAttempt>, StoreError> {
        Err(closed())
    }
    #[cfg(test)]
    async fn put_browser_login(&self, _: BrowserLogin) -> Result<(), StoreError> {
        Err(closed())
    }
    async fn delete_browser_login(&self, _: &SecretDigest) -> Result<(), StoreError> {
        Err(closed())
    }
    async fn insert_subject(&self, _: SubjectAuthState) -> Result<bool, StoreError> {
        Err(closed())
    }
    async fn replace_unfenced_subject(&self, _: SubjectAuthState) -> Result<bool, StoreError> {
        Err(closed())
    }
    async fn replace_subject(
        &self,
        _: &str,
        _: u64,
        _: SubjectAuthState,
    ) -> Result<bool, StoreError> {
        Err(closed())
    }
}
