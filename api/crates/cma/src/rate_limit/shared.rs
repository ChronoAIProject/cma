use super::*;

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Refusal {
    Limited(u64),
    Unavailable,
}

impl RateLimiters {
    pub(super) async fn check_operation(
        &self,
        budget: Budget,
        key: &RateKey,
    ) -> Result<(), Refusal> {
        // Reject repeated local excess before it can generate database traffic.
        self.check(budget, key).map_err(|delay| {
            if matches!(budget, Budget::Default) {
                lease::record("refused");
            }
            Refusal::Limited(delay)
        })?;
        let Some((store, config)) = &self.shared else {
            return Ok(());
        };
        let (tier, limit) = match budget {
            Budget::Default => ("default", config.default_per_minute),
            Budget::Create => ("create", config.create_per_minute),
            Budget::Input => ("input", config.input_per_minute),
            Budget::Control => ("control", config.control_per_minute),
            // Recovery lookups have their own counter and the same finite
            // per-caller ceiling as control submissions.
            Budget::Recovery => ("recovery", config.control_per_minute),
            Budget::Admission | Budget::Health => unreachable!("admission remains per pod"),
        };
        use sha2::{Digest, Sha256};
        let identity = match key {
            RateKey::Subject(subject) => format!("subject:{subject}"),
            RateKey::Client(ClientKey::Network(ip)) => format!("network:{ip}"),
            RateKey::Client(ClientKey::Unidentified) => "unidentified".into(),
        };
        let key = format!("{tier}:{:x}", Sha256::digest(identity.as_bytes()));
        if matches!(budget, Budget::Default) && config.default_lease_enabled {
            return self
                .default_leases
                .check(store.as_ref(), &key, limit.get())
                .await;
        }
        let outcome = match store.check(&key, limit.get()).await {
            Ok(None) => Ok(()),
            Ok(Some(delay)) => Err(Refusal::Limited(delay)),
            Err(error) => {
                tracing::warn!(%error, "shared request budget unavailable");
                Err(Refusal::Unavailable)
            }
        };
        if matches!(budget, Budget::Default) {
            lease::record(match &outcome {
                Ok(()) => "charged",
                Err(Refusal::Limited(_)) => "refused",
                Err(Refusal::Unavailable) => "unavailable",
            });
        }
        outcome
    }
}

pub(super) fn unavailable_response() -> Response {
    HttpProblem::RequestBudgetUnavailable.into_response()
}
