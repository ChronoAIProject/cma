use crate::ingress_budget::diagnostic::text;
use std::sync::Arc;

use crate::{AgentSlot, RuntimeCredential, RuntimeHandle};

/// Short-lived native access authority. Renewable OAuth secrets stay in CMA.
#[derive(Debug, Clone)]
pub enum HostedSubscriptionCredential {
    Warm,
    Connected {
        access_token: RuntimeCredential,
        account_id: String,
        plan_type: Option<String>,
    },
}

#[async_trait::async_trait]
pub trait HostedSubscriptionSource: Send + Sync {
    async fn credential(
        &self,
        sandbox_id: &str,
        runtime: &RuntimeHandle,
        agent: &AgentSlot,
        force_refresh: bool,
        previous_account: Option<&str>,
    ) -> crate::Result<HostedSubscriptionCredential>;
}

#[derive(Clone)]
pub(super) struct HostedSubscriptionBinding {
    pub source: Arc<dyn HostedSubscriptionSource>,
    pub sandbox_id: String,
    pub runtime: RuntimeHandle,
    pub agent: AgentSlot,
}

impl HostedSubscriptionBinding {
    pub(super) async fn credential(
        &self,
        params: &serde_json::Value,
    ) -> crate::Result<serde_json::Value> {
        let force_refresh = params
            .get("forceRefresh")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let previous = params
            .get("previousAccountId")
            .and_then(serde_json::Value::as_str);
        if previous.is_some_and(|account| account.is_empty() || account.len() > 256) {
            return Err(crate::AdapterError::Rejected(text(
                "invalid subscription account boundary",
            )));
        }
        let credential = self
            .source
            .credential(
                &self.sandbox_id,
                &self.runtime,
                &self.agent,
                force_refresh,
                previous,
            )
            .await?;
        Ok(match credential {
            HostedSubscriptionCredential::Warm if !force_refresh && previous.is_none() => {
                serde_json::json!({"mode":"warm"})
            }
            HostedSubscriptionCredential::Warm => {
                return Err(crate::AdapterError::Rejected(text(
                    "warm sandbox has no user subscription",
                )))
            }
            HostedSubscriptionCredential::Connected {
                access_token,
                account_id,
                plan_type,
            } => serde_json::json!({
                "mode":"subscription",
                "tokens":{"accessToken":access_token.expose(), "chatgptAccountId":account_id, "chatgptPlanType":plan_type}
            }),
        })
    }
}
