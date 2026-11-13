//! Request holds are separate from customer billing and never authorize replay.

use serde::{Deserialize, Serialize};

use super::{BudgetError, BudgetPolicy};
use crate::AgentId;

/// Operators certify the admitted text model's byte-token and framing bound.
/// Rates in the owner policy must cover the highest applicable model valuation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestModelPolicy {
    pub model: String,
    pub input_overhead_tokens: u64,
}

impl RequestModelPolicy {
    pub fn validate(&self) -> Result<(), BudgetError> {
        if self.model.trim().is_empty()
            || self.model.len() > 128
            || self.model.trim() != self.model
            || !(1..=1_048_576).contains(&self.input_overhead_tokens)
        {
            return Err(BudgetError::InvalidPolicy);
        }
        Ok(())
    }

    pub fn quote(&self, policy: &BudgetPolicy, request: &ModelRequest) -> Result<u64, BudgetError> {
        self.validate()?;
        policy.validate()?;
        if request.model != self.model
            || request.input_bytes == 0
            || request.input_bytes > 32 * 1024
            || request.max_output_tokens == 0
            || request.max_output_tokens > 4096
            || request.fingerprint.len() != 64
            || !request
                .fingerprint
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(BudgetError::Unavailable);
        }
        let input = request
            .input_bytes
            .checked_add(self.input_overhead_tokens)
            .ok_or(BudgetError::Unavailable)?;
        policy.value_usage(input, request.max_output_tokens)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestScope {
    pub agent_id: AgentId,
    pub turn_sequence: u64,
    pub request_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelRequest {
    pub model: String,
    pub input_bytes: u64,
    pub max_output_tokens: u64,
    /// Digest only: prompt content and credentials never enter the budget ledger.
    pub fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestHold {
    pub scope: RequestScope,
    pub request: ModelRequest,
    pub input_tokens_bound: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

#[async_trait::async_trait]
pub trait RequestBudget: Send + Sync {
    /// Only a fresh, confirmed commit permits one send. A repeated key must not
    /// return another send permit, including after an uncertain prior outcome.
    /// None means a confirmed read found this owner outside the enabled scope.
    async fn reserve_request(
        &self,
        scope: &RequestScope,
        request: &ModelRequest,
    ) -> Result<Option<RequestHold>, BudgetError>;

    /// Missing/ambiguous usage leaves the full hold outstanding. Exact duplicate
    /// settlement is idempotent; conflicting settlement fails closed.
    async fn settle_request(
        &self,
        hold: &RequestHold,
        usage: RequestUsage,
    ) -> Result<(), BudgetError>;
}
