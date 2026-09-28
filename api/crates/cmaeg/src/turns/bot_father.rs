//! Narrow CMA handoff for the dedicated Bot Father binding.
//!
//! Bot setup is a CMA business workflow. CMAEG owns the verified event
//! reference and Telegram presentation, so the provider model never receives
//! credentials or has to imitate the setup protocol.

use cmaeg_core::{channel::Secret, message::OutboundMessage, seal::EventRef};
use reqwest::{Client, StatusCode, Url};
use serde_json::json;
use std::time::Duration;

use crate::config::CMAEG_BOT_FATHER_BINDING_ID;

mod input;
#[cfg(test)]
mod tests;
mod view;

use view::SetupView;

#[derive(Clone)]
pub struct BotFatherClient {
    http: Client,
    endpoint: Url,
    binding_id: String,
}

impl BotFatherClient {
    pub fn from_env(cma_base_url: &str) -> Result<Option<Self>, String> {
        let Some(binding_id) = std::env::var(CMAEG_BOT_FATHER_BINDING_ID)
            .ok()
            .filter(|value| !value.trim().is_empty())
        else {
            return Ok(None);
        };
        if !valid_binding(&binding_id) {
            return Err(format!("{CMAEG_BOT_FATHER_BINDING_ID} is invalid"));
        }
        let mut endpoint = Url::parse(cma_base_url)
            .map_err(|_| "invalid CMA origin for Bot Father setup".to_owned())?;
        endpoint.set_path("/api/v1/bot-setups/advance");
        endpoint.set_query(None);
        endpoint.set_fragment(None);
        let http = Client::builder()
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(20))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent("cmaeg-bot-father/1")
            .build()
            .map_err(|_| "Bot Father HTTP client configuration failed".to_owned())?;
        Ok(Some(Self {
            http,
            endpoint,
            binding_id,
        }))
    }

    pub fn matches(&self, binding_id: &str) -> bool {
        self.binding_id == binding_id
    }

    pub fn handles_setup_activity(&self, activity: &cmaeg_core::activity::Activity) -> bool {
        input::is_setup_activity(activity)
    }

    pub async fn advance(
        &self,
        credential: &Secret,
        event_ref: &EventRef,
        activity: &cmaeg_core::activity::Activity,
    ) -> Result<OutboundMessage, String> {
        let (draft, customization) = input::request(activity)?;
        let body = json!({
            "event_ref": event_ref.as_str(),
            "activity": activity,
            "draft": draft,
            "customization": customization,
        });
        let response = self
            .http
            .post(self.endpoint.clone())
            .bearer_auth(credential.as_str())
            .json(&body)
            .send()
            .await
            .map_err(|_| "Bot Father setup service is temporarily unavailable.".to_owned())?;
        let status = response.status();
        let bytes = response
            .bytes()
            .await
            .map_err(|_| "Bot Father setup service returned no response.".to_owned())?;
        if !status.is_success() {
            return Err(match status {
                StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
                    "Bot Father could not verify this Telegram conversation. Start again in the private chat.".into()
                }
                StatusCode::CONFLICT => {
                    "That Bot Father choice has expired. Please use the newest card.".into()
                }
                StatusCode::BAD_REQUEST => {
                    let detail = serde_json::from_slice::<serde_json::Value>(&bytes).ok()
                        .filter(|problem| problem["code"] == "invalid_setup")
                        .and_then(|problem| problem["detail"].as_str().map(str::to_owned))
                        .filter(|detail| detail.len() <= 1_024);
                    format!("CMA could not accept this configuration. {} Reply with a corrected value, or `back` to return to the profile choice.", detail.as_deref().unwrap_or("Check the fields in the current configuration template."))
                }
                StatusCode::UNPROCESSABLE_ENTITY => {
                    "The configuration fields do not match the template. Keep the required fields and use the shown field types and values, then send the corrected JSON.".into()
                }
                _ => "Bot Father could not continue right now. Please try again.".into(),
            });
        }
        let view: SetupView = serde_json::from_slice(&bytes)
            .map_err(|_| "Bot Father returned an invalid setup view.".to_owned())?;
        view.message()
    }
}

fn valid_binding(value: &str) -> bool {
    value.starts_with("bnd_")
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}
