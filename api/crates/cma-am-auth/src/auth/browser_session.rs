//! Select the joined reader without changing the default request chain.
use super::{persistence::BrowserLogin, *};

impl AuthState {
    pub fn with_browser_session_join(mut self: Arc<Self>, enabled: bool) -> Arc<Self> {
        Arc::get_mut(&mut self)
            .expect("auth configured before sharing")
            .browser_session_join_enabled = enabled;
        self
    }

    pub(super) async fn read_browser_session(
        &self,
        id: &SecretDigest,
    ) -> Result<Option<(BrowserLogin, Option<SubjectAuthState>)>, StoreError> {
        if self.browser_session_join_enabled {
            return self.store.browser_session(id).await;
        }
        let Some(login) = self.store.browser_login(id).await? else {
            return Ok(None);
        };
        // Expiry and the subject point read remain ordered at the original call site.
        Ok(Some((login, None)))
    }
}

#[cfg(test)]
#[path = "browser_session_tests.rs"]
mod tests;
