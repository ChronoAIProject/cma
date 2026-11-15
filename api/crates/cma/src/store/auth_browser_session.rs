//! One indexed round trip for the browser handle and its current subject.
use futures_util::TryStreamExt;
use mongodb::bson::{deserialize_from_document, Document};

use super::*;

fn pipeline(id: &SecretDigest) -> [Document; 3] {
    [
        doc! {"$match": {"_id": binary(*id.as_bytes())}},
        // Keep the source document intact, including unrecognized additive fields.
        doc! {"$replaceWith": {"login": "$$ROOT"}},
        doc! {"$lookup": {"from": SUBJECTS, "localField": "login.subject",
        "foreignField": "_id", "as": "subjects"}},
    ]
}

impl MongoAuthStore {
    pub(super) async fn browser_session_record(
        &self,
        id: &SecretDigest,
    ) -> Result<Option<(BrowserLogin, Option<SubjectAuthState>)>, StoreError> {
        let mut cursor = self
            .browser_logins
            .aggregate(pipeline(id))
            .await
            .map_err(|_| StoreError::Unavailable("read browser session".into()))?;
        let Some(mut row) = cursor
            .try_next()
            .await
            .map_err(|_| StoreError::Unavailable("read browser session cursor".into()))?
        else {
            return Ok(None);
        };
        let document: BrowserLoginDoc = deserialize_from_document(
            row.remove("login")
                .and_then(|value| value.as_document().cloned())
                .ok_or_else(|| StoreError::Malformed("browser session login is invalid".into()))?,
        )
        .map_err(|_| StoreError::Unavailable("browser login document is invalid".into()))?;
        // Use the same schema, revocation and digest validation as the point read.
        let Some(login) = Self::browser_login_from_doc(document)? else {
            return Ok(None);
        };
        if login.expires_at <= SystemTime::now() {
            return Ok(Some((login, None)));
        }
        let subjects = row
            .get_array("subjects")
            .map_err(|_| StoreError::Malformed("browser session subjects are invalid".into()))?;
        let state = match subjects.as_slice() {
            [] => None,
            [mongodb::bson::Bson::Document(document)] => {
                Some(self.decode_session_subject(document.clone())?)
            }
            _ => {
                return Err(StoreError::Malformed(
                    "browser session subject is not unique".into(),
                ))
            }
        };
        Ok(Some((login, state)))
    }

    fn decode_session_subject(&self, document: Document) -> Result<SubjectAuthState, StoreError> {
        let document = deserialize_from_document(document)
            .map_err(|_| StoreError::Malformed("subject auth document is invalid".into()))?;
        self.subject_from_doc(document)
    }
}

#[cfg(test)]
#[path = "auth_browser_session_tests.rs"]
mod tests;
