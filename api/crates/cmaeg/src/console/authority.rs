//! Owner authority, re-derived on every console request (Amendment R6
//! §R6.4).
//!
//! A session's `selected` owner is a selection, never a grant. Every
//! `/console*` request except sign-in, login, callback, signed-out and logout
//! checks it again before any read or write, with exactly REST's
//! `authenticate_token` and its per-token cache (60 s for a success, 10 s for
//! a failure). The session keeps no verdict of its own, so a demotion or a
//! deactivation stops the console within 60 s, as it stops REST. The access
//! token is refreshed only when it expires within 30 s, and anything NyxID
//! cannot answer fails closed with a 503 page while the session is kept.
use std::sync::Arc;

use super::{
    session::{EndReason, Flash, OwnerSelection, Refresh, SessionAuth, SessionCell, SessionStore},
    subject_hash_prefix,
};
use crate::{
    auth::{Creator, CreatorAuthError, CreatorAuthenticator},
    telemetry::names,
};

/// Proof that this request's session may act for `owner()` now. Only
/// [`ConsoleAuthority`] constructs it, so no console read or write can take a
/// bare owner string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Authorized {
    creator: Creator,
    scope: OwnerScope,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnerScope {
    Personal,
    Organization,
}

impl Authorized {
    pub(in crate::console) fn new(creator: Creator, scope: OwnerScope) -> Self {
        Self { creator, scope }
    }

    /// The verified owner: the subject for Personal, else the organization id.
    pub fn owner(&self) -> &str {
        &self.creator.subject
    }

    /// The creator every console mutation passes to the same channel
    /// functions as REST.
    pub fn creator(&self) -> &Creator {
        &self.creator
    }

    pub fn scope(&self) -> OwnerScope {
        self.scope
    }
}

/// Why a request was not authorized.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Denied {
    /// The session is gone, or NyxID just refused it and it ended.
    SignedOut,
    /// NyxID could not answer in time; the session and its selection are
    /// kept.
    Unavailable,
    /// The selected organization's admin role ended: the session now selects
    /// Personal with a flash. A GET is served with this Personal authority,
    /// so an organization record reads as 404; a POST makes no change.
    Downgraded(Authorized),
    /// A requested owner was refused; the selection is unchanged.
    Refused,
}

pub struct ConsoleAuthority {
    creator_auth: Arc<CreatorAuthenticator>,
    sessions: Arc<SessionStore>,
}

impl ConsoleAuthority {
    pub(in crate::console) fn new(
        creator_auth: Arc<CreatorAuthenticator>,
        sessions: Arc<SessionStore>,
    ) -> Self {
        Self {
            creator_auth,
            sessions,
        }
    }

    /// Derive `cell`'s authority for its selected owner. The caller cloned the
    /// cell's `Arc` and holds no map guard; this holds only the cell's async
    /// `auth` lock, so one session's checks are serialized (concurrent
    /// requests make one cache fill) while other sessions proceed.
    pub async fn authorize(&self, cell: &Arc<SessionCell>) -> Result<Authorized, Denied> {
        let mut auth = cell.lock_auth().await;
        let subject = self.live(cell)?;
        let refreshed = self.usable_token(cell, &mut auth, &subject).await?;
        let selected = cell.meta().selected.clone();
        match self.check(&auth, &selected).await {
            Ok(creator) => {
                let authorized = self.accept(cell, &subject, &selected, creator)?;
                record(if refreshed { "refreshed" } else { "ok" });
                Ok(authorized)
            }
            Err(CreatorAuthError::ProviderUnavailable) => Err(self.unavailable(&subject)),
            Err(CreatorAuthError::NotAuthenticated | CreatorAuthError::Forbidden) => match selected
            {
                OwnerSelection::Personal => Err(self.end(cell, &subject)),
                OwnerSelection::Organization(organization) => {
                    self.downgrade(cell, &auth, &subject, organization).await
                }
            },
        }
    }

    /// Verify `requested` exactly as REST verifies `X-CMAEG-Owner`, through
    /// the same cache, and only then store it as the selection. A refused
    /// organization leaves the selection unchanged with a notice.
    pub async fn select(
        &self,
        cell: &Arc<SessionCell>,
        requested: OwnerSelection,
    ) -> Result<Authorized, Denied> {
        let mut auth = cell.lock_auth().await;
        let subject = self.live(cell)?;
        self.usable_token(cell, &mut auth, &subject).await?;
        match self.check(&auth, &requested).await {
            Ok(creator) => {
                let authorized = self.accept(cell, &subject, &requested, creator)?;
                cell.meta().selected = requested;
                Ok(authorized)
            }
            Err(CreatorAuthError::ProviderUnavailable) => Err(self.unavailable(&subject)),
            Err(CreatorAuthError::NotAuthenticated | CreatorAuthError::Forbidden) => {
                match requested {
                    OwnerSelection::Personal => Err(self.end(cell, &subject)),
                    OwnerSelection::Organization(_) => {
                        cell.meta().flash = Some(Flash::OwnerRefused);
                        Err(Denied::Refused)
                    }
                }
            }
        }
    }

    /// The session's subject, unless it ended or expired while this request
    /// waited for the `auth` lock.
    fn live(&self, cell: &Arc<SessionCell>) -> Result<String, Denied> {
        if self.sessions.end_if_expired(cell) {
            return Err(Denied::SignedOut);
        }
        Ok(cell.meta().subject.clone())
    }

    /// Refresh the access token if it expires within 30 s; true when it was
    /// rotated. A definitive refusal has already ended the session.
    async fn usable_token(
        &self,
        cell: &Arc<SessionCell>,
        auth: &mut SessionAuth,
        subject: &str,
    ) -> Result<bool, Denied> {
        match self.sessions.refresh(cell, auth).await {
            Refresh::NotDue => Ok(false),
            Refresh::Rotated => Ok(true),
            Refresh::Transient => {
                record("refresh_transient");
                tracing::warn!(subject_hash_prefix = %subject_hash_prefix(subject), outcome = "refresh_transient", "console authorization");
                Err(Denied::Unavailable)
            }
            Refresh::Ended => {
                record("ended");
                tracing::info!(subject_hash_prefix = %subject_hash_prefix(subject), outcome = "ended", reason = "refresh", "console authorization");
                Err(Denied::SignedOut)
            }
        }
    }

    /// REST's rule and cache: `None` checks the bearer's own identity,
    /// `Some(org)` that the bearer is an admin of `org`.
    async fn check(
        &self,
        auth: &SessionAuth,
        owner: &OwnerSelection,
    ) -> Result<Creator, CreatorAuthError> {
        let organization = match owner {
            OwnerSelection::Personal => None,
            OwnerSelection::Organization(id) => Some(id.as_str()),
        };
        self.creator_auth
            .authenticate_token(auth.access().as_str(), organization)
            .await
    }

    /// A positive check for `owner`. Personal must still be the subject the
    /// session signed in as; anyone else ends it.
    fn accept(
        &self,
        cell: &Arc<SessionCell>,
        subject: &str,
        owner: &OwnerSelection,
        creator: Creator,
    ) -> Result<Authorized, Denied> {
        let scope = match owner {
            OwnerSelection::Personal if creator.subject != subject => {
                return Err(self.end(cell, subject))
            }
            OwnerSelection::Personal => OwnerScope::Personal,
            OwnerSelection::Organization(_) => OwnerScope::Organization,
        };
        Ok(Authorized::new(creator, scope))
    }

    /// NyxID refused the selected organization: the selection becomes
    /// Personal with a flash, and the Personal check decides the rest.
    async fn downgrade(
        &self,
        cell: &Arc<SessionCell>,
        auth: &SessionAuth,
        subject: &str,
        organization: String,
    ) -> Result<Authorized, Denied> {
        {
            let mut meta = cell.meta();
            meta.selected = OwnerSelection::Personal;
            meta.flash = Some(Flash::AdminEnded { organization });
        }
        record("downgraded");
        tracing::info!(subject_hash_prefix = %subject_hash_prefix(subject), outcome = "downgraded", "console authorization");
        match self.check(auth, &OwnerSelection::Personal).await {
            Ok(creator) => {
                let personal = self.accept(cell, subject, &OwnerSelection::Personal, creator)?;
                Err(Denied::Downgraded(personal))
            }
            Err(CreatorAuthError::NotAuthenticated | CreatorAuthError::Forbidden) => {
                Err(self.end(cell, subject))
            }
            Err(CreatorAuthError::ProviderUnavailable) => Err(self.unavailable(subject)),
        }
    }

    /// End the session NyxID refused, revoking its refresh token.
    fn end(&self, cell: &Arc<SessionCell>, subject: &str) -> Denied {
        self.sessions.end(cell, EndReason::Refused);
        record("ended");
        tracing::info!(subject_hash_prefix = %subject_hash_prefix(subject), outcome = "ended", reason = "identity", "console authorization");
        Denied::SignedOut
    }

    fn unavailable(&self, subject: &str) -> Denied {
        record("unavailable");
        tracing::warn!(subject_hash_prefix = %subject_hash_prefix(subject), outcome = "unavailable", "console authorization");
        Denied::Unavailable
    }
}

fn record(outcome: &'static str) {
    metrics::counter!(names::CONSOLE_AUTHORIZATIONS, names::OUTCOME => outcome).increment(1);
}
