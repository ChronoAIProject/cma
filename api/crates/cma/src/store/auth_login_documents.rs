//! Encrypted PKCE attempts and initiating-browser authority.
use super::*;

impl MongoAuthStore {
    pub(super) fn login_to_doc(
        &self,
        attempt: LoginAttempt,
    ) -> Result<LoginAttemptDoc, StoreError> {
        let id = *attempt.id.as_bytes();
        Ok(LoginAttemptDoc {
            id: binary(id),
            document_schema_version: DOCUMENT_SCHEMA_VERSION,
            browser_binding: binary(*attempt.browser_binding.as_bytes()),
            verifier: self
                .cipher
                .seal(LOGIN_KIND, &id, "verifier", attempt.verifier.expose())?,
            return_to: attempt.return_to,
            actor_link_intent: attempt
                .actor_link_intent
                .map(|intent| {
                    let serialized = serde_json::to_string(&intent)
                        .map_err(|_| StoreError::Malformed("actor login intent".into()))?;
                    self.cipher
                        .seal(LOGIN_KIND, &id, "actor_link_intent", &serialized)
                })
                .transpose()?,
            service_authorization: attempt
                .service_authorization
                .map(|intent| {
                    let serialized = serde_json::to_string(&intent).map_err(|_| {
                        StoreError::Malformed("service authorization intent".into())
                    })?;
                    self.cipher
                        .seal(LOGIN_KIND, &id, "service_authorization", &serialized)
                })
                .transpose()?,
            silent: attempt.silent,
            consent_requested: attempt.consent_requested,
            custody_reserved: attempt.custody_reserved,
            started_at: attempt.started_at.map(DateTime::from_system_time),
            expires_at: DateTime::from_system_time(attempt.expires_at),
        })
    }

    pub(super) fn browser_login_to_doc(login: BrowserLogin) -> BrowserLoginDoc {
        BrowserLoginDoc {
            id: binary(*login.id.as_bytes()),
            document_schema_version: DOCUMENT_SCHEMA_VERSION,
            subject: Some(login.subject.0),
            linked_session: None,
            binding_id: None,
            revoked: false,
            explicit_started_at: None,
            preference_revoked: false,
            expires_at: DateTime::from_system_time(login.expires_at),
        }
    }

    pub(super) fn login_from_doc(
        &self,
        document: LoginAttemptDoc,
    ) -> Result<LoginAttempt, StoreError> {
        schema::require_current(document.document_schema_version)?;
        let id = digest(&document.id, "login attempt")?;
        let browser_binding = digest(&document.browser_binding, "login browser binding")?;
        let verifier =
            self.cipher
                .open(LOGIN_KIND, id.as_bytes(), "verifier", document.verifier)?;
        let actor_link_intent = document
            .actor_link_intent
            .map(|envelope| {
                let plaintext =
                    self.cipher
                        .open(LOGIN_KIND, id.as_bytes(), "actor_link_intent", envelope)?;
                serde_json::from_str(&plaintext)
                    .map_err(|_| StoreError::Malformed("actor login intent".into()))
            })
            .transpose()?;
        let service_authorization = document
            .service_authorization
            .map(|envelope| {
                let plaintext = self.cipher.open(
                    LOGIN_KIND,
                    id.as_bytes(),
                    "service_authorization",
                    envelope,
                )?;
                serde_json::from_str(&plaintext)
                    .map_err(|_| StoreError::Malformed("service authorization intent".into()))
            })
            .transpose()?;
        Ok(LoginAttempt {
            id,
            browser_binding,
            verifier: SecretValue::new(verifier),
            return_to: document.return_to,
            actor_link_intent,
            service_authorization,
            silent: document.silent,
            consent_requested: document.consent_requested,
            custody_reserved: document.custody_reserved,
            started_at: document.started_at.map(|time| time.to_system_time()),
            expires_at: document.expires_at.to_system_time(),
        })
    }
}
