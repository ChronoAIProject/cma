//! One-time grants for attaching a terminal from a browser.
//!
//! Every other route on this service authenticates the same way: the caller
//! presents a credential in a header and middleware verifies it. A browser
//! cannot do that for a WebSocket — the API has no way to set a header on the
//! upgrade — so a product frontend holding a bearer token, which is what
//! chrono-bot's actual consumers are, could not attach the one surface that
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
//! **A ticket is never recorded.** The activity middleware keeps only what its
//! classifier names, and `ticket` is not a name it knows — see
//! `chrono-bot/src/activity/classify.rs`, whose query allowlist has three keys
//! and this is not one of them. Nothing here logs a ticket's value either. The
//! temptation will be to log one while debugging an attach that will not work;
//! log the session and the subject instead, which is what identifies the
//! attempt anyway.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use layer2_core::{AttachmentClaim, Authorization, Caller, SessionId, SubjectId};
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

/// What was issued, held until it is redeemed or expires.
struct Ticket {
    session: SessionId,
    subject: SubjectId,
    authorization: Authorization,
    claim: AttachmentClaim,
    expires: Instant,
}

/// Tickets this process has issued and not yet seen back.
///
/// In memory on purpose. A ticket outlives neither the thirty seconds it is
/// given nor the process that minted it, and a restart in that window costs a
/// client one extra round trip on a flow that already has to handle a refused
/// redemption. Persisting it would mean storing an authorization decision in a
/// database for the sake of a case that resolves itself.
#[derive(Default)]
pub struct TerminalTickets {
    issued: Mutex<HashMap<String, Ticket>>,
}

impl TerminalTickets {
    /// A fresh, empty store.
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
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
    ) -> IssuedTicket {
        let value = mint();
        let expires = Instant::now() + LIFETIME;
        let mut issued = self.issued.lock().await;
        // Swept here rather than on a timer: this is the only moment the map
        // is known to be growing, and an expired ticket left behind is not
        // dangerous — it is refused on redemption — only untidy.
        issued.retain(|_, t| t.expires > Instant::now());
        issued.insert(
            value.clone(),
            Ticket {
                session: session.clone(),
                subject: caller.subject.clone(),
                authorization: caller.authorization.clone(),
                claim,
                expires,
            },
        );
        tracing::info!(
            session_id = %session,
            subject = %caller.subject,
            takeover = matches!(claim, AttachmentClaim::Takeover),
            "issued a terminal attach ticket"
        );
        IssuedTicket {
            value,
            expires_at: layer2_core::rfc3339_at(SystemTime::now() + LIFETIME),
        }
    }

    /// Redeem a ticket for the session it was issued against, consuming it.
    ///
    /// Returns `None` for anything that is not a live ticket for this session:
    /// unknown, already used, expired, or issued for something else. The four
    /// are deliberately indistinguishable to the caller — telling them apart
    /// would say whether a value they guessed had ever existed.
    pub async fn redeem(&self, value: &str, session: &SessionId) -> Option<RedeemedTicket> {
        // Removed before it is checked, so a redemption that is refused for
        // any reason still burns the ticket. Leaving a rejected one in place
        // would make it retryable, which is the one property single use is
        // supposed to remove.
        let ticket = self.issued.lock().await.remove(value)?;
        if ticket.expires <= Instant::now() {
            tracing::info!(session_id = %session, "an expired attach ticket was presented");
            return None;
        }
        if &ticket.session != session {
            // A ticket for one session presented against another. Worth
            // saying, because it is either a client bug or somebody trying.
            tracing::warn!(
                session_id = %session,
                issued_for = %ticket.session,
                "an attach ticket was presented against a session it was not issued for"
            );
            return None;
        }
        Some(RedeemedTicket {
            caller: Caller::new(ticket.subject, ticket.authorization),
            claim: ticket.claim,
        })
    }
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
