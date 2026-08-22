//! Reading a session's reserved-capacity spend from its durable history.

use crate::authz::Caller;
use crate::error::CoreError;
use crate::ids::{RateCardId, SessionId};
use crate::pricing::accrue;
use crate::service::SessionService;

/// What a session has accrued at the price pinned when it was created.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spend {
    pub priced: bool,
    pub rate_card: Option<RateCardId>,
    pub micros_per_hour: Option<u64>,
    pub accrued_micros: Option<u64>,
    pub billable_seconds: Option<u64>,
    pub accruing: bool,
    pub unbounded_intervals: u32,
}

impl Spend {
    fn unpriced() -> Self {
        Self {
            priced: false,
            rate_card: None,
            micros_per_hour: None,
            accrued_micros: None,
            billable_seconds: None,
            accruing: false,
            unbounded_intervals: 0,
        }
    }
}

impl SessionService {
    /// Read what this session has accrued for reserved capacity.
    ///
    /// Ownership is the whole gate. Spend is not a kernel ability and is not
    /// tied to conversation usage: every session plane reserves a sandbox.
    /// The answer is derived from the pinned session price and durable status
    /// log, never from the current deployment price or an in-memory counter.
    pub async fn get_spend(&self, caller: &Caller, id: &SessionId) -> Result<Spend, CoreError> {
        let record = self.record(id).await?;
        if !record.session.is_owned_by(&caller.subject) {
            return Err(CoreError::Forbidden);
        }
        let Some(pricing) = &record.session.pricing else {
            return Ok(Spend::unpriced());
        };
        let events = self.events_since(caller, id, None).await?;
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let accrual = accrue(&events, pricing, &record.session.created_at, now_secs);
        Ok(Spend {
            priced: true,
            rate_card: Some(pricing.rate_card.clone()),
            micros_per_hour: Some(pricing.micros_per_hour),
            accrued_micros: Some(accrual.micros),
            billable_seconds: Some(accrual.billable_seconds),
            accruing: accrual.open,
            unbounded_intervals: accrual.unbounded_intervals,
        })
    }
}
