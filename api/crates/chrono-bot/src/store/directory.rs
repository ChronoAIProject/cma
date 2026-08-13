//! The record of who has used the service, in MongoDB.
//!
//! Keyed by subject, so a person has one entry however many times they sign
//! in. The write is an update rather than a replace for one reason: the first
//! time somebody was seen is the fact this collection exists to hold, and a
//! replace would overwrite it on every sign-in and make everybody look new.
//!
//! Nothing here logs a name or an email address. They are details about a
//! person, and the process log is not where those belong.

use futures_util::TryStreamExt;
use layer2_core::{
    store::{StoreError, UserDirectoryStore},
    DirectoryQuery, RegisteredUser, SubjectId,
};
use mongodb::bson::doc;
use mongodb::options::{IndexOptions, UpdateOptions};
use mongodb::{Client, Collection, IndexModel};
use serde::{Deserialize, Serialize};

const USERS: &str = "users";

/// How a person is shaped on the wire.
///
/// A distinct type from the core's model on purpose: the core owns its model
/// and this owns how it is written. `_id` carries the subject, so the database
/// enforces one entry per person rather than the application.
#[derive(Debug, Serialize, Deserialize)]
struct UserDoc {
    #[serde(rename = "_id")]
    subject: String,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    email: Option<String>,
    first_seen: String,
    last_seen: String,
}

fn from_doc(d: UserDoc) -> RegisteredUser {
    RegisteredUser {
        subject: SubjectId(d.subject),
        display_name: d.display_name,
        email: d.email,
        first_seen: d.first_seen,
        last_seen: d.last_seen,
    }
}

/// A MongoDB-backed directory of subjects.
pub struct MongoUserDirectoryStore {
    users: Collection<UserDoc>,
}

impl MongoUserDirectoryStore {
    /// Open the store over an existing connection, ensuring its index exists.
    pub(super) async fn open(client: &Client, database: &str) -> Result<Self, StoreError> {
        let store = Self {
            users: client.database(database).collection(USERS),
        };
        store.ensure_indexes().await?;
        tracing::info!(database, "user directory ready");
        Ok(store)
    }

    async fn ensure_indexes(&self) -> Result<(), StoreError> {
        // The only listing this collection serves: everybody, most recently
        // seen first.
        let recent = IndexModel::builder()
            .keys(doc! { "last_seen": -1 })
            .options(IndexOptions::builder().build())
            .build();
        self.users
            .create_index(recent)
            .await
            .map_err(|e| StoreError::Unavailable(format!("user index: {e}")))?;
        Ok(())
    }
}

#[async_trait::async_trait]
impl UserDirectoryStore for MongoUserDirectoryStore {
    async fn record_sign_in(&self, user: &RegisteredUser) -> Result<(), StoreError> {
        // The details the provider released now, and the earliest arrival we
        // have ever recorded. Splitting the write this way is what keeps
        // `first_seen` meaning what it says: it is set when the document is
        // created and never touched again.
        let update = doc! {
            "$set": {
                "display_name": user.display_name.as_deref(),
                "email": user.email.as_deref(),
                "last_seen": &user.last_seen,
            },
            "$setOnInsert": { "first_seen": &user.first_seen },
        };
        self.users
            .update_one(doc! { "_id": &user.subject.0 }, update)
            .with_options(UpdateOptions::builder().upsert(true).build())
            .await
            .map_err(|e| StoreError::Unavailable(format!("record sign-in: {e}")))?;
        Ok(())
    }

    async fn recent(&self, query: &DirectoryQuery) -> Result<Vec<RegisteredUser>, StoreError> {
        // The limit goes to the server. Reading the collection and truncating
        // here would leave the cost this bound exists to remove.
        let cursor = self
            .users
            .find(doc! {})
            .sort(doc! { "last_seen": -1 })
            .limit(i64::try_from(query.limit).unwrap_or(i64::MAX))
            .await
            .map_err(|e| StoreError::Unavailable(format!("list users: {e}")))?;
        collect(cursor).await
    }

    async fn by_subjects(&self, subjects: &[SubjectId]) -> Result<Vec<RegisteredUser>, StoreError> {
        if subjects.is_empty() {
            return Ok(Vec::new());
        }
        // Bounded by what was asked about, and served from the `_id` index the
        // collection already has.
        let ids: Vec<&str> = subjects.iter().map(SubjectId::as_str).collect();
        let cursor = self
            .users
            .find(doc! { "_id": { "$in": ids } })
            .await
            .map_err(|e| StoreError::Unavailable(format!("name users: {e}")))?;
        collect(cursor).await
    }
}

/// Drain a cursor into the core's model.
async fn collect(mut cursor: mongodb::Cursor<UserDoc>) -> Result<Vec<RegisteredUser>, StoreError> {
    let mut out = Vec::new();
    while let Some(d) = cursor
        .try_next()
        .await
        .map_err(|e| StoreError::Unavailable(format!("user cursor: {e}")))?
    {
        out.push(from_doc(d));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc_for(subject: &str) -> UserDoc {
        UserDoc {
            subject: subject.to_string(),
            display_name: Some("Ada Lovelace".into()),
            email: Some("ada@example.com".into()),
            first_seen: "2026-08-01T00:00:00Z".into(),
            last_seen: "2026-08-14T00:00:00Z".into(),
        }
    }

    #[test]
    fn a_person_reads_back_with_everything_a_surface_needs_to_name_them() {
        let user = from_doc(doc_for("76fe9d91"));
        assert_eq!(user.subject, SubjectId("76fe9d91".into()));
        assert_eq!(user.label(), Some("Ada Lovelace"));
        assert_eq!(user.first_seen, "2026-08-01T00:00:00Z");
    }

    #[test]
    fn a_document_written_before_a_field_existed_still_reads() {
        // A provider that released no details leaves them absent, and an entry
        // stored by an older build must not become unreadable.
        let doc: UserDoc = serde_json::from_str(
            r#"{"_id":"76fe9d91","first_seen":"2026-01-01T00:00:00Z",
                "last_seen":"2026-01-02T00:00:00Z"}"#,
        )
        .expect("parses");
        let user = from_doc(doc);
        assert!(user.display_name.is_none());
        assert!(user.email.is_none());
        assert_eq!(user.label(), None);
    }

    #[test]
    fn the_subject_is_the_key_so_one_person_has_one_entry() {
        let json = serde_json::to_value(doc_for("76fe9d91")).expect("serialises");
        assert_eq!(json["_id"], "76fe9d91");
        assert!(
            json.get("subject").is_none(),
            "the subject is the key, not a second field to disagree with it: {json}"
        );
    }
}
