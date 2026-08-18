//! Reading sessions and their history.
//!
//! Split from `service` so both stay within the project's file-size limit, on
//! a division that is real rather than convenient: that module changes what a
//! session is, and this only answers questions about one. Every answer here is
//! scoped by who is asking, and the scoping lives here rather than in an
//! interface so that no interface can forget it.

use crate::authz::{Caller, Capability};
use crate::error::CoreError;
use crate::event::{Event, StoredEvent};
use crate::ids::SessionId;
use crate::service::{store_failed, SessionService};
use crate::session::Session;

impl SessionService {
    /// Read a session, scoped to its owner.
    pub async fn get_session(&self, caller: &Caller, id: &SessionId) -> Result<Session, CoreError> {
        let record = self.record(id).await?;
        if !caller.may_read_owned_by(&record.session.owner) {
            return Err(CoreError::Forbidden);
        }
        Ok(record.session)
    }

    /// List the subject's sessions, newest first.
    pub async fn list_sessions(&self, caller: &Caller) -> Vec<Session> {
        // A privileged reader sees every session; everyone else sees only their
        // own. The filter is here rather than in an interface so no interface
        // can forget it, and it is pushed into the query rather than applied
        // after so a large deployment does not read every record to show one.
        let result = if caller.authorization.allows(Capability::ReadAllSessions) {
            self.store.list_all().await
        } else {
            self.store.list_for(&caller.subject).await
        };
        match result {
            Ok(records) => records.into_iter().map(|r| r.session).collect(),
            Err(e) => {
                // Listing is a read: report nothing rather than pretend the
                // caller has no sessions, and make the failure visible.
                tracing::error!(error = %e, "cannot list sessions");
                Vec::new()
            }
        }
    }

    /// Every session there is, whoever owns it.
    ///
    /// Distinct from `list_sessions` in two ways that matter to whoever is
    /// watching the service rather than using it. It refuses a caller who is
    /// not entitled to see across subjects instead of quietly narrowing to
    /// their own work, and it reports a store it could not read instead of
    /// answering with an empty list — an operator told there are no sessions
    /// will act on that, and "the record is unreachable" is a different fact.
    pub async fn list_all_sessions(&self, caller: &Caller) -> Result<Vec<Session>, CoreError> {
        caller.authorization.require(Capability::ReadAllSessions)?;
        let records = self.store.list_all().await.map_err(store_failed)?;
        tracing::info!(
            reader = %caller.subject,
            returned = records.len(),
            "every session was listed"
        );
        Ok(records.into_iter().map(|r| r.session).collect())
    }

    /// Durable events for a session, optionally from a cursor.
    ///
    /// Somebody else's log takes the grant that exists to open it, which is not
    /// the one that lists their sessions. Both are privileged and an operator
    /// may well hold each, but they are issued separately and a monitoring role
    /// given only the narrower one must not arrive here.
    ///
    /// An event this build cannot name is skipped rather than projected. It was
    /// written by a newer control plane during a rolling deployment, and its
    /// sequence stays where it is in storage, so what a client sees is a gap —
    /// which is what they already tolerate, because they resume on `seq` rather
    /// than on how many events they have counted.
    pub async fn events_since(
        &self,
        caller: &Caller,
        id: &SessionId,
        after: Option<u64>,
    ) -> Result<Vec<Event>, CoreError> {
        let record = self.record(id).await?;
        if !caller.may_read_event_log_owned_by(&record.session.owner) {
            return Err(CoreError::Forbidden);
        }
        let stored = self
            .store
            .events_since(id, after.unwrap_or(0))
            .await
            .map_err(store_failed)?;
        Ok(stored
            .into_iter()
            .filter_map(StoredEvent::into_known)
            .collect())
    }

    /// Watch what is happening on a session, as it happens.
    ///
    /// The counterpart to `events_since` and deliberately not the same thing.
    /// That reads the record; this watches the moment. A turn takes as long as
    /// the agent takes, and between it starting and its message being
    /// committed the record says nothing at all — so a working agent and a
    /// dead one are drawn identically by anything that has only the log.
    ///
    /// **Nothing here is durable and nothing here has a sequence.** A frame is
    /// what is happening rather than what happened: numbering these would
    /// punch permanent holes in the sequence a resuming client cursors on, and
    /// replaying them would show somebody their conversation being typed a
    /// second time. A client that missed them finds the finished message
    /// waiting for it in the log, which is the whole reason the split exists.
    ///
    /// Scoped like every other read here, and additionally to the ability: a
    /// session with no conversation has no live frames to give, and refusing
    /// by name is what tells a caller that rather than handing them a receiver
    /// that stays silent forever.
    ///
    /// Frames are dropped rather than buffered when a receiver falls behind —
    /// `broadcast` says so with `Lagged`, and a caller that missed some has
    /// missed liveness, not history.
    pub async fn subscribe_live(
        &self,
        caller: &Caller,
        id: &SessionId,
    ) -> Result<tokio::sync::broadcast::Receiver<crate::event::LiveFrame>, CoreError> {
        let record = self.record(id).await?;
        if !caller.may_read_event_log_owned_by(&record.session.owner) {
            return Err(CoreError::Forbidden);
        }
        record
            .session
            .capabilities
            .require(crate::capabilities::Capability::Conversation)?;
        // Taken from the live entry, because that is where the runtime's
        // output arrives. A session with no entry is one nothing is running
        // for; there is no liveness to watch and saying so is better than a
        // receiver that never yields.
        self.entries
            .read()
            .await
            .get(id)
            .map(|entry| entry.live_frames.subscribe())
            .ok_or(CoreError::SessionNotFound)
    }
}
