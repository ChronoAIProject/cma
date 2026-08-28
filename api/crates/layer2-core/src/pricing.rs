//! The deployment's complete price history.
//!
//! A price is an account of what a session was charged under, not merely a
//! setting for the next session. Cards are therefore appended and sealed:
//! their figures and effective times never change after they enter the list,
//! and retiring one is the only permitted mutation to an existing card.

use serde::{Deserialize, Serialize};

use crate::clock::seconds_from_rfc3339;
use crate::event::{Event, EventKind};
use crate::session::{SessionStatus, StopReason};

pub use crate::ids::RateCardId;

/// The only currency this deployment prices in.
pub const CURRENCY: &str = "USD";

/// What a deployment charges for capacity, in micro-USD.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rate {
    /// Micro-USD charged for one CPU reserved for one hour.
    pub cpu_micros_per_cpu_hour: u64,
    /// Micro-USD charged for one GiB reserved for one hour.
    pub memory_micros_per_gib_hour: u64,
}

impl Rate {
    /// Price one hour of a reservation, in micro-USD.
    ///
    /// Memory is measured in MiB on the session and billed per GiB, so 1,024
    /// MiB is one billed unit. Multiplication happens before division to keep
    /// fractional cores and GiB exact; `u128` intermediates prevent a wrapped
    /// result from becoming a plausible but wrong price.
    pub fn micros_per_hour(&self, cpu_millis: u32, memory_mib: u32) -> Option<u64> {
        let cpu = self.cpu_micros_per_cpu_hour as u128 * cpu_millis as u128 / 1_000;
        let mem = self.memory_micros_per_gib_hour as u128 * memory_mib as u128 / 1_024;
        u64::try_from(cpu + mem).ok()
    }
}

/// A service-computed price for the size a new session will actually receive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Quote {
    pub rate_card: RateCardId,
    pub rate: Rate,
    pub cpu_millis: u32,
    pub memory_mib: u32,
    pub micros_per_hour: u64,
    /// What a person is shown. Rendered here so no client rounds.
    pub display: String,
    pub currency: &'static str,
    /// RFC 3339 in UTC.
    pub quoted_at: String,
}

/// What a session was priced at, settled once when it was created.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionPricing {
    pub rate_card: RateCardId,
    pub rate: Rate,
    pub cpu_millis: u32,
    pub memory_mib: u32,
    pub micros_per_hour: u64,
    /// RFC 3339 in UTC.
    pub quoted_at: String,
}

/// Capacity time a session has accrued at its pinned rate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Accrual {
    pub billable_seconds: u64,
    pub micros: u64,
    /// Whether an interval is still open, so this session is accruing now.
    pub open: bool,
    /// When the open interval started, when that bound can be trusted.
    pub since: Option<String>,
    /// Intervals skipped because their bounds could not be trusted.
    pub unbounded_intervals: u32,
}

/// What a session has accrued, derived from its own durable history.
///
/// Pure: no clock, store, runtime, or settings. `now_secs` is supplied so the
/// same history always produces the same answer. The ledger has one-second
/// resolution because its durable timestamps do, and divides only after all
/// billable seconds have been combined, rounding deterministically toward the
/// owner.
pub fn accrue(
    events: &[Event],
    pricing: &SessionPricing,
    created_at: &str,
    now_secs: u64,
) -> Accrual {
    let created = interval_bound(created_at);
    let mut open: Option<(String, Option<u64>)> = None;
    let mut billable_seconds = 0_u64;
    let mut unbounded_intervals = 0_u32;

    for event in events {
        let EventKind::StatusChanged {
            status,
            stop_reason,
            ..
        } = &event.kind
        else {
            continue;
        };
        let next_holds = holds_capacity(*status, *stop_reason);
        match (open.is_some(), next_holds) {
            (false, true) => {
                open = Some((
                    event.appended_at.clone(),
                    interval_bound(&event.appended_at),
                ));
            }
            (true, false) => {
                let (_, start) = open.take().expect("the interval is open");
                let end = interval_bound(&event.appended_at);
                match bounded_seconds(start, end, created, now_secs) {
                    Some(seconds) => {
                        billable_seconds = billable_seconds.saturating_add(seconds);
                    }
                    None => {
                        unbounded_intervals = unbounded_intervals.saturating_add(1);
                    }
                }
            }
            _ => {}
        }
    }

    let is_open = open.is_some();
    let mut since = None;
    if let Some((stamp, start)) = open {
        match bounded_seconds(start, (now_secs > 0).then_some(now_secs), created, now_secs) {
            Some(seconds) => {
                billable_seconds = billable_seconds.saturating_add(seconds);
                since = Some(stamp);
            }
            None => {
                unbounded_intervals = unbounded_intervals.saturating_add(1);
            }
        }
    }

    let micros = pricing.micros_per_hour as u128 * billable_seconds as u128 / 3_600;
    Accrual {
        billable_seconds,
        micros: micros.min(u128::from(u64::MAX)) as u64,
        open: is_open,
        since,
        unbounded_intervals,
    }
}

fn interval_bound(stamp: &str) -> Option<u64> {
    seconds_from_rfc3339(stamp).filter(|seconds| *seconds > 0)
}

fn bounded_seconds(
    start: Option<u64>,
    end: Option<u64>,
    created: Option<u64>,
    now_secs: u64,
) -> Option<u64> {
    let (start, end, created) = (start?, end?, created?);
    if start < created || end > now_secs {
        return None;
    }
    Some(end.saturating_sub(start))
}

fn holds_capacity(status: SessionStatus, stop_reason: Option<StopReason>) -> bool {
    match status {
        SessionStatus::Ready | SessionStatus::Running | SessionStatus::Rotating => true,
        SessionStatus::Idle => stop_reason.is_some_and(|reason| !reason.is_terminal()),
        SessionStatus::Provisioning | SessionStatus::Terminated => false,
    }
}

/// Micro-USD as the lossless string a person is shown.
pub fn format_usd(micros: u64) -> String {
    let dollars = micros / 1_000_000;
    let mut fraction = format!("{:06}", micros % 1_000_000);
    while fraction.len() > 2 && fraction.ends_with('0') {
        fraction.pop();
    }
    format!("${dollars}.{fraction}")
}

/// Price a bounded number of seconds at one hourly micro-USD rate.
///
/// Trigger preview and authenticated Launch admission deliberately share this
/// arithmetic so the anonymous estimate cannot drift from the quote later
/// pinned at review. Division rounds deterministically toward the contributor.
pub(crate) fn estimate_micros(micros_per_hour: u64, seconds: u32) -> Option<u64> {
    let estimated = (u128::from(micros_per_hour) * u128::from(seconds)) / 3_600;
    u64::try_from(estimated).ok()
}

/// One rate the deployment has charged under.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RateCard {
    pub id: RateCardId,
    /// RFC 3339 in UTC, minted when the card was appended.
    pub effective_from: String,
    pub rate: Rate,
    /// RFC 3339 in UTC. `Some` once a later card superseded this one.
    pub retired_at: Option<String>,
}

/// Every rate this deployment has ever charged, oldest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PriceList {
    pub cards: Vec<RateCard>,
}

/// Why one price list cannot legitimately follow another.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PriceListViolation {
    #[error("card {id} was removed")]
    CardRemoved { id: RateCardId },
    #[error("card {id} changed field '{field}'")]
    CardChanged { id: RateCardId, field: &'static str },
    #[error("card {id} had its retirement cleared")]
    RetirementCleared { id: RateCardId },
    #[error("the order of existing cards changed")]
    OrderChanged,
    #[error("{appended} cards were appended in one change")]
    SeveralAppended { appended: usize },
    #[error("cards {first} and {second} are both in force")]
    SeveralInForce {
        first: RateCardId,
        second: RateCardId,
    },
}

impl PriceList {
    /// The newest card which has not been retired.
    ///
    /// A valid list has at most one. If a stored document has several, the
    /// newest is the least surprising rate for a new quote and the corruption
    /// is reported rather than hidden.
    pub fn in_force(&self) -> Option<&RateCard> {
        let mut current: Option<&RateCard> = None;
        for candidate in self.cards.iter().filter(|card| card.retired_at.is_none()) {
            if let Some(first) = current {
                tracing::error!(
                    first = %first.id,
                    second = %candidate.id,
                    "several price cards are in force; using the newest"
                );
            }
            current = Some(candidate);
        }
        current
    }

    /// Find the card that priced a session, including after it was retired.
    pub fn card(&self, id: &RateCardId) -> Option<&RateCard> {
        self.cards.iter().find(|card| &card.id == id)
    }

    /// Return a new list with one rate appended at `at`.
    ///
    /// The same instant retires the outgoing card and starts the incoming one,
    /// leaving neither a gap nor an overlap in the declared history.
    pub fn appended(&self, rate: Rate, at: &str) -> PriceList {
        let current = self.in_force().map(|card| card.id.clone());
        let mut next = self.clone();
        if let Some(id) = current {
            let card = next
                .cards
                .iter_mut()
                .rev()
                .find(|card| card.id == id && card.retired_at.is_none())
                .expect("the in-force card came from this list");
            card.retired_at = Some(at.to_string());
        }
        next.cards.push(RateCard {
            id: RateCardId::generate(),
            effective_from: at.to_string(),
            rate,
            retired_at: None,
        });
        next
    }

    /// Whether `self` is a legitimate successor to `previous`.
    pub fn sealed_against(&self, previous: &PriceList) -> Result<(), PriceListViolation> {
        if self.cards.len() < previous.cards.len() {
            let removed = previous
                .cards
                .iter()
                .find(|old| !self.cards.iter().any(|card| card.id == old.id))
                .unwrap_or(&previous.cards[self.cards.len()]);
            return Err(PriceListViolation::CardRemoved {
                id: removed.id.clone(),
            });
        }

        for old in &previous.cards {
            if !self.cards.iter().any(|card| card.id == old.id) {
                return Err(PriceListViolation::CardRemoved { id: old.id.clone() });
            }
        }

        if previous
            .cards
            .iter()
            .zip(&self.cards)
            .any(|(old, current)| old.id != current.id)
        {
            return Err(PriceListViolation::OrderChanged);
        }

        for (old, current) in previous.cards.iter().zip(&self.cards) {
            if current.effective_from != old.effective_from {
                return Err(PriceListViolation::CardChanged {
                    id: old.id.clone(),
                    field: "effective_from",
                });
            }
            if current.rate != old.rate {
                return Err(PriceListViolation::CardChanged {
                    id: old.id.clone(),
                    field: "rate",
                });
            }
            match (&old.retired_at, &current.retired_at) {
                (Some(_), None) => {
                    return Err(PriceListViolation::RetirementCleared { id: old.id.clone() });
                }
                (Some(old_at), Some(current_at)) if old_at != current_at => {
                    return Err(PriceListViolation::CardChanged {
                        id: old.id.clone(),
                        field: "retired_at",
                    });
                }
                _ => {}
            }
        }

        let appended = self.cards.len() - previous.cards.len();
        if appended > 1 {
            return Err(PriceListViolation::SeveralAppended { appended });
        }

        let mut in_force = self.cards.iter().filter(|card| card.retired_at.is_none());
        match (in_force.next(), in_force.next()) {
            (Some(first), Some(second)) => {
                return Err(PriceListViolation::SeveralInForce {
                    first: first.id.clone(),
                    second: second.id.clone(),
                });
            }
            (None, None) if !self.cards.is_empty() => {
                let last = self.cards.last().expect("the list is not empty");
                return Err(PriceListViolation::CardChanged {
                    id: last.id.clone(),
                    field: "retired_at",
                });
            }
            _ => {}
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rate(cpu: u64, memory: u64) -> Rate {
        Rate {
            cpu_micros_per_cpu_hour: cpu,
            memory_micros_per_gib_hour: memory,
        }
    }

    fn first_card() -> PriceList {
        PriceList::default().appended(rate(40_000, 5_000), "2026-08-23T01:00:00Z")
    }

    fn parse_display(display: &str) -> u64 {
        let (dollars, fraction) = display
            .strip_prefix('$')
            .and_then(|amount| amount.split_once('.'))
            .expect("the display is a dollar amount");
        let padded = format!("{fraction:0<6}");
        dollars.parse::<u64>().expect("whole dollars fit") * 1_000_000
            + padded.parse::<u64>().expect("micro-dollars fit")
    }

    #[test]
    fn fractional_capacity_is_priced_after_multiplication() {
        assert_eq!(rate(40_000, 0).micros_per_hour(250, 1), Some(10_000));
    }

    #[test]
    fn cpu_and_binary_memory_prices_are_added_exactly() {
        assert_eq!(
            rate(40_000, 5_000).micros_per_hour(1_000, 1_024),
            Some(45_000)
        );
    }

    #[test]
    fn a_price_larger_than_u64_is_refused() {
        assert_eq!(
            rate(u64::MAX, u64::MAX).micros_per_hour(u32::MAX, u32::MAX),
            None
        );
    }

    #[test]
    fn display_is_lossless_for_the_full_required_range() {
        for micros in [0, 1, 45_000, 1_250_000, 40_000_000, u64::MAX] {
            let display = format_usd(micros);
            assert_eq!(parse_display(&display), micros, "display was {display}");
        }
    }

    #[test]
    fn display_keeps_two_places_without_rounding_away_micro_dollars() {
        assert_eq!(format_usd(45_000), "$0.045");
        assert_eq!(format_usd(1_250_000), "$1.25");
        assert_eq!(format_usd(40_000_000), "$40.00");
        assert_eq!(format_usd(1), "$0.000001");
    }

    #[test]
    fn an_empty_list_has_no_card_in_force() {
        let list = PriceList::default();
        assert!(list.in_force().is_none());
        assert!(list.sealed_against(&PriceList::default()).is_ok());
    }

    #[test]
    fn appending_retires_the_previous_card_at_the_same_instant() {
        let previous = first_card();
        let next = previous.appended(rate(50_000, 6_000), "2026-08-23T02:00:00Z");

        assert_eq!(
            next.cards[0].retired_at.as_deref(),
            Some(next.cards[1].effective_from.as_str())
        );
        assert_eq!(next.in_force(), next.cards.last());
    }

    #[test]
    fn appending_never_touches_a_historical_card() {
        let first = first_card();
        let original = first.cards[0].clone();
        let second = first.appended(rate(50_000, 6_000), "2026-08-23T02:00:00Z");
        let third = second.appended(rate(60_000, 7_000), "2026-08-23T03:00:00Z");

        assert_eq!(third.cards[0].id, original.id);
        assert_eq!(third.cards[0].effective_from, original.effective_from);
        assert_eq!(third.cards[0].rate, original.rate);
    }

    #[test]
    fn a_card_that_priced_a_session_is_still_readable_after_it_is_retired() {
        let previous = first_card();
        let id = previous.cards[0].id.clone();
        let next = previous.appended(rate(50_000, 6_000), "2026-08-23T02:00:00Z");

        let historical = next.card(&id).expect("the retired card remains");
        assert_eq!(historical.rate, rate(40_000, 5_000));
        assert!(historical.retired_at.is_some());
    }

    #[test]
    fn sealed_against_refuses_a_rewritten_rate() {
        let previous = first_card();
        let mut changed = previous.clone();
        changed.cards[0].rate.cpu_micros_per_cpu_hour += 1;

        assert_eq!(
            changed.sealed_against(&previous),
            Err(PriceListViolation::CardChanged {
                id: previous.cards[0].id.clone(),
                field: "rate",
            })
        );
    }

    #[test]
    fn sealed_against_refuses_a_rewritten_effective_time() {
        let previous = first_card();
        let mut changed = previous.clone();
        changed.cards[0].effective_from = "2026-08-23T01:00:01Z".into();

        assert_eq!(
            changed.sealed_against(&previous),
            Err(PriceListViolation::CardChanged {
                id: previous.cards[0].id.clone(),
                field: "effective_from",
            })
        );
    }

    #[test]
    fn sealed_against_refuses_a_deleted_card() {
        let previous = first_card();
        let mut changed = previous.clone();
        changed.cards.clear();

        assert_eq!(
            changed.sealed_against(&previous),
            Err(PriceListViolation::CardRemoved {
                id: previous.cards[0].id.clone(),
            })
        );
    }

    #[test]
    fn sealed_against_refuses_a_cleared_retirement() {
        let previous = first_card().appended(rate(50_000, 6_000), "2026-08-23T02:00:00Z");
        let mut changed = previous.clone();
        changed.cards[0].retired_at = None;

        assert_eq!(
            changed.sealed_against(&previous),
            Err(PriceListViolation::RetirementCleared {
                id: previous.cards[0].id.clone(),
            })
        );
    }

    #[test]
    fn sealed_against_refuses_a_changed_retirement() {
        let previous = first_card().appended(rate(50_000, 6_000), "2026-08-23T02:00:00Z");
        let mut changed = previous.clone();
        changed.cards[0].retired_at = Some("2026-08-23T02:00:01Z".into());

        assert_eq!(
            changed.sealed_against(&previous),
            Err(PriceListViolation::CardChanged {
                id: previous.cards[0].id.clone(),
                field: "retired_at",
            })
        );
    }

    #[test]
    fn sealed_against_refuses_a_reordering() {
        let previous = first_card().appended(rate(50_000, 6_000), "2026-08-23T02:00:00Z");
        let mut changed = previous.clone();
        changed.cards.swap(0, 1);

        assert_eq!(
            changed.sealed_against(&previous),
            Err(PriceListViolation::OrderChanged)
        );
    }

    #[test]
    fn sealed_against_refuses_two_appends_in_one_save() {
        let previous = PriceList::default();
        let changed = previous
            .appended(rate(40_000, 5_000), "2026-08-23T01:00:00Z")
            .appended(rate(50_000, 6_000), "2026-08-23T02:00:00Z");

        assert_eq!(
            changed.sealed_against(&previous),
            Err(PriceListViolation::SeveralAppended { appended: 2 })
        );
    }

    #[test]
    fn sealed_against_refuses_two_cards_in_force() {
        let previous = first_card();
        let mut changed = previous.clone();
        changed.cards.push(RateCard {
            id: RateCardId("rate_second".into()),
            effective_from: "2026-08-23T02:00:00Z".into(),
            rate: rate(50_000, 6_000),
            retired_at: None,
        });

        assert_eq!(
            changed.sealed_against(&previous),
            Err(PriceListViolation::SeveralInForce {
                first: previous.cards[0].id.clone(),
                second: RateCardId("rate_second".into()),
            })
        );
    }

    #[test]
    fn sealed_against_refuses_a_nonempty_list_with_no_card_in_force() {
        let previous = first_card();
        let mut changed = previous.clone();
        changed.cards[0].retired_at = Some("2026-08-23T02:00:00Z".into());

        assert_eq!(
            changed.sealed_against(&previous),
            Err(PriceListViolation::CardChanged {
                id: previous.cards[0].id.clone(),
                field: "retired_at",
            })
        );
    }

    #[test]
    fn sealed_against_accepts_a_legitimate_append() {
        let previous = first_card();
        let changed = previous.appended(rate(50_000, 6_000), "2026-08-23T02:00:00Z");
        assert!(changed.sealed_against(&previous).is_ok());
    }

    #[test]
    fn sealed_against_accepts_an_unchanged_list() {
        let previous = first_card();
        assert!(previous.sealed_against(&previous).is_ok());
    }

    #[test]
    fn two_cards_in_force_are_reported_rather_than_hidden() {
        let mut list = first_card();
        list.cards.push(RateCard {
            id: RateCardId("rate_newest".into()),
            effective_from: "2026-08-23T02:00:00Z".into(),
            rate: rate(50_000, 6_000),
            retired_at: None,
        });

        assert_eq!(list.in_force(), list.cards.last());
    }
}

#[cfg(test)]
#[path = "pricing_accrual_tests.rs"]
mod accrual_tests;
