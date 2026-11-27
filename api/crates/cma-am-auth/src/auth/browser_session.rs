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

    pub(super) async fn resolve_browser_session(
        &self,
        id: SecretDigest,
        login: BrowserLogin,
        state: Option<SubjectAuthState>,
        browser_proof: Option<&mut Option<relay::BrowserRelayProof>>,
    ) -> Result<Option<RequestAuth>, StoreError> {
        if login.expires_at <= SystemTime::now() {
            let _ = self.store.delete_browser_login(&id).await;
            return Ok(None);
        }
        let state = if self.browser_session_join_enabled {
            state
        } else {
            self.store.subject(&login.subject).await?
        };
        let Some(mut state) = state else {
            return Ok(Some(RequestAuth {
                binding: None,
                credential_source: layer2_core::resource_access::CredentialSource::Retained,
                subject: login.subject,
                authorization: Authorization::none(),
                profile: Profile::default(),
                delegated_credential: None,
            }));
        };
        if state.subject != login.subject {
            return Err(StoreError::Malformed(
                "browser authority crossed its identity boundary".into(),
            ));
        }
        let had_tokens = state.tokens.is_some();
        refresh::refresh_if_needed(self, &mut state).await?;
        if had_tokens && state.tokens.is_none() {
            return Ok(None);
        }
        let mut receipt = None;
        let resolved = cookie_authorization::request_for_browser_cookie_with_receipt(
            self,
            &login.subject,
            state,
            browser_proof.is_some().then_some(&mut receipt),
        )
        .await?;
        // Every browser verification rechecks logout after IO, including logout
        // committed by another replica.
        if !self.store.browser_login(&id).await?.is_some_and(|current| {
            current.subject == login.subject && current.expires_at > SystemTime::now()
        }) {
            return Ok(None);
        }
        if let (Some(slot), Some(receipt)) = (browser_proof, receipt) {
            *slot = Some(relay::BrowserRelayProof::capture(id, receipt));
        }
        Ok(resolved)
    }
}

#[cfg(test)]
#[path = "browser_session_tests.rs"]
mod tests;
