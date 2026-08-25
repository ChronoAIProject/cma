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
use crate::store::{Page, SessionListQuery, SessionRecord};

impl SessionService {
    /// Read a session, scoped to its owner.
    pub async fn get_session(&self, caller: &Caller, id: &SessionId) -> Result<Session, CoreError> {
        let record = self.record(id).await?;
        if !caller.may_read_owned_by(&record.session.owner) {
            return Err(CoreError::Forbidden);
        }
        Ok(record.session)
    }

    /// List the sessions visible to the caller, newest first.
    pub async fn list_sessions(
        &self,
        caller: &Caller,
        query: &SessionListQuery,
    ) -> Result<Page<Session>, CoreError> {
        // A privileged reader sees every session; everyone else sees only their
        // own. The filter is here rather than in an interface so no interface
        // can forget it, and it is pushed into the query rather than applied
        // after so a large deployment does not read every record to show one.
        let result = if caller.authorization.allows(Capability::ReadAllSessions) {
            self.store.list_all(query).await
        } else {
            self.store.list_for(&caller.subject, query).await
        };
        let records = result.map_err(|e| {
            tracing::error!(reader = %caller.subject, error = %e, "cannot list sessions");
            store_failed(e)
        })?;
        Ok(records.map(|record| record.session))
    }

    /// The caller's current live-session allowance.
    ///
    /// Advisory: asking again on create or revive is the authority. This read
    /// exists so a client can avoid offering an action that is currently known
    /// to fail. It is always scoped to the caller's own subject; an operator's
    /// cross-subject read grant does not turn this into somebody else's quota.
    pub async fn session_allowance(
        &self,
        caller: &Caller,
    ) -> Result<crate::SessionAllowance, CoreError> {
        let settings = self.settings.for_provisioning().await;
        let live = self
            .store
            .count_live_for(&caller.subject)
            .await
            .map_err(store_failed)?;
        Ok(crate::SessionAllowance {
            limit: settings.live_sessions.map(|limit| limit.get()),
            live,
            exempt: caller.authorization.allows(Capability::ExceedSessionLimit),
        })
    }

    /// Load the record behind an event-log read, enforcing that log's grant.
    async fn readable_event_log_record(
        &self,
        caller: &Caller,
        id: &SessionId,
    ) -> Result<SessionRecord, CoreError> {
        let record = self.record(id).await?;
        if !caller.may_read_event_log_owned_by(&record.session.owner) {
            return Err(CoreError::Forbidden);
        }
        Ok(record)
    }

    /// Every session there is, whoever owns it.
    ///
    /// Distinct from `list_sessions` because it refuses a caller who is not
    /// entitled to see across subjects instead of quietly narrowing to their
    /// own work.
    pub async fn list_all_sessions(
        &self,
        caller: &Caller,
        query: &SessionListQuery,
    ) -> Result<Page<Session>, CoreError> {
        caller.authorization.require(Capability::ReadAllSessions)?;
        let records = self.store.list_all(query).await.map_err(store_failed)?;
        tracing::info!(
            reader = %caller.subject,
            returned = records.items.len(),
            "every session was listed"
        );
        Ok(records.map(|record| record.session))
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
        self.readable_event_log_record(caller, id).await?;
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

    /// Highest durable sequence the event log for this session has reached.
    ///
    /// Scoped exactly like [`Self::events_since`]. In particular, a caller
    /// granted cross-owner event-log reads does not also need the separate
    /// cross-owner session-read grant merely to validate a replay position.
    ///
    /// The physical tail can lead a whole-record write, while a position an
    /// older writer advertised remains reserved even when its event was lost.
    /// The greater value is therefore the only head that rejects neither kind
    /// of valid cursor.
    pub async fn event_log_head(&self, caller: &Caller, id: &SessionId) -> Result<u64, CoreError> {
        let advertised = self
            .readable_event_log_record(caller, id)
            .await?
            .session
            .last_seq;
        let physical = self.store.event_tail(id).await.map_err(store_failed)?;
        Ok(advertised.max(physical))
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

    /// Watch what a session is using, as it uses it.
    ///
    /// A sibling of `subscribe_live` rather than a widening of it: live frames
    /// remain conversation-only, and readings have their own lag budget.
    ///
    /// Scoped to seeing the session rather than opening its event log. A
    /// reading contains current resource figures, not the session's ordered
    /// history. The named ability is checked before any receiver is handed out.
    pub async fn subscribe_resources(
        &self,
        caller: &Caller,
        id: &SessionId,
    ) -> Result<tokio::sync::broadcast::Receiver<crate::resources::ResourceReading>, CoreError>
    {
        let record = self.record(id).await?;
        if !caller.may_read_owned_by(&record.session.owner) {
            return Err(CoreError::Forbidden);
        }
        record
            .session
            .capabilities
            .require(crate::capabilities::Capability::Resources)?;
        self.entries
            .read()
            .await
            .get(id)
            .map(|entry| entry.resources.subscribe())
            .ok_or(CoreError::SessionNotFound)
    }

    /// What is known about a session's resource use right now.
    ///
    /// Scoped exactly like `subscribe_resources`: seeing the session and
    /// holding the named ability are both required. A session with no live
    /// entry answers `Awaiting`; a point read can report that nothing is held
    /// even though there is no channel to subscribe to.
    pub async fn last_reading(
        &self,
        caller: &Caller,
        id: &SessionId,
    ) -> Result<crate::resources::ResourceStatus, CoreError> {
        let record = self.record(id).await?;
        if !caller.may_read_owned_by(&record.session.owner) {
            return Err(CoreError::Forbidden);
        }
        record
            .session
            .capabilities
            .require(crate::capabilities::Capability::Resources)?;

        let entries = self.entries.read().await;
        Ok(match entries.get(id) {
            Some(entry) => match &entry.last_reading {
                Some(held) => crate::resources::ResourceStatus::Held(held.clone()),
                None if entry.readings_unreported => crate::resources::ResourceStatus::Unreported,
                None => crate::resources::ResourceStatus::Awaiting,
            },
            None => crate::resources::ResourceStatus::Awaiting,
        })
    }
}
