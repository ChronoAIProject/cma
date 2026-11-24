//! Negotiated presentation policy, independent of provider receipt URLs.
use axum::http::HeaderMap;
use layer2_core::sandbox::{agent_io::AgentEventKind, AgentEventStreamAuthorization};

use super::{error::Error, shapes::EventItem};

pub(super) const PREFERENCE: &str = "cma-external-approval-v1";

#[derive(Clone, Default)]
pub(super) struct ReviewOrigin(Option<String>);

impl ReviewOrigin {
    pub fn configured(value: &str) -> Self {
        let valid = url::Url::parse(value).ok().filter(|url| {
            url.scheme() == "https"
                && url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none()
                && url.path() == "/"
                && !value.contains('\\')
                && !value.chars().any(|c| c.is_whitespace() || c.is_control())
        });
        Self(valid.map(|url| url.origin().ascii_serialization()))
    }
}

#[derive(Clone)]
pub(super) struct Preference {
    requested: bool,
    origin: ReviewOrigin,
    review: bool,
}

impl Preference {
    pub fn new(
        requested: bool,
        origin: &ReviewOrigin,
        authority: &AgentEventStreamAuthorization,
    ) -> Self {
        Self {
            requested,
            origin: origin.clone(),
            review: authority.can_review_external_approval() && origin.0.is_some(),
        }
    }

    pub fn project(&self, items: &mut [EventItem]) -> Result<(), Error> {
        for item in items {
            match &mut item.kind {
                Some(AgentEventKind::ExternalApprovalRequested { approve_url, .. }) => {
                    if !self.requested {
                        return Err(Error::ExternalApprovalCapability);
                    }
                    *approve_url = self
                        .origin
                        .0
                        .as_ref()
                        .filter(|_| self.review)
                        .map(|origin| format!("{origin}/assistant/approvals"));
                }
                Some(AgentEventKind::ExternalApprovalResolved { .. }) if !self.requested => {
                    return Err(Error::ExternalApprovalCapability);
                }
                _ => {}
            }
        }
        Ok(())
    }

    pub fn headers(&self, headers: &mut HeaderMap) {
        if !self.requested {
            return;
        }
        headers.insert("preference-applied", PREFERENCE.parse().unwrap());
        headers.insert(
            "cma-external-approval-action",
            if self.review { "review" } else { "none" }.parse().unwrap(),
        );
        if let Some(origin) = &self.origin.0 {
            if let Ok(origin) = origin.parse() {
                headers.insert("cma-approval-review-origin", origin);
            }
        }
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod producer_tests;
