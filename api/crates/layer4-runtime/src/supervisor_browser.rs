use super::*;
use layer3_adapter::{
    BrowserObservation, BrowserOperation, BrowserReceipt, BrowserRequest, BrowserState,
};

impl Supervisor {
    pub(super) async fn browser(&self, request: BrowserRequest) -> Result<ResponseFrame> {
        if request.sandbox_id != self.config.sandbox_id
            || request.runtime_generation != self.config.generation
            || request.boot_id != self.boot_id
            || crate::paths::safe_component(request.agent_id.as_str(), "agent_id").is_err()
        {
            return Err(RuntimeError::rejected(
                "browser_identity",
                "Browser request names a different runtime",
            ));
        }
        self.require_credentialed().await?;
        let _operation = self.operation_gate.lock().await;
        if self.held_checkpoint.lock().await.is_some() {
            return Ok(ResponseFrame::Browser(BrowserReceipt {
                agent_id: request.agent_id,
                boot_id: self.boot_id.clone(),
                observation: BrowserObservation::idle(BrowserState::Paused),
            }));
        }
        let mut browsers = self.browsers.lock().await;
        if request.operation == BrowserOperation::Enable
            && !browsers.contains_key(&request.agent_id)
        {
            if !self.agents.read().await.contains_key(&request.agent_id) {
                return Err(RuntimeError::rejected(
                    "browser_agent_absent",
                    "Browser enablement requires a running Agent",
                ));
            }
            if !browsers.is_empty() {
                return Err(RuntimeError::rejected(
                    "browser_capacity",
                    "Only one Agent browser may be enabled in this Sandbox",
                ));
            }
            let browser = crate::browser::Browser::start(
                &self.config,
                &self.boot_id,
                request.agent_id.as_str(),
            )
            .await?;
            browsers.insert(request.agent_id.clone(), browser);
        }
        let observation = if let Some(browser) = browsers.get_mut(&request.agent_id) {
            browser.exchange(&request).await?
        } else {
            BrowserObservation::idle(BrowserState::NotEnabled)
        };
        if request.operation == BrowserOperation::Disable {
            browsers.remove(&request.agent_id);
        }
        Ok(ResponseFrame::Browser(BrowserReceipt {
            agent_id: request.agent_id,
            boot_id: self.boot_id.clone(),
            observation,
        }))
    }
}
