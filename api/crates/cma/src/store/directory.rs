//! MongoDB persistence for the retained public user directory.

use futures_util::TryStreamExt as _;
use layer2_core::{
    DirectoryQuery, Page, PublicIdentity, RegisteredUser, StoreError, SubjectId, UserDirectoryStore,
};
use mongodb::{
    bson::{doc, DateTime, Document},
    options::UpdateOptions,
    Client, Collection, IndexModel,
};
use serde::{Deserialize, Serialize};

use super::{
    durability::majority_collection,
    schema::{self, DOCUMENT_SCHEMA_VERSION},
};

pub(super) const USERS: &str = "users";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct UserDoc {
    #[serde(rename = "_id")]
    subject: String,
    document_schema_version: i64,
    display_name: Option<String>,
    avatar_url: Option<String>,
    first_seen_at: String,
    last_seen_at: String,
    observed_at: DateTime,
}

impl UserDoc {
    fn into_record(self) -> RegisteredUser {
        RegisteredUser {
            identity: PublicIdentity {
                subject: SubjectId::new(self.subject),
                display_name: self.display_name,
                avatar_url: self.avatar_url,
            },
            first_seen_at: self.first_seen_at,
            last_seen_at: self.last_seen_at,
        }
    }

    fn cursor(&self) -> String {
        format!("{}:{}", self.observed_at.timestamp_millis(), self.subject)
    }
}

/// Durable directory adapter keyed by identity-provider subject.
pub struct MongoUserDirectoryStore {
    users: Collection<UserDoc>,
}

impl MongoUserDirectoryStore {
    pub(super) async fn open(client: &Client, database: &str) -> Result<Self, StoreError> {
        let database = client.database(database);
        schema::initialize_collections(&database, &[USERS]).await?;
        let store = Self {
            users: majority_collection(&database, USERS),
        };
        store
            .users
            .create_index(
                IndexModel::builder()
                    .keys(doc! { "observed_at": -1, "_id": -1 })
                    .build(),
            )
            .await
            .map_err(|error| StoreError::Unavailable(format!("directory index: {error}")))?;
        Ok(store)
    }
}

#[async_trait::async_trait]
impl UserDirectoryStore for MongoUserDirectoryStore {
    async fn observe(&self, identity: &PublicIdentity) -> Result<(), StoreError> {
        let at = layer2_core::now_rfc3339();
        let observed_at = DateTime::now();
        self.users
            .update_one(
                doc! {
                    "_id": identity.subject.as_str(),
                    "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                },
                doc! {
                    "$set": {
                        "display_name": identity.display_name.as_deref(),
                        "avatar_url": identity.avatar_url.as_deref(),
                        "last_seen_at": &at,
                        "observed_at": observed_at,
                    },
                    "$setOnInsert": {
                        "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                        "first_seen_at": &at,
                    },
                },
            )
            .with_options(UpdateOptions::builder().upsert(true).build())
            .await
            .map_err(|error| StoreError::Unavailable(format!("observe directory user: {error}")))?;
        Ok(())
    }

    async fn list(&self, query: &DirectoryQuery) -> Result<Page<RegisteredUser>, StoreError> {
        query
            .validate()
            .map_err(|error| StoreError::Malformed(error.to_string()))?;
        let mut clauses = Vec::<Document>::new();
        if let Some(text) = query
            .text
            .as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty())
        {
            let expression = regex_literal(text);
            clauses.push(doc! {
                "$or": [
                    { "_id": { "$regex": &expression, "$options": "i" } },
                    { "display_name": { "$regex": &expression, "$options": "i" } },
                ]
            });
        }
        if let Some(cursor) = query.page.cursor.as_deref() {
            let (observed_at, subject) = decode_cursor(cursor)?;
            clauses.push(doc! {
                "$or": [
                    { "observed_at": { "$lt": observed_at } },
                    { "observed_at": observed_at, "_id": { "$lt": subject } },
                ]
            });
        }
        let filter = match clauses.len() {
            0 => doc! {},
            1 => clauses.pop().expect("one clause exists"),
            _ => doc! { "$and": clauses },
        };
        let mut cursor = self
            .users
            .find(filter)
            .sort(doc! { "observed_at": -1, "_id": -1 })
            .limit(i64::from(query.page.limit.saturating_add(1)))
            .await
            .map_err(|error| StoreError::Unavailable(format!("list directory: {error}")))?;
        let mut rows = Vec::new();
        while let Some(row) = cursor
            .try_next()
            .await
            .map_err(|error| StoreError::Unavailable(format!("list directory cursor: {error}")))?
        {
            schema::require_current(row.document_schema_version)?;
            rows.push(row);
        }
        let has_more = rows.len() > query.page.limit as usize;
        if has_more {
            rows.pop();
        }
        let next_cursor = has_more.then(|| {
            rows.last()
                .expect("a non-empty overfull page has a retained row")
                .cursor()
        });
        Ok(Page::new(
            rows.into_iter().map(UserDoc::into_record).collect(),
            next_cursor,
        ))
    }
}

fn regex_literal(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        if matches!(
            character,
            '\\' | '.' | '^' | '$' | '|' | '?' | '*' | '+' | '(' | ')' | '[' | ']' | '{' | '}'
        ) {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

fn decode_cursor(raw: &str) -> Result<(DateTime, &str), StoreError> {
    let (millis, subject) = raw
        .split_once(':')
        .filter(|(_, subject)| !subject.is_empty())
        .ok_or_else(|| StoreError::Malformed("directory cursor is invalid".into()))?;
    let millis = millis
        .parse::<i64>()
        .map_err(|_| StoreError::Malformed("directory cursor is invalid".into()))?;
    Ok((DateTime::from_millis(millis), subject))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_shape_preserves_public_identity_and_cursor() {
        let row = UserDoc {
            subject: "subject-a".into(),
            document_schema_version: DOCUMENT_SCHEMA_VERSION,
            display_name: Some("Ada".into()),
            avatar_url: None,
            first_seen_at: "2026-01-01T00:00:00Z".into(),
            last_seen_at: "2026-01-02T00:00:00Z".into(),
            observed_at: DateTime::from_millis(42),
        };
        let cursor = row.cursor();
        assert_eq!(row.document_schema_version, DOCUMENT_SCHEMA_VERSION);
        assert_eq!(
            decode_cursor(&cursor).unwrap(),
            (row.observed_at, "subject-a")
        );
        let record = row.into_record();
        assert_eq!(record.identity.subject.as_str(), "subject-a");
        assert_eq!(record.identity.display_name.as_deref(), Some("Ada"));
    }

    #[test]
    fn text_search_is_literal() {
        assert_eq!(regex_literal("a.*[b]"), "a\\.\\*\\[b\\]");
    }
}
