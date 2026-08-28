//! One-time grants for attaching a terminal from a browser.
//!
//! Every other route on this service authenticates the same way: the caller
//! presents a credential in a header and middleware verifies it. A browser
//! cannot do that for a WebSocket — the API has no way to set a header on the
//! upgrade — so a product frontend holding a bearer token, which is what
//! CMA's actual consumers are, could not attach the one surface that
//! offers full terminal parity. Cross-origin cookies are not a workable answer
//! for clients on somebody else's origin.
//!
//! So the credential is exchanged, over a route that *can* carry a header, for
//! a ticket the upgrade can carry in its URL. A ticket in a URL is tolerable
//! only because of what this file makes true of it: it is single-use, it lives
//! about thirty seconds, it is bound to one session and one subject, and it
//! grants nothing but the attachment it was issued for. The long-lived
//! credential never appears in a URL, a proxy log, or a browser history entry.
//!
//! **A ticket value is never recorded.** The durable store receives only its
//! SHA-256 identifier, which is enough to find and atomically consume a record
//! but cannot be presented on an upgrade. The activity middleware likewise
//! keeps only what its classifier names, and `ticket` is not a name it knows — see
//! `cma/src/activity/classify.rs`, whose query allowlist has three keys
//! and this is not one of them. Nothing here logs a ticket's value either. The
//! temptation will be to log one while debugging an attach that will not work;
//! log the session and the subject instead, which is what identifies the
//! attempt anyway.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use layer2_core::store::StoreError;
use layer2_core::{AttachmentClaim, Authorization, Caller, SessionId, SubjectId};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

/// How long a ticket remains redeemable.
///
/// Long enough for a browser to open a socket after the response arrives —
/// including a slow network and a page that does something else first — and
/// short enough that the exposure a URL creates is measured in seconds. It is
/// not a session lifetime and must never become one.
const LIFETIME: Duration = Duration::from_secs(30);

/// A ticket, as it is handed to the client that asked for one.
#[derive(Debug, Clone)]
pub struct IssuedTicket {
    /// The opaque value to present on the upgrade.
    pub value: String,
    /// When it stops being redeemable, RFC 3339 in UTC.
    pub expires_at: String,
}

/// What redeeming a ticket establishes.
pub struct RedeemedTicket {
    /// Who the ticket was issued to, with the grants they held at the time.
    pub caller: Caller,
    /// Whether this attachment supersedes whoever currently holds one.
    pub claim: AttachmentClaim,
}

/// Stable lookup key for a ticket, derived without retaining its bearer value.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TicketId([u8; 32]);

impl TicketId {
    /// Rebuild an identifier read from durable storage.
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Bytes used as the durable record key.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

/// A terminal ticket as the persistence port holds it.
#[derive(Clone)]
pub struct StoredTicket {
    /// One-way identifier of the opaque value returned to the caller.
    pub id: TicketId,
    /// The only session this ticket may open.
    pub session: SessionId,
    /// The subject that exchanged their credential for this ticket.
    pub subject: SubjectId,
    /// Capabilities captured when the ticket was issued.
    pub authorization: Authorization,
    /// Whether redemption may supersede an existing attachment.
    pub claim: AttachmentClaim,
    /// Absolute application expiry. TTL cleanup is only storage hygiene.
    pub expires_at: SystemTime,
}

/// Durable operations required by terminal tickets.
#[async_trait::async_trait]
pub trait TerminalTicketStore: Send + Sync {
    /// Record one freshly issued ticket.
    async fn put(&self, ticket: StoredTicket) -> Result<(), StoreError>;

    /// Atomically remove and return one ticket, if present.
    async fn take(&self, id: &TicketId) -> Result<Option<StoredTicket>, StoreError>;
}

/// Process-local ticket storage for focused tests and embedded callers.
///
/// Production wiring supplies MongoDB. Keeping this implementation explicit,
/// rather than building it into [`TerminalTickets`], makes choosing it visible.
#[derive(Default)]
pub struct MemoryTerminalTicketStore {
    issued: Mutex<std::collections::HashMap<TicketId, StoredTicket>>,
}

#[async_trait::async_trait]
impl TerminalTicketStore for MemoryTerminalTicketStore {
    async fn put(&self, ticket: StoredTicket) -> Result<(), StoreError> {
        let now = SystemTime::now();
        let mut issued = self.issued.lock().await;
        issued.retain(|_, held| held.expires_at > now);
        issued.insert(ticket.id.clone(), ticket);
        Ok(())
    }

    async fn take(&self, id: &TicketId) -> Result<Option<StoredTicket>, StoreError> {
        Ok(self.issued.lock().await.remove(id))
    }
}

/// One-time terminal grants backed by an explicitly supplied store.
pub struct TerminalTickets {
    store: Arc<dyn TerminalTicketStore>,
    lifetime: Duration,
}

impl TerminalTickets {
    /// Build ticket custody over a durable store.
    pub fn new(store: Arc<dyn TerminalTicketStore>) -> Arc<Self> {
        Arc::new(Self {
            store,
            lifetime: LIFETIME,
        })
    }

    /// Build process-local ticket custody for tests and embedded use.
    pub fn in_memory() -> Arc<Self> {
        Self::new(Arc::new(MemoryTerminalTicketStore::default()))
    }

    #[cfg(test)]
    fn with_lifetime(store: Arc<dyn TerminalTicketStore>, lifetime: Duration) -> Arc<Self> {
        Arc::new(Self { store, lifetime })
    }

    /// Mint a ticket for this caller to attach this session.
    ///
    /// The caller's grants are captured as they stand. A ticket is a snapshot
    /// of what its holder could do when they asked, valid for as long as the
    /// ticket is — which is the honest reading of a credential exchanged for a
    /// shorter-lived one.
    pub async fn issue(
        &self,
        caller: &Caller,
        session: &SessionId,
        claim: AttachmentClaim,
    ) -> Result<IssuedTicket, StoreError> {
        let value = mint();
        let expires_at = SystemTime::now() + self.lifetime;
        self.store
            .put(StoredTicket {
                id: identify(&value),
                session: session.clone(),
                subject: caller.subject.clone(),
                authorization: caller.authorization.clone(),
                claim,
                expires_at,
            })
            .await?;
        tracing::info!(
            session_id = %session,
            subject = %caller.subject,
            takeover = matches!(claim, AttachmentClaim::Takeover),
            "issued a terminal attach ticket"
        );
        Ok(IssuedTicket {
            value,
            expires_at: layer2_core::rfc3339_at(expires_at),
        })
    }

    /// Redeem a ticket for the session it was issued against, consuming it.
    ///
    /// Returns `None` for anything that is not a live ticket for this session:
    /// unknown, already used, expired, or issued for something else. The four
    /// are deliberately indistinguishable to the caller — telling them apart
    /// would say whether a value they guessed had ever existed.
    pub async fn redeem(
        &self,
        value: &str,
        session: &SessionId,
    ) -> Result<Option<RedeemedTicket>, StoreError> {
        // Removed before it is checked, so a redemption that is refused for
        // any reason still burns the ticket. Leaving a rejected one in place
        // would make it retryable, which is the one property single use is
        // supposed to remove.
        let Some(ticket) = self.store.take(&identify(value)).await? else {
            return Ok(None);
        };
        if ticket.expires_at <= SystemTime::now() {
            tracing::info!(session_id = %session, "an expired attach ticket was presented");
            return Ok(None);
        }
        if &ticket.session != session {
            // A ticket for one session presented against another. Worth
            // saying, because it is either a client bug or somebody trying.
            tracing::warn!(
                session_id = %session,
                issued_for = %ticket.session,
                "an attach ticket was presented against a session it was not issued for"
            );
            return Ok(None);
        }
        Ok(Some(RedeemedTicket {
            caller: Caller::new(ticket.subject, ticket.authorization),
            claim: ticket.claim,
        }))
    }
}

fn identify(value: &str) -> TicketId {
    TicketId(Sha256::digest(value.as_bytes()).into())
}

/// A value nobody can guess.
///
/// Two version-4 identifiers rather than one: they are drawn from the
/// operating system's cryptographic generator, and doubling them puts the
/// value far enough beyond guessing that its thirty-second life is not the
/// only thing protecting it.
fn mint() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

#[cfg(test)]
#[path = "tickets_tests.rs"]
mod tickets_tests;
