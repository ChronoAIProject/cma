//! Shared physical slots; observers leave an optional reserve for controls.
use super::*;
use tokio::sync::OwnedSemaphorePermit;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Class {
    Observer,
    Ordinary,
    Control,
}

pub(super) fn class(path: &str, method: &axum::http::Method) -> Class {
    if path.starts_with("/internal/agent-manager/v1/live/") {
        Class::Observer
    } else if path.starts_with("/internal/agent-manager/v1/control/")
        || (*method == axum::http::Method::POST
            && (path.ends_with("/recover-worktree") || path.ends_with("/skills")))
    {
        Class::Control
    } else {
        Class::Ordinary
    }
}
impl Class {
    fn label(self) -> &'static str {
        match self {
            Self::Observer => "observer",
            Self::Ordinary => "ordinary",
            Self::Control => "control",
        }
    }
}
pub(super) struct Permit {
    permit: Option<OwnedSemaphorePermit>,
    class: Class,
}
impl Drop for Permit {
    fn drop(&mut self) {
        drop(self.permit.take());
        metrics::gauge!("cma_relay_permits_in_use", "class" => self.class.label()).decrement(1.);
    }
}
impl Relay {
    pub(super) fn acquire(&self, class: Class) -> Result<Permit, ()> {
        let _gate = self.capacity_gate.lock().unwrap();
        let permit = if !matches!(class, Class::Control)
            && self.slots.available_permits() <= self.capacity.control_reserve
        {
            None
        } else {
            self.slots.clone().try_acquire_owned().ok()
        };
        let Some(permit) = permit else {
            metrics::counter!("cma_relay_refusals_total", "class" => class.label(), "reason" => "slots_exhausted").increment(1);
            return Err(());
        };
        metrics::gauge!("cma_relay_permits_in_use", "class" => class.label()).increment(1.);
        metrics::counter!("cma_relay_acquisitions_total", "class" => class.label()).increment(1);
        Ok(Permit {
            permit: Some(permit),
            class,
        })
    }
}

pub(super) fn refused(observer: bool) -> axum::response::Response {
    use axum::response::IntoResponse;
    if observer {
        layer1_public::rest::HttpProblem::RelayObserverCapacityExhausted
    } else {
        layer1_public::rest::HttpProblem::RelayCapacityExhausted
    }
    .into_response()
}
