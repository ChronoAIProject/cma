//! Browser custody helpers; session and binding cookie roles remain distinct.
use super::*;

impl AuthState {
    #[cfg(any(test, feature = "test-support"))]
    pub async fn issue_browser_login(&self, subject: SubjectId) -> Result<String, StoreError> {
        let value = token::random_token();
        self.store
            .put_browser_login(persistence::BrowserLogin {
                id: SecretDigest::identify(AUTH_COOKIE_DOMAIN, &value),
                subject,
                expires_at: SystemTime::now() + AUTH_COOKIE_LIFETIME,
            })
            .await?;
        Ok(value)
    }

    pub(super) async fn revoke_browser_binding(&self, value: &str) -> Result<(), StoreError> {
        self.identity_store.clear();
        self.store
            .revoke_browser_binding(&SecretDigest::identify(AUTH_COOKIE_DOMAIN, value))
            .await
    }

    #[cfg(any(test, feature = "test-support"))]
    pub async fn revoke_browser_login(&self, value: &str) -> Result<(), StoreError> {
        // Clear every local record and fence pending writes, including other
        // tokens for this browser. No credential is needed to perform eviction.
        self.identity_store.clear();
        self.store
            .delete_browser_login(&SecretDigest::identify(AUTH_COOKIE_DOMAIN, value))
            .await
    }
}
