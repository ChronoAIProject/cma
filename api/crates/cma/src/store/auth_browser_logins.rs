//! Fresh sessions and binding links commit together; either browser handle revokes both.
use super::*;
use crate::store::sandbox_mongo::{
    commit_transaction, is_transient, start_transaction, CommitOutcome, TRANSACTION_ATTEMPTS,
};
use crate::store::transaction_terminal::GuardedTerminal as _;
use mongodb::ClientSession;

pub(super) enum Mutation<'a> {
    Complete(&'a SecretDigest, &'a BrowserLogin, Option<SystemTime>),
    Revoke(&'a SecretDigest, bool, bool),
}

enum Failure {
    Driver(mongodb::error::Error),
    Invalid(StoreError),
}
impl From<mongodb::error::Error> for Failure {
    fn from(error: mongodb::error::Error) -> Self {
        Self::Driver(error)
    }
}
impl From<StoreError> for Failure {
    fn from(error: StoreError) -> Self {
        Self::Invalid(error)
    }
}
fn unavailable(error: impl std::fmt::Display) -> StoreError {
    StoreError::Unavailable(format!("browser custody: {error}"))
}

impl MongoAuthStore {
    pub(super) async fn reserve_browser_login_record(
        &self,
        id: SecretDigest,
        expires_at: SystemTime,
    ) -> Result<bool, StoreError> {
        let row = BrowserLoginDoc {
            id: binary(*id.as_bytes()),
            document_schema_version: DOCUMENT_SCHEMA_VERSION,
            subject: None,
            linked_session: None,
            binding_id: None,
            revoked: false,
            explicit_started_at: None,
            preference_revoked: false,
            expires_at: DateTime::from_system_time(expires_at),
        };
        match self.browser_logins.insert_one(row).await {
            Ok(_) => Ok(true),
            Err(error) if crate::store::sandbox_mongo::is_duplicate_key(&error) => Ok(false),
            Err(error) => Err(unavailable(error)),
        }
    }

    pub(super) async fn renew_browser_login_record(
        &self,
        binding: &SecretDigest,
        expires_at: SystemTime,
    ) -> Result<bool, StoreError> {
        let result = self
            .browser_logins
            .update_one(
                Self::pending_browser(binding),
                doc! {"$max":{"expires_at":DateTime::from_system_time(expires_at)}},
            )
            .await
            .map_err(unavailable)?;
        Ok(result.matched_count == 1)
    }

    fn pending_browser(binding: &SecretDigest) -> mongodb::bson::Document {
        doc! {"_id":binary(*binding.as_bytes()), "document_schema_version":DOCUMENT_SCHEMA_VERSION,
        "subject":{"$exists":false}, "linked_session":{"$exists":false}, "revoked":{"$ne":true},
        "expires_at":{"$gt":DateTime::now()}}
    }

    pub(super) async fn complete_browser_login_record(
        &self,
        binding: &SecretDigest,
        login: BrowserLogin,
        explicit_started_at: Option<SystemTime>,
    ) -> Result<bool, StoreError> {
        if binding == &login.id {
            return Err(StoreError::Malformed(
                "session must differ from browser binding".into(),
            ));
        }
        self.mutate_browser_custody(Mutation::Complete(binding, &login, explicit_started_at))
            .await
    }

    async fn mutate_browser_custody(&self, operation: Mutation<'_>) -> Result<bool, StoreError> {
        for _ in 0..TRANSACTION_ATTEMPTS {
            let mut session = self.client.start_session().await.map_err(unavailable)?;
            start_transaction(&mut session).await.map_err(unavailable)?;
            let result = match &operation {
                Mutation::Complete(binding, login, started) => {
                    self.complete_browser_transaction(binding, login, *started, &mut session)
                        .await
                }
                Mutation::Revoke(id, binding, suppress) => {
                    self.revoke_browser_transaction(id, *binding, *suppress, &mut session)
                        .await
                }
            };
            match result {
                Ok(false) => {
                    let _ = session.abort_guarded().await;
                    return Ok(false);
                }
                Ok(true) => match commit_transaction(&mut session).await {
                    CommitOutcome::Committed => return Ok(true),
                    CommitOutcome::Retry(_) => continue,
                    CommitOutcome::Unknown(error) | CommitOutcome::Failed(error) => {
                        return Err(unavailable(error))
                    }
                },
                Err(error) => {
                    let _ = session.abort_guarded().await;
                    match error {
                        Failure::Driver(error) if is_transient(&error) => continue,
                        Failure::Driver(error) => return Err(unavailable(error)),
                        Failure::Invalid(error) => return Err(error),
                    }
                }
            }
        }
        Err(unavailable("transaction retry budget exhausted"))
    }

    async fn complete_browser_transaction(
        &self,
        binding: &SecretDigest,
        login: &BrowserLogin,
        explicit_started_at: Option<SystemTime>,
        session: &mut ClientSession,
    ) -> Result<bool, Failure> {
        if !self
            .link_browser_reservation(binding, login, explicit_started_at, session)
            .await?
        {
            return Ok(false);
        }
        self.insert_linked_browser_login(binding, login, session)
            .await?;
        Ok(true)
    }

    // Separate transaction steps permit deterministic tests of intermediate visibility.
    pub(super) async fn link_browser_reservation(
        &self,
        binding: &SecretDigest,
        login: &BrowserLogin,
        explicit_started_at: Option<SystemTime>,
        session: &mut ClientSession,
    ) -> Result<bool, mongodb::error::Error> {
        let expiry = if explicit_started_at.is_some() {
            login
                .expires_at
                .max(SystemTime::now() + crate::auth::SIGN_IN_PREFERENCE_LIFETIME)
        } else {
            login.expires_at
        };
        let mut fields = doc! {"linked_session":binary(*login.id.as_bytes()),"expires_at":DateTime::from_system_time(expiry)};
        if let Some(started) = explicit_started_at {
            fields.insert("explicit_started_at", DateTime::from_system_time(started));
        }
        let result = self
            .browser_logins
            .update_one(Self::pending_browser(binding), doc! {"$set":fields})
            .session(session)
            .await?;
        Ok(result.matched_count == 1)
    }

    pub(super) async fn insert_linked_browser_login(
        &self,
        binding: &SecretDigest,
        login: &BrowserLogin,
        session: &mut ClientSession,
    ) -> Result<(), mongodb::error::Error> {
        let mut row = Self::browser_login_to_doc(login.clone());
        row.binding_id = Some(binary(*binding.as_bytes()));
        self.browser_logins.insert_one(row).session(session).await?;
        Ok(())
    }

    async fn revoke_browser_transaction(
        &self,
        id: &SecretDigest,
        binding_cookie: bool,
        suppress_preference: bool,
        session: &mut ClientSession,
    ) -> Result<bool, Failure> {
        let row = self
            .browser_logins
            .find_one(doc! {"_id":binary(*id.as_bytes())})
            .session(&mut *session)
            .await?;
        if let Some(row) = &row {
            schema::require_current(row.document_schema_version)?;
        }
        let (binding, login) = match row {
            Some(row) if row.subject.is_some() => (
                row.binding_id
                    .as_ref()
                    .map(|value| digest(value, "browser binding"))
                    .transpose()?,
                Some(id.clone()),
            ),
            Some(row) => (
                Some(id.clone()),
                row.linked_session
                    .as_ref()
                    .map(|value| digest(value, "linked browser login"))
                    .transpose()?,
            ),
            None => (binding_cookie.then(|| id.clone()), None),
        };
        if let Some(binding) = binding {
            let previous = self
                .browser_logins
                .find_one(doc! {"_id":binary(*binding.as_bytes())})
                .session(&mut *session)
                .await?;
            let explicit_started_at = previous.as_ref().and_then(|row| row.explicit_started_at);
            let preference_revoked =
                suppress_preference || previous.as_ref().is_some_and(|row| row.preference_revoked);
            let expiry = DateTime::from_system_time(
                SystemTime::now() + crate::auth::BROWSER_BINDING_LIFETIME,
            )
            .max(
                previous
                    .as_ref()
                    .map_or(DateTime::MIN, |row| row.expires_at),
            );
            self.browser_logins.replace_one(doc!{"_id":binary(*binding.as_bytes()),"document_schema_version":DOCUMENT_SCHEMA_VERSION},
                BrowserLoginDoc {id:binary(*binding.as_bytes()),document_schema_version:DOCUMENT_SCHEMA_VERSION,subject:None,linked_session:None,binding_id:None,revoked:true,
                    explicit_started_at,preference_revoked,expires_at:expiry})
                .upsert(true).session(&mut *session).await?;
        }
        if let Some(login) = login {
            self.browser_logins.delete_one(doc! {"_id":binary(*login.as_bytes()),"document_schema_version":DOCUMENT_SCHEMA_VERSION})
                .session(session).await?;
        }
        Ok(true)
    }

    pub(super) async fn revoke_browser_custody(
        &self,
        id: &SecretDigest,
        binding_cookie: bool,
        suppress_preference: bool,
    ) -> Result<(), StoreError> {
        self.mutate_browser_custody(Mutation::Revoke(id, binding_cookie, suppress_preference))
            .await
            .map(|_| ())
    }

    #[cfg(test)]
    pub(super) async fn put_browser_login_record(
        &self,
        login: BrowserLogin,
    ) -> Result<(), StoreError> {
        self.browser_logins
            .insert_one(Self::browser_login_to_doc(login))
            .await
            .map_err(unavailable)?;
        Ok(())
    }

    pub(super) async fn read_browser_preference(
        &self,
        id: &SecretDigest,
        started_at: SystemTime,
    ) -> Result<bool, StoreError> {
        Ok(self.browser_logins.find_one(doc!{"_id":binary(*id.as_bytes()),"document_schema_version":DOCUMENT_SCHEMA_VERSION,
            "explicit_started_at":DateTime::from_system_time(started_at),"preference_revoked":{"$ne":true},"expires_at":{"$gt":DateTime::now()}}).await.map_err(unavailable)?.is_some())
    }

    pub(super) async fn browser_login_record(
        &self,
        id: &SecretDigest,
    ) -> Result<Option<BrowserLogin>, StoreError> {
        self.browser_logins
            .find_one(doc! {"_id":binary(*id.as_bytes())})
            .await
            .map_err(unavailable)?
            .map(Self::browser_login_from_doc)
            .transpose()
            .map(Option::flatten)
    }
    pub(super) fn browser_login_from_doc(
        document: BrowserLoginDoc,
    ) -> Result<Option<BrowserLogin>, StoreError> {
        schema::require_current(document.document_schema_version)?;
        let Some(subject) = document.subject.filter(|_| !document.revoked) else {
            return Ok(None);
        };
        Ok(Some(BrowserLogin {
            id: digest(&document.id, "browser login")?,
            subject: SubjectId::new(subject),
            expires_at: document.expires_at.to_system_time(),
        }))
    }

    pub(super) async fn delete_browser_login_record(
        &self,
        id: &SecretDigest,
    ) -> Result<(), StoreError> {
        self.revoke_browser_custody(id, false, true).await
    }
}
