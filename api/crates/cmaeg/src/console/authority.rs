//! Owner authority, re-derived on every console request (Amendment R3).
//!
//! A session's `selected` owner is a selection, never a grant. Every
//! `/console*` request except sign-in, login, callback, signed-out and logout
//! verifies it again through NyxID before any read or write: reads may reuse
//! a session's verdict for 60 s (10 s when NyxID was unavailable), and every
//! mutation verifies afresh. Checks hold one of 8 permits, at most one per
//! subject, and wait at most 2 s for them; anything NyxID cannot answer fails
//! closed with a 503 page and keeps the session.
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use dashmap::DashMap;
use tokio::sync::Semaphore;

use super::{
    clock::Clock,
    session::{EndReason, Flash, OwnerSelection, Refresh, SessionAuth, SessionCell, SessionStore},
    subject_hash_prefix,
};
use crate::{
    auth::{Creator, CreatorAuthError, CreatorAuthenticator},
    telemetry::names,
};

const OK_TTL: Duration = Duration::from_secs(60);
const UNAVAILABLE_TTL: Duration = Duration::from_secs(10);
const CHECK_PERMITS: usize = 8;
const PERMIT_WAIT: Duration = Duration::from_secs(2);

/// Proof that this request's session may act for `owner()` now. Only
/// [`ConsoleAuthority`] constructs it, so no console read or write can take a
/// bare owner string.
#[derive(Debug, Clone)]
pub struct Authorized {
    creator: Creator,
    scope: OwnerScope,
    verified_at: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnerScope {
    Personal,
    Organization,
}

impl Authorized {
    pub(in crate::console) fn new(
        creator: Creator,
        scope: OwnerScope,
        verified_at: Instant,
    ) -> Self {
        Self {
            creator,
            scope,
            verified_at,
        }
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

    pub fn verified_at(&self) -> Instant {
        self.verified_at
    }
}

/// What a request needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Need {
    /// A read; an `Ok` verdict for the selected owner may be reused for 60 s.
    Read,
    /// A mutation of the selected owner's data: always verified afresh.
    Fresh,
    /// Switch to this owner: verified afresh before it is stored.
    Select(OwnerSelection),
}

/// Why a request was not authorized.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Denied {
    /// The session is gone, or NyxID just refused it and it ended.
    SignedOut,
    /// NyxID could not answer in time; the session is kept.
    Unavailable,
    /// The selected organization's admin role ended: the session now selects
    /// Personal with a flash, and a mutation makes no change.
    Downgraded,
    /// A requested owner was refused; the selection is unchanged.
    Refused,
}

/// A session's last answer, reused by reads.
pub(crate) struct Verdict {
    owner: OwnerSelection,
    result: Option<Creator>,
    at: Instant,
}

enum Check {
    Ok(Creator),
    Refused,
    Unavailable,
}

pub struct ConsoleAuthority {
    creator_auth: Arc<CreatorAuthenticator>,
    sessions: Arc<SessionStore>,
    clock: Arc<dyn Clock>,
    permits: Arc<Semaphore>,
    subjects: DashMap<String, Arc<Semaphore>>,
}

impl ConsoleAuthority {
    pub fn new(
        creator_auth: Arc<CreatorAuthenticator>,
        sessions: Arc<SessionStore>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            creator_auth,
            sessions,
            clock,
            permits: Arc::new(Semaphore::new(CHECK_PERMITS)),
            subjects: DashMap::new(),
        }
    }

    /// Derive `cell`'s authority for `need`. The caller cloned the cell's
    /// `Arc` and holds no map guard; this holds only the cell's async `auth`
    /// lock, so one session's checks are serialized and others proceed.
    pub async fn authorize(
        &self,
        cell: &Arc<SessionCell>,
        need: Need,
    ) -> Result<Authorized, Denied> {
        let mut auth = cell.lock_auth().await;
        if cell.is_ended() {
            return Err(Denied::SignedOut);
        }
        let (subject, stored) = {
            let meta = cell.meta();
            (meta.subject.clone(), meta.selected.clone())
        };
        let selected = match &need {
            Need::Select(requested) => requested.clone(),
            _ => stored,
        };
        let mut refreshed = false;
        let mut transient = false;
        match self.sessions.refresh(cell, &mut auth, false).await {
            Refresh::NotDue => {}
            Refresh::Rotated => refreshed = true,
            Refresh::Transient { usable: true } => transient = true,
            Refresh::Transient { usable: false } => {
                return Err(self.unavailable(&mut auth, &selected, &subject))
            }
            Refresh::Ended => return Err(self.ended(&subject)),
        }
        let now = self.clock.now();
        if need == Need::Read {
            if let Some(verdict) = auth
                .verdict
                .as_ref()
                .filter(|verdict| verdict.owner == selected)
            {
                match &verdict.result {
                    Some(creator) if now.duration_since(verdict.at) < OK_TTL => {
                        record("ok");
                        return Ok(authorized(creator.clone(), &selected, verdict.at));
                    }
                    None if now.duration_since(verdict.at) < UNAVAILABLE_TTL => {
                        record("unavailable");
                        return Err(Denied::Unavailable);
                    }
                    _ => {}
                }
            }
        }
        let mut check = self.check(&subject, &auth, &selected).await;
        if matches!(check, Check::Refused) && !refreshed {
            // A stale token is never reported as a demotion: refresh once
            // and ask again before believing NyxID's refusal.
            match self.sessions.refresh(cell, &mut auth, true).await {
                Refresh::Rotated => {
                    refreshed = true;
                    check = self.check(&subject, &auth, &selected).await;
                }
                Refresh::Ended => return Err(self.ended(&subject)),
                Refresh::NotDue | Refresh::Transient { .. } => {
                    return Err(self.unavailable(&mut auth, &selected, &subject));
                }
            }
        }
        match check {
            Check::Ok(creator) => {
                if selected == OwnerSelection::Personal && creator.subject != subject {
                    self.sessions.end(cell, EndReason::Refused);
                    return Err(self.ended(&subject));
                }
                record(if refreshed {
                    "refreshed"
                } else if transient {
                    "refresh_transient"
                } else {
                    "ok"
                });
                Ok(self.remember(&mut auth, creator, &selected))
            }
            Check::Unavailable => Err(self.unavailable(&mut auth, &selected, &subject)),
            Check::Refused => {
                self.refused(cell, &mut auth, &subject, &selected, &need)
                    .await
            }
        }
    }

    /// NyxID refused `selected` for a fresh token.
    async fn refused(
        &self,
        cell: &Arc<SessionCell>,
        auth: &mut SessionAuth,
        subject: &str,
        selected: &OwnerSelection,
        need: &Need,
    ) -> Result<Authorized, Denied> {
        let OwnerSelection::Organization(organization) = selected else {
            self.sessions.end(cell, EndReason::Refused);
            return Err(self.ended(subject));
        };
        match self.check(subject, auth, &OwnerSelection::Personal).await {
            Check::Ok(creator) if creator.subject == subject => {
                if matches!(need, Need::Select(_)) {
                    // The fresh refusal replaces any verdict for that owner,
                    // so the next read checks again instead of reusing it.
                    if auth
                        .verdict
                        .as_ref()
                        .is_some_and(|verdict| verdict.owner == *selected)
                    {
                        auth.verdict = None;
                    }
                    cell.meta().flash = Some(Flash::OwnerRefused);
                    tracing::info!(subject_hash_prefix = %subject_hash_prefix(subject), outcome = "refused", owner_kind = "organization", "console owner selection");
                    return Err(Denied::Refused);
                }
                // The user really lost the organization's admin role.
                {
                    let mut meta = cell.meta();
                    meta.selected = OwnerSelection::Personal;
                    meta.flash = Some(Flash::AdminEnded {
                        organization: organization.clone(),
                    });
                }
                record("downgraded");
                tracing::info!(subject_hash_prefix = %subject_hash_prefix(subject), outcome = "downgraded", "console authorization");
                let personal = self.remember(auth, creator, &OwnerSelection::Personal);
                match need {
                    Need::Read => Ok(personal),
                    Need::Fresh | Need::Select(_) => Err(Denied::Downgraded),
                }
            }
            Check::Unavailable => Err(self.unavailable(auth, selected, subject)),
            Check::Ok(_) | Check::Refused => {
                self.sessions.end(cell, EndReason::Refused);
                Err(self.ended(subject))
            }
        }
    }

    /// One NyxID check under the permits: at most 8 in flight and one per
    /// subject, waiting at most 2 s for both.
    async fn check(&self, subject: &str, auth: &SessionAuth, owner: &OwnerSelection) -> Check {
        // The map guard is a temporary of this statement.
        let gate = self
            .subjects
            .entry(subject.to_owned())
            .or_insert_with(|| Arc::new(Semaphore::new(1)))
            .clone();
        let permits = tokio::time::timeout(PERMIT_WAIT, async {
            let own = gate.clone().acquire_owned().await;
            let shared = self.permits.clone().acquire_owned().await;
            (own, shared)
        })
        .await;
        let check = match permits {
            Ok((Ok(_own), Ok(_shared))) => {
                let organization = match owner {
                    OwnerSelection::Personal => None,
                    OwnerSelection::Organization(id) => Some(id.as_str()),
                };
                match self
                    .creator_auth
                    .verify_uncached(auth.access().as_str(), organization)
                    .await
                {
                    Ok(creator) => Check::Ok(creator),
                    Err(CreatorAuthError::NotAuthenticated) => Check::Refused,
                    Err(CreatorAuthError::ProviderUnavailable) => Check::Unavailable,
                }
            }
            _ => Check::Unavailable,
        };
        drop(gate);
        // Drop the subject's gate once nobody holds or awaits it.
        self.subjects
            .remove_if(subject, |_, gate| Arc::strong_count(gate) == 1);
        check
    }

    fn remember(
        &self,
        auth: &mut SessionAuth,
        creator: Creator,
        owner: &OwnerSelection,
    ) -> Authorized {
        let at = self.clock.now();
        auth.verdict = Some(Verdict {
            owner: owner.clone(),
            result: Some(creator.clone()),
            at,
        });
        authorized(creator, owner, at)
    }

    fn unavailable(&self, auth: &mut SessionAuth, owner: &OwnerSelection, subject: &str) -> Denied {
        auth.verdict = Some(Verdict {
            owner: owner.clone(),
            result: None,
            at: self.clock.now(),
        });
        record("unavailable");
        tracing::warn!(subject_hash_prefix = %subject_hash_prefix(subject), outcome = "unavailable", "console authorization");
        Denied::Unavailable
    }

    fn ended(&self, subject: &str) -> Denied {
        record("ended");
        tracing::info!(subject_hash_prefix = %subject_hash_prefix(subject), outcome = "ended", "console authorization");
        Denied::SignedOut
    }
}

fn authorized(creator: Creator, owner: &OwnerSelection, at: Instant) -> Authorized {
    let scope = match owner {
        OwnerSelection::Personal => OwnerScope::Personal,
        OwnerSelection::Organization(_) => OwnerScope::Organization,
    };
    Authorized::new(creator, scope, at)
}

fn record(outcome: &'static str) {
    metrics::counter!(names::CONSOLE_AUTHORIZATIONS, names::OUTCOME => outcome).increment(1);
}
