//! Who has actually used this service.
//!
//! The core owns no user model and authorises nothing against this. Identity is
//! asserted by whoever issues it, and a subject that has never been seen here
//! is still a perfectly valid subject — so this is a record of who has turned
//! up, kept because "who is using this" is a question the service alone can
//! answer and nobody else keeps the answer to.
//!
//! **A subject identifier is not how a person is shown.** It is an opaque
//! string, right for keying state and wrong in front of a human being, so the
//! account details released at sign-in are kept beside it and [`RegisteredUser::label`]
//! is what a surface renders. Falling back to the identifier is a last resort
//! for a provider that released nothing, not the normal case.

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::authz::{Caller, Capability};
use crate::clock::now_rfc3339;
use crate::error::CoreError;
use crate::ids::SubjectId;
use crate::store::{StoreError, UserDirectoryStore};

/// Somebody who has used the service.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegisteredUser {
    /// The authenticated principal. Opaque, and not how they are shown.
    pub subject: SubjectId,
    /// The account name the provider released, when it released one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// The account's email address, when the provider released it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// When this subject was first seen, RFC 3339 in UTC.
    pub first_seen: String,
    /// When this subject was last seen.
    pub last_seen: String,
}

impl RegisteredUser {
    /// A person seen now for the first time.
    pub fn seen_now(
        subject: SubjectId,
        display_name: Option<String>,
        email: Option<String>,
    ) -> Self {
        let now = now_rfc3339();
        Self {
            subject,
            display_name: display_name.filter(|s| !s.trim().is_empty()),
            email: email.filter(|s| !s.trim().is_empty()),
            first_seen: now.clone(),
            last_seen: now,
        }
    }

    /// How this person should be named to somebody reading a screen.
    ///
    /// The account name if there is one, otherwise the email address. `None`
    /// means the provider released nothing worth showing, and the surface has
    /// to decide what to draw — it is not an invitation to print the subject
    /// identifier as though it were a name.
    pub fn label(&self) -> Option<&str> {
        self.display_name
            .as_deref()
            .or(self.email.as_deref())
            .filter(|s| !s.trim().is_empty())
    }
}

/// The record of who has used the service.
pub struct UserDirectory {
    store: Arc<dyn UserDirectoryStore>,
}

impl UserDirectory {
    /// Build the directory over its port.
    pub fn new(store: Arc<dyn UserDirectoryStore>) -> Arc<Self> {
        Arc::new(Self { store })
    }

    /// Note that a subject has signed in.
    ///
    /// Returns nothing and takes no caller: this is the service recording its
    /// own history, and a person's sign-in must not fail because the note about
    /// it could not be written. A failure is reported at error level rather
    /// than swallowed, so a directory that has stopped being written is visible
    /// in the process output instead of only in a list that quietly stops
    /// growing.
    pub async fn note_sign_in(
        &self,
        subject: &SubjectId,
        display_name: Option<String>,
        email: Option<String>,
    ) {
        let user = RegisteredUser::seen_now(subject.clone(), display_name, email);
        match self.store.record_sign_in(&user).await {
            Ok(()) => tracing::info!(
                %subject,
                named = user.label().is_some(),
                "a subject was recorded as having used the service"
            ),
            // The name is not logged, only whether there was one. It is display
            // material about a person, and the process log is not where a
            // person's details belong.
            Err(e) => tracing::error!(%subject, error = %e, "cannot record a sign-in"),
        }
    }

    /// Everybody known, most recently seen first.
    pub async fn list(&self, caller: &Caller) -> Result<Vec<RegisteredUser>, CoreError> {
        caller
            .authorization
            .require(Capability::ReadUserDirectory)?;
        let users = self.store.list().await.map_err(store_failed)?;
        tracing::info!(
            reader = %caller.subject,
            returned = users.len(),
            "the directory of subjects was read"
        );
        Ok(users)
    }

    /// How to name each subject known, keyed by subject.
    ///
    /// For a surface that lists work by owner. A page of opaque identifiers is
    /// a page nobody can act on, and asking for one name at a time would be a
    /// query per row — so the whole mapping is handed over at once and joined
    /// by whoever is rendering.
    ///
    /// Subjects the provider released no details for are simply absent, which
    /// leaves the surface to decide what to draw rather than handing it an
    /// identifier dressed up as a name.
    pub async fn labels(&self, caller: &Caller) -> Result<HashMap<SubjectId, String>, CoreError> {
        caller
            .authorization
            .require(Capability::ReadUserDirectory)?;
        let users = self.store.list().await.map_err(store_failed)?;
        Ok(users
            .into_iter()
            .filter_map(|u| u.label().map(|l| (u.subject.clone(), l.to_string())))
            .collect())
    }
}

/// Render a storage failure as a core error.
///
/// Unreachable storage is reported as unavailability rather than as an empty
/// directory, so a reader is never told nobody has used the service when the
/// truth is that nobody could be asked.
fn store_failed(e: StoreError) -> CoreError {
    CoreError::RuntimeUnavailable(format!("user directory: {e}"))
}

#[cfg(test)]
#[path = "directory_tests.rs"]
mod tests;
