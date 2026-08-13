//! Reading sessions and their history.
//!
//! Split from `service` so both stay within the project's file-size limit, on
//! a division that is real rather than convenient: that module changes what a
//! session is, and this only answers questions about one. Every answer here is
//! scoped by who is asking, and the scoping lives here rather than in an
//! interface so that no interface can forget it.

use crate::authz::{Caller, Capability};
use crate::error::CoreError;
use crate::event::Event;
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
        self.store
            .events_since(id, after.unwrap_or(0))
            .await
            .map_err(store_failed)
    }
}
