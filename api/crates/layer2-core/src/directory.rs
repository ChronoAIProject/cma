//! Read-only identity directory backed by an explicit store port.

use std::{collections::BTreeMap, sync::Arc};

use serde::{Deserialize, Serialize};

use crate::{Caller, Capability, CoreError, Page, PageRequest, StoreError, SubjectId};

/// Public identity fields safe to return from CMA.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicIdentity {
    pub subject: SubjectId,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
}

/// Directory record with its CMA observation timestamp.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisteredUser {
    pub identity: PublicIdentity,
    pub first_seen_at: String,
    pub last_seen_at: String,
}

/// Bounded, opaque directory query.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectoryQuery {
    #[serde(flatten)]
    pub page: PageRequest,
    pub text: Option<String>,
}

#[async_trait::async_trait]
pub trait UserDirectoryStore: Send + Sync {
    async fn observe(&self, identity: &PublicIdentity) -> Result<(), StoreError>;
    async fn user(&self, subject: &SubjectId) -> Result<Option<RegisteredUser>, StoreError>;
    async fn list(&self, query: &DirectoryQuery) -> Result<Page<RegisteredUser>, StoreError>;

    /// Resolve public names for identities on an already-authorized resource page.
    async fn display_names(
        &self,
        subjects: &[SubjectId],
    ) -> Result<BTreeMap<String, String>, StoreError> {
        let mut names = BTreeMap::new();
        for subject in subjects {
            if let Some(user) = self.user(subject).await? {
                if user.identity.subject != *subject {
                    return Err(StoreError::Malformed("directory identity mismatch".into()));
                }
                if let Some(name) = user.identity.display_name {
                    names.insert(subject.0.clone(), name);
                }
            }
        }
        Ok(names)
    }
}

pub struct UserDirectory {
    store: Arc<dyn UserDirectoryStore>,
}

impl UserDirectory {
    pub fn new(store: Arc<dyn UserDirectoryStore>) -> Arc<Self> {
        Arc::new(Self { store })
    }

    pub async fn observe(&self, identity: &PublicIdentity) -> Result<(), StoreError> {
        self.store.observe(identity).await
    }

    pub async fn user(&self, subject: &SubjectId) -> Result<Option<RegisteredUser>, StoreError> {
        self.store.user(subject).await
    }

    /// Call only after authorizing access to the records bearing these subjects.
    /// This resolves names, without granting access to the directory itself.
    pub async fn display_names(
        &self,
        subjects: &[SubjectId],
    ) -> Result<BTreeMap<String, String>, StoreError> {
        if subjects.len() > 200 {
            return Err(StoreError::Malformed(
                "identity lookup exceeds page bound".into(),
            ));
        }
        if subjects.is_empty() {
            return Ok(BTreeMap::new());
        }
        let mut names = self.store.display_names(subjects).await?;
        if names
            .keys()
            .any(|key| !subjects.iter().any(|subject| &subject.0 == key))
        {
            return Err(StoreError::Malformed("directory identity mismatch".into()));
        }
        names.retain(|subject, name| !name.trim().is_empty() && name.trim() != subject);
        Ok(names)
    }

    /// Called only after the resource policy has authorized sharing management.
    /// Exposes only public identity fields, with a non-empty search and hard cap.
    pub(crate) async fn share_recipients(
        &self,
        text: &str,
    ) -> Result<Vec<PublicIdentity>, StoreError> {
        if text.trim().len() < 2 || text.len() > 128 {
            return Ok(Vec::new());
        }
        let query = DirectoryQuery {
            page: PageRequest {
                limit: 20,
                cursor: None,
            },
            text: Some(text.trim().to_owned()),
        };
        let result = self.store.list(&query).await?;
        Ok(result
            .items
            .into_iter()
            .take(20)
            .map(|u| u.identity)
            .collect())
    }

    pub async fn list(
        &self,
        caller: &Caller,
        query: &DirectoryQuery,
    ) -> Result<Page<RegisteredUser>, CoreError> {
        caller.require(Capability::ReadUserDirectory)?;
        query.validate()?;
        self.store
            .list(query)
            .await
            .map_err(|error| CoreError::RuntimeUnavailable(error.to_string()))
    }
}

impl DirectoryQuery {
    /// Keep search allocation and store work bounded before reaching an adapter.
    pub fn validate(&self) -> Result<(), CoreError> {
        PageRequest::new(self.page.limit, self.page.cursor.clone())?;
        if self
            .text
            .as_ref()
            .is_some_and(|text| text.trim().is_empty() || text.len() > 128)
        {
            return Err(CoreError::InvalidRequest(
                "directory text must contain between 1 and 128 bytes".into(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directory_queries_are_bounded() {
        assert!(DirectoryQuery::default().validate().is_ok());
        assert!(DirectoryQuery {
            page: PageRequest {
                limit: 0,
                cursor: None
            },
            text: None,
        }
        .validate()
        .is_err());
        assert!(DirectoryQuery {
            page: PageRequest::default(),
            text: Some(" ".into()),
        }
        .validate()
        .is_err());
    }
}
