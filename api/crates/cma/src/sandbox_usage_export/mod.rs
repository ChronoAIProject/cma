//! Evidence delivery has no funding, pricing or wallet prerequisite.
mod contract;
mod nyxid;
mod reconcile;
mod worker;

use crate::store::sandbox_usage_export::{
    CompletenessAttempt, CompletenessReceipt, FrozenUsageEvent,
};
#[cfg(test)]
pub(crate) use contract::MissingRange;
pub(crate) use contract::{Cursor, Receipt};
pub(crate) use worker::UsageExporter;

#[cfg(test)]
pub(crate) fn http_for_test(
    base: reqwest::Url,
    tokens: std::sync::Arc<dyn crate::producer_token::ProducerTokenProvider>,
) -> std::sync::Arc<dyn UsageTransport> {
    std::sync::Arc::new(nyxid::NyxIdUsageTransport::with_tokens(base, tokens).unwrap())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TransportError {
    Unavailable,
    RateLimited(u64),
    Unauthorized,
    Forbidden,
    CursorChanged,
    Prerequisite,
    Conflict,
    Invalid,
}

#[async_trait::async_trait]
pub(crate) trait UsageTransport: Send + Sync {
    async fn submit(&self, event: &FrozenUsageEvent) -> Result<Receipt, TransportError>;
    async fn receipt(&self, event_id: &str) -> Result<Option<Receipt>, TransportError>;
    async fn cursor(&self, stream_id: &str) -> Result<Option<Cursor>, TransportError>;
    async fn submit_completeness(
        &self,
        _attempt: &CompletenessAttempt,
    ) -> Result<CompletenessReceipt, TransportError> {
        Err(TransportError::Unavailable)
    }
    async fn completeness_receipt(
        &self,
        _id: &str,
    ) -> Result<Option<CompletenessReceipt>, TransportError> {
        Err(TransportError::Unavailable)
    }
}

pub(crate) mod completeness_contract;
#[cfg(test)]
mod completeness_fixture_tests;
