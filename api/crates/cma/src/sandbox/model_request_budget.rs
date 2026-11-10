use std::sync::Arc;

use layer2_core::budget::{
    BudgetError, ModelRequest, RequestBudget, RequestHold, RequestScope, RequestUsage,
};
use sha2::{Digest, Sha256};

#[derive(Clone)]
pub(super) struct RequestBudgetContext {
    pub budget: Arc<dyn RequestBudget>,
    pub scope: RequestScope,
}

pub(super) struct RequestPermit {
    budget: Arc<dyn RequestBudget>,
    hold: RequestHold,
}

impl RequestBudgetContext {
    pub async fn reserve(
        &self,
        model: &str,
        body: &[u8],
        max_output_tokens: u64,
    ) -> Result<Option<RequestPermit>, BudgetError> {
        let request = ModelRequest {
            model: model.into(),
            input_bytes: body.len() as u64,
            max_output_tokens,
            fingerprint: format!("{:x}", Sha256::digest(body)),
        };
        self.budget
            .reserve_request(&self.scope, &request)
            .await
            .map(|hold| {
                hold.map(|hold| RequestPermit {
                    budget: self.budget.clone(),
                    hold,
                })
            })
    }
}

impl RequestPermit {
    pub async fn settle(&self, body: &[u8]) -> Result<(), BudgetError> {
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(body) else {
            return Ok(());
        };
        if value["status"] != "completed"
            || !value["error"].is_null()
            || value["model"].as_str() != Some(self.hold.request.model.as_str())
        {
            return Ok(());
        }
        let Some(input_tokens) = value["usage"]["input_tokens"].as_u64() else {
            return Ok(());
        };
        let Some(output_tokens) = value["usage"]["output_tokens"].as_u64() else {
            return Ok(());
        };
        self.budget
            .settle_request(
                &self.hold,
                RequestUsage {
                    input_tokens,
                    output_tokens,
                },
            )
            .await?;
        if input_tokens > self.hold.input_tokens_bound
            || output_tokens > self.hold.request.max_output_tokens
        {
            return Err(BudgetError::Unavailable);
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "model_request_budget_tests.rs"]
mod tests;
