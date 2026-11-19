use super::{AdapterError, Bridge, SettingsRequest, CALL_DEADLINE};

#[async_trait::async_trait]
impl crate::ConversationSettingsSupport for Bridge {
    async fn delivery_observation(&self) -> crate::ingress_budget::DeliveryObservation {
        crate::ingress_budget::DeliveryObservation {
            progress: self.output_observation().await,
            history: Some(self.retained_history_deadline.observe()),
        }
    }
    async fn managed_context_may_be_ready(&self) -> bool {
        self.context_readiness_session().await.is_some()
    }
    async fn retained_history_deadline(
        &self,
    ) -> Option<tokio::sync::watch::Receiver<Option<tokio::time::Instant>>> {
        self.retained_history_deadline.legacy_receiver()
    }
    async fn managed_context_progress(&self) -> Option<tokio::sync::watch::Receiver<u64>> {
        self.output_progress().await
    }

    async fn managed_context_receipt(&self, sequence: u64) -> Option<String> {
        self.context_readiness_receipt(sequence, false).await
    }

    async fn managed_transport_receipt(&self, sequence: u64) -> Option<String> {
        self.context_readiness_receipt(sequence, true).await
    }

    async fn system_prompt_supported(&self) -> bool {
        self.instructions_supported().await
    }
}

impl Bridge {
    pub(in crate::hosted) async fn instructions_supported(&self) -> bool {
        use std::sync::atomic::Ordering;
        match self.instructions_support.load(Ordering::Acquire) {
            1 => return true,
            2 => return false,
            _ => {}
        }
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            self.call_prepared(
                "cma/agent-instructions",
                &SettingsRequest::Support { action: "support" },
                CALL_DEADLINE,
            ),
        )
        .await;
        match result {
            Ok(Ok(value)) => {
                let supported = value.get("supported").and_then(serde_json::Value::as_bool)
                    == Some(true)
                    && value.get("version").and_then(serde_json::Value::as_u64) == Some(1);
                self.instructions_support
                    .store(if supported { 1 } else { 2 }, Ordering::Release);
                supported
            }
            Ok(Err(AdapterError::Rejected(_))) => {
                self.instructions_support.store(2, Ordering::Release);
                false
            }
            _ => false,
        }
    }
}
