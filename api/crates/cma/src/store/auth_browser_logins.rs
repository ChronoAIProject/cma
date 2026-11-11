//! Browser custody: pending reservations never authenticate and promotion never upserts.
use super::*;

impl MongoAuthStore {
    pub(super) async fn reserve_browser_login_record(
        &self,
        id: SecretDigest,
        expires_at: SystemTime,
    ) -> Result<(), StoreError> {
        self.browser_logins
            .insert_one(BrowserLoginDoc {
                id: binary(*id.as_bytes()),
                document_schema_version: DOCUMENT_SCHEMA_VERSION,
                subject: None,
                expires_at: DateTime::from_system_time(expires_at),
            })
            .await
            .map_err(|error| StoreError::Unavailable(format!("reserve browser login: {error}")))?;
        Ok(())
    }

    pub(super) async fn complete_browser_login_record(
        &self,
        login: BrowserLogin,
    ) -> Result<bool, StoreError> {
        let result = self
            .browser_logins
            .replace_one(
                doc! {
                    "_id": binary(*login.id.as_bytes()),
                    "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                    "subject": {"$exists": false},
                    "expires_at": {"$gt": DateTime::now()},
                },
                Self::browser_login_to_doc(login),
            )
            .await
            .map_err(|error| StoreError::Unavailable(format!("complete browser login: {error}")))?;
        Ok(result.matched_count == 1)
    }

    #[cfg(test)]
    pub(super) async fn put_browser_login_record(
        &self,
        login: BrowserLogin,
    ) -> Result<(), StoreError> {
        let id = binary(*login.id.as_bytes());
        self.browser_logins
            .replace_one(
                doc! {
                    "_id": &id,
                    "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                },
                Self::browser_login_to_doc(login),
            )
            .upsert(true)
            .await
            .map_err(|error| StoreError::Unavailable(format!("store browser login: {error}")))?;
        Ok(())
    }

    pub(super) async fn browser_login_record(
        &self,
        id: &SecretDigest,
    ) -> Result<Option<BrowserLogin>, StoreError> {
        self.browser_logins
            .find_one(doc! { "_id": binary(*id.as_bytes()) })
            .await
            .map_err(|error| StoreError::Unavailable(format!("read browser login: {error}")))?
            .map(|document| {
                schema::require_current(document.document_schema_version)?;
                let Some(subject) = document.subject else {
                    return Ok(None);
                };
                Ok(Some(BrowserLogin {
                    id: digest(&document.id, "browser login")?,
                    subject: SubjectId::new(subject),
                    expires_at: document.expires_at.to_system_time(),
                }))
            })
            .transpose()
            .map(Option::flatten)
    }

    pub(super) async fn delete_browser_login_record(
        &self,
        id: &SecretDigest,
    ) -> Result<(), StoreError> {
        self.browser_logins
            .delete_one(doc! {
                "_id": binary(*id.as_bytes()),
                "document_schema_version": DOCUMENT_SCHEMA_VERSION,
            })
            .await
            .map_err(|error| StoreError::Unavailable(format!("delete browser login: {error}")))?;
        Ok(())
    }
}
