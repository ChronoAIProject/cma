//! What a ticket must and must not do.
//!
//! The properties here are the entire reason a ticket in a URL is acceptable
//! at all, so each of them is asserted rather than assumed.

use layer2_core::{Authorization, Capability};

use super::*;

fn caller(subject: &str) -> Caller {
    Caller::new(
        SubjectId(subject.to_string()),
        Authorization::from_capabilities([Capability::UseOwnSessions]),
    )
}

fn session(id: &str) -> SessionId {
    SessionId(id.to_string())
}

#[tokio::test]
async fn a_ticket_redeems_once_and_only_once() {
    let tickets = TerminalTickets::in_memory();
    let issued = tickets
        .issue(&caller("subject-a"), &session("sesn_1"), Default::default())
        .await
        .expect("ticket is stored");

    assert!(
        tickets
            .redeem(&issued.value, &session("sesn_1"))
            .await
            .expect("ticket store answers")
            .is_some(),
        "the first redemption must work"
    );
    assert!(
        tickets
            .redeem(&issued.value, &session("sesn_1"))
            .await
            .expect("ticket store answers")
            .is_none(),
        "a ticket that can be replayed is a credential, not a ticket"
    );
}

#[tokio::test]
async fn redemption_restores_the_subject_and_the_grants_they_held() {
    let tickets = TerminalTickets::in_memory();
    let issued = tickets
        .issue(&caller("subject-a"), &session("sesn_1"), Default::default())
        .await
        .expect("ticket is stored");

    let redeemed = tickets
        .redeem(&issued.value, &session("sesn_1"))
        .await
        .expect("ticket store answers")
        .expect("redeems");
    assert_eq!(redeemed.caller.subject, SubjectId("subject-a".into()));
    assert!(
        redeemed
            .caller
            .authorization
            .allows(Capability::UseOwnSessions),
        "a ticket that lost its grants would attach as somebody who may do nothing"
    );
}

#[tokio::test]
async fn a_ticket_is_bound_to_the_session_it_was_issued_for() {
    // Otherwise one attachable session would be a key to every other one the
    // subject owns.
    let tickets = TerminalTickets::in_memory();
    let issued = tickets
        .issue(&caller("subject-a"), &session("sesn_1"), Default::default())
        .await
        .expect("ticket is stored");

    assert!(
        tickets
            .redeem(&issued.value, &session("sesn_2"))
            .await
            .expect("ticket store answers")
            .is_none(),
        "a ticket must not open a session it was not issued for"
    );
}

#[tokio::test]
async fn a_ticket_refused_for_the_wrong_session_is_still_spent() {
    // Refusing without consuming would leave it retryable, which is exactly
    // the property single use exists to remove.
    let tickets = TerminalTickets::in_memory();
    let issued = tickets
        .issue(&caller("subject-a"), &session("sesn_1"), Default::default())
        .await
        .expect("ticket is stored");

    assert!(tickets
        .redeem(&issued.value, &session("sesn_2"))
        .await
        .expect("ticket store answers")
        .is_none());
    assert!(
        tickets
            .redeem(&issued.value, &session("sesn_1"))
            .await
            .expect("ticket store answers")
            .is_none(),
        "a refused redemption must burn the ticket"
    );
}

#[tokio::test]
async fn an_expired_ticket_is_refused() {
    let tickets = TerminalTickets::with_lifetime(
        Arc::new(MemoryTerminalTicketStore::default()),
        Duration::ZERO,
    );
    let issued = tickets
        .issue(&caller("subject-a"), &session("sesn_1"), Default::default())
        .await
        .expect("ticket is stored");

    assert!(
        tickets
            .redeem(&issued.value, &session("sesn_1"))
            .await
            .expect("ticket store answers")
            .is_none(),
        "an expired ticket must not open anything"
    );
}

#[tokio::test]
async fn a_value_nobody_issued_is_refused() {
    let tickets = TerminalTickets::in_memory();
    assert!(tickets
        .redeem("not-a-ticket-anybody-minted", &session("sesn_1"))
        .await
        .expect("ticket store answers")
        .is_none());
}

#[tokio::test]
async fn a_takeover_ticket_carries_the_claim_it_was_asked_for() {
    // The decision is made where the caller was authenticated, not at the
    // upgrade: a ticket redeemed later must not be able to escalate itself.
    let tickets = TerminalTickets::in_memory();
    let ordinary = tickets
        .issue(&caller("subject-a"), &session("sesn_1"), Default::default())
        .await
        .expect("ticket is stored");
    let forceful = tickets
        .issue(
            &caller("subject-a"),
            &session("sesn_1"),
            AttachmentClaim::Takeover,
        )
        .await
        .expect("ticket is stored");

    assert_eq!(
        tickets
            .redeem(&ordinary.value, &session("sesn_1"))
            .await
            .expect("ticket store answers")
            .expect("redeems")
            .claim,
        AttachmentClaim::Exclusive
    );
    assert_eq!(
        tickets
            .redeem(&forceful.value, &session("sesn_1"))
            .await
            .expect("ticket store answers")
            .expect("redeems")
            .claim,
        AttachmentClaim::Takeover
    );
}

#[tokio::test]
async fn two_tickets_are_never_the_same_value() {
    let tickets = TerminalTickets::in_memory();
    let first = tickets
        .issue(&caller("subject-a"), &session("sesn_1"), Default::default())
        .await
        .expect("ticket is stored");
    let second = tickets
        .issue(&caller("subject-a"), &session("sesn_1"), Default::default())
        .await
        .expect("ticket is stored");
    assert_ne!(first.value, second.value);
    assert!(
        first.value.len() >= 64,
        "a guessable ticket is a session anybody can open: {}",
        first.value.len()
    );
}

#[tokio::test]
async fn expired_tickets_do_not_accumulate() {
    // The store is swept where it is known to be growing. Without it a busy
    // deployment holds every ticket it ever minted.
    let store = Arc::new(MemoryTerminalTicketStore::default());
    let tickets = TerminalTickets::with_lifetime(store.clone(), Duration::ZERO);
    let stale = tickets
        .issue(&caller("subject-a"), &session("sesn_1"), Default::default())
        .await
        .expect("ticket is stored");

    let _fresh = tickets
        .issue(&caller("subject-a"), &session("sesn_1"), Default::default())
        .await
        .expect("ticket is stored");
    let held = store.issued.lock().await;
    assert_eq!(held.len(), 1, "the expired ticket should have been swept");
    assert!(!held.contains_key(&identify(&stale.value)));
}

#[tokio::test]
async fn a_ticket_issued_by_one_instance_redeems_on_another() {
    let store = Arc::new(MemoryTerminalTicketStore::default());
    let first = TerminalTickets::new(store.clone());
    let second = TerminalTickets::new(store);
    let issued = first
        .issue(&caller("subject-a"), &session("sesn_1"), Default::default())
        .await
        .expect("ticket is stored");

    let redeemed = second
        .redeem(&issued.value, &session("sesn_1"))
        .await
        .expect("ticket store answers");
    assert!(
        redeemed.is_some(),
        "another instance must see durable state"
    );
}

#[tokio::test]
async fn concurrent_redemptions_have_exactly_one_winner() {
    let store = Arc::new(MemoryTerminalTicketStore::default());
    let tickets = TerminalTickets::new(store);
    let issued = tickets
        .issue(&caller("subject-a"), &session("sesn_1"), Default::default())
        .await
        .expect("ticket is stored");
    let left = Arc::clone(&tickets);
    let right = Arc::clone(&tickets);
    let value_left = issued.value.clone();
    let value_right = issued.value;
    let id = session("sesn_1");

    let (left, right) = tokio::join!(
        left.redeem(&value_left, &id),
        right.redeem(&value_right, &id),
    );
    let winners = [left, right]
        .into_iter()
        .map(|result| result.expect("ticket store answers"))
        .filter(Option::is_some)
        .count();
    assert_eq!(winners, 1);
}

struct UnavailableStore;

#[async_trait::async_trait]
impl TerminalTicketStore for UnavailableStore {
    async fn put(&self, _: StoredTicket) -> Result<(), StoreError> {
        Err(StoreError::Unavailable("test outage".into()))
    }

    async fn take(&self, _: &TicketId) -> Result<Option<StoredTicket>, StoreError> {
        Err(StoreError::Unavailable("test outage".into()))
    }
}

#[tokio::test]
async fn a_store_outage_is_not_reported_as_an_invalid_ticket() {
    let tickets = TerminalTickets::new(Arc::new(UnavailableStore));
    assert!(tickets
        .issue(&caller("subject-a"), &session("sesn_1"), Default::default())
        .await
        .is_err());
    assert!(tickets.redeem("unknown", &session("sesn_1")).await.is_err());
}
