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

/// How a subject is shown on a surface that shows people.
///
/// Display material and nothing else: absent fields mean the provider released
/// nothing worth drawing, and no decision is ever made on the strength of one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicIdentity {
    /// The name to show, when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// Absolute URL of the picture to show, when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,
}

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
    /// Absolute URL of the account's picture, when it has one.
    ///
    /// Kept beside the name because the surfaces that show one show the other,
    /// and a face is how somebody recognises their own work in a list far
    /// faster than reading a name off it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,
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
        avatar_url: Option<String>,
    ) -> Self {
        let now = now_rfc3339();
        Self {
            subject,
            display_name: display_name.filter(|s| !s.trim().is_empty()),
            email: email.filter(|s| !s.trim().is_empty()),
            avatar_url: avatar_url.filter(|s| !s.trim().is_empty()),
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

/// Most entries a single read of the directory may ask for.
pub const MAX_DIRECTORY_LIMIT: usize = 500;

/// Entries a read of the directory returns when it asks for no particular
/// number.
pub const DEFAULT_DIRECTORY_LIMIT: usize = 100;

/// Which entries a read of the directory wants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryQuery {
    /// Most entries to return, most recently seen first.
    pub limit: usize,
}

impl Default for DirectoryQuery {
    fn default() -> Self {
        Self {
            limit: DEFAULT_DIRECTORY_LIMIT,
        }
    }
}

impl DirectoryQuery {
    /// Bring the limit inside what a read may ask for.
    ///
    /// Clamped rather than refused, matching the record of actions: a caller
    /// asking for more than will be handed over wants as much as they can get,
    /// and a zero would otherwise ask the store for nothing at all.
    pub fn clamped(mut self) -> Self {
        self.limit = self.limit.clamp(1, MAX_DIRECTORY_LIMIT);
        self
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
        avatar_url: Option<String>,
    ) {
        let user = RegisteredUser::seen_now(subject.clone(), display_name, email, avatar_url);
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

    /// The most recently seen subjects, newest first, bounded by the query.
    ///
    /// Bounded because this only ever grows. A read with no ceiling is fine
    /// for as long as the service is young and is an outage afterwards, and
    /// the point at which it stops being fine is not announced.
    pub async fn list(
        &self,
        caller: &Caller,
        query: DirectoryQuery,
    ) -> Result<Vec<RegisteredUser>, CoreError> {
        caller
            .authorization
            .require(Capability::ReadUserDirectory)?;
        let query = query.clamped();
        let users = self.store.recent(&query).await.map_err(store_failed)?;
        tracing::info!(
            reader = %caller.subject,
            returned = users.len(),
            limit = query.limit,
            "the directory of subjects was read"
        );
        Ok(users)
    }

    /// How to name the given subjects, as far as this caller may know.
    ///
    /// For a surface that lists work by owner. Scoped to the subjects on the
    /// page rather than reading the whole directory to resolve a screenful of
    /// them: the page is bounded and the directory is not, and this runs on
    /// every read of that surface.
    ///
    /// Naming people is its own grant, and a caller without it gets no names
    /// rather than no page. The list of work is offered on the capability that
    /// opens the list of work; refusing it outright because a *second* grant is
    /// missing would leave a view drawn empty by entitlement, which teaches
    /// whoever is reading it that empty means "nothing here".
    ///
    /// Subjects the provider released no details for are simply absent, which
    /// leaves the surface to decide what to draw rather than handing it an
    /// identifier dressed up as a name.
    /// How the given subjects should be shown to this caller.
    ///
    /// Richer than a name because the surfaces that name somebody also draw
    /// them, and resolving the two separately would mean a row that has a face
    /// and no name or the reverse, depending on which call was refused.
    ///
    /// A caller may always be told who *they* are. Reading the directory is a
    /// grant about learning other people's names, and refusing somebody their
    /// own would leave them looking at their own work attributed to nobody.
    /// Every other subject requires that grant, and one not covered by it is
    /// simply absent rather than rendered as an identifier.
    pub async fn identities_known_to(
        &self,
        caller: &Caller,
        subjects: &[SubjectId],
    ) -> Result<HashMap<SubjectId, PublicIdentity>, CoreError> {
        let may_name_others = caller.authorization.allows(Capability::ReadUserDirectory);
        let wanted: Vec<SubjectId> = subjects
            .iter()
            .filter(|s| may_name_others || *s == &caller.subject)
            .cloned()
            .collect();
        if wanted.is_empty() {
            return Ok(HashMap::new());
        }
        let users = self
            .store
            .by_subjects(&wanted)
            .await
            .map_err(store_failed)?;
        Ok(users
            .into_iter()
            .map(|u| {
                (
                    u.subject.clone(),
                    PublicIdentity {
                        display_name: u.label().map(str::to_string),
                        avatar_url: u.avatar_url.clone(),
                    },
                )
            })
            .collect())
    }

    pub async fn names_known_to(
        &self,
        caller: &Caller,
        subjects: &[SubjectId],
    ) -> Result<HashMap<SubjectId, String>, CoreError> {
        if !caller.authorization.allows(Capability::ReadUserDirectory) || subjects.is_empty() {
            return Ok(HashMap::new());
        }
        let users = self
            .store
            .by_subjects(subjects)
            .await
            .map_err(store_failed)?;
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
