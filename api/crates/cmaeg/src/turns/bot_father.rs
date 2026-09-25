//! Narrow CMA handoff for the dedicated Bot Father binding.
//!
//! Bot setup is a CMA business workflow. CMAEG owns the verified event
//! reference and Telegram presentation, so the provider model never receives
//! credentials or has to imitate the setup protocol.

use cmaeg_core::{
    activity::ActivityKind,
    channel::Secret,
    message::{Block, ChoiceOption, Field, OptionStyle, OutboundMessage},
    seal::EventRef,
};
use reqwest::{Client, StatusCode, Url};
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Duration;

const BINDING_ENV: &str = "CMAEG_BOT_FATHER_BINDING_ID";

#[derive(Clone)]
pub struct BotFatherClient {
    http: Client,
    endpoint: Url,
    binding_id: String,
}

impl BotFatherClient {
    pub fn from_env(cma_base_url: &str) -> Result<Option<Self>, String> {
        let Some(binding_id) = std::env::var(BINDING_ENV)
            .ok()
            .filter(|value| !value.trim().is_empty())
        else {
            return Ok(None);
        };
        if !valid_binding(&binding_id) {
            return Err(format!("{BINDING_ENV} is invalid"));
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

    pub async fn advance(
        &self,
        credential: &Secret,
        event_ref: &EventRef,
        activity: &cmaeg_core::activity::Activity,
    ) -> Result<OutboundMessage, String> {
        let body = json!({
            "event_ref": event_ref.as_str(),
            "draft": draft(activity),
            "customization": {},
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

fn draft(activity: &cmaeg_core::activity::Activity) -> Value {
    let ActivityKind::Message { text, .. } = &activity.kind else {
        return json!({});
    };
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.starts_with("link_") || trimmed.starts_with("/start link_") {
        return json!({});
    }
    let lower = trimmed.to_ascii_lowercase();
    let name = if let Some(start) = trimmed.find("叫 ") {
        let value = &trimmed[start + "叫 ".len()..];
        value
            .split(" 的 Bot")
            .next()
            .or_else(|| value.split("的 Bot").next())
            .map(str::trim)
            .filter(|value| !value.is_empty())
    } else if let Some(start) = lower.find("called ") {
        let value = &trimmed[start + "called ".len()..];
        value
            .split(|character: char| character == '.' || character == ',' || character == ':')
            .next()
            .map(str::trim)
            .filter(|value| !value.is_empty())
    } else {
        None
    };
    let instructions = [
        "Agent instructions：",
        "Agent instructions:",
        "instructions:",
    ]
    .iter()
    .find_map(|marker| {
        trimmed
            .find(marker)
            .map(|index| &trimmed[index + marker.len()..])
    })
    .map(str::trim)
    .filter(|value| !value.is_empty());
    if name.is_none() && instructions.is_none() && trimmed.len() <= 80 {
        return json!({"name": trimmed});
    }
    json!({
        "name": name,
        "instructions": instructions,
    })
}

#[derive(Debug, Deserialize)]
struct SetupView {
    step: String,
    interaction_id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    choices: Vec<SetupChoice>,
    #[serde(default)]
    connection_url: Option<String>,
    #[serde(default)]
    notice: Option<String>,
    #[serde(default)]
    identity: SetupIdentity,
    #[serde(default)]
    result: Option<SetupResult>,
}
#[derive(Debug, Deserialize)]
struct SetupChoice {
    id: String,
    label: String,
    #[serde(default)]
    selected: bool,
}
#[derive(Debug, Default, Deserialize)]
struct SetupIdentity {
    #[serde(default)]
    login_url: Option<String>,
}
#[derive(Debug, Deserialize)]
struct SetupResult {
    #[serde(default)]
    state: String,
    #[serde(default)]
    operation: Option<String>,
    #[serde(default)]
    failure: Option<String>,
    #[serde(default)]
    bot_url: Option<String>,
}

impl SetupView {
    fn message(self) -> Result<OutboundMessage, String> {
        let title = match self.step.as_str() {
            "login" => "Connect Bot Father to NyxID",
            "owner" => "Who should own this Bot?",
            "name" => "Name your Bot",
            "agent" => "Agent Profile",
            "workspace" => "Workspace Profile",
            "sandbox" => "Sandbox Profile",
            "services" => "Connect services",
            "external_bot" => "Choose the Telegram Bot",
            "admission" => "Who may use the Bot?",
            "review" => "Review your Bot",
            "provisioning" => "Creating your Bot",
            "complete" => "Your Bot is ready",
            "cancelled" => "Bot creation cancelled",
            _ => "CMA Bot Father",
        };
        let mut body = Vec::new();
        if let Some(name) = &self.name {
            body.push(format!("Bot: {name}"));
        }
        if let Some(notice) = self.notice {
            body.push(notice);
        }
        let bot_url = self
            .result
            .as_ref()
            .and_then(|result| result.bot_url.clone());
        if let Some(result) = self.result.as_ref() {
            body.push(format!("Status: {}", result.state));
            if let Some(operation) = &result.operation {
                body.push(format!("Working on: {operation}"));
            }
            if let Some(failure) = &result.failure {
                body.push(failure.clone());
            }
        }
        let mut blocks = vec![Block::Card {
            title: title.into(),
            body: (!body.is_empty()).then_some(body.join("\n")),
            fields: vec![Field {
                label: "Step".into(),
                value: self.step.clone(),
            }],
            image: None,
        }];
        if !self.choices.is_empty() {
            blocks.push(Block::Choice {
                id: self.interaction_id,
                prompt: "Choose an option:".into(),
                options: self
                    .choices
                    .into_iter()
                    .map(|choice| {
                        let id = choice.id;
                        let style = if id == "cancel" {
                            OptionStyle::Danger
                        } else if id.starts_with("create:") {
                            OptionStyle::Primary
                        } else {
                            OptionStyle::Default
                        };
                        ChoiceOption {
                            id,
                            label: choice.label,
                            style,
                            selected: choice.selected,
                        }
                    })
                    .collect(),
                multi: self.step == "services",
            });
        }
        if let Some(url) = self.connection_url.or(self.identity.login_url).or(bot_url) {
            let label = if self.step == "login" {
                "Sign in to NyxID"
            } else if self.step == "complete" {
                "Open your Bot"
            } else {
                "Continue in NyxID"
            };
            blocks.push(Block::Link {
                url,
                label: Some(label.into()),
            });
        }
        Ok(OutboundMessage {
            text: Some(
                "CMA Bot Father will create the resources only after you confirm Create Bot."
                    .into(),
            ),
            blocks,
            reply_to: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::draft;
    use cmaeg_core::activity::{
        Activity, ActivityConversation, ActivityKind, Actor, ActorKind, ConversationKind,
        SourceKind,
    };
    use cmaeg_core::channel::Platform;
    use cmaeg_core::ids::EventId;
    use serde_json::json;

    fn activity(text: &str) -> Activity {
        Activity {
            schema_version: 1,
            event_id: EventId::new("event").unwrap(),
            kind: ActivityKind::Message {
                text: text.into(),
                attachments: vec![],
                mentions_bot: false,
                reply_to: None,
            },
            actor: Actor {
                id: "actor".into(),
                display_name: None,
                kind: ActorKind::Human,
            },
            conversation: ActivityConversation {
                id: "chat".into(),
                kind: ConversationKind::Private,
                thread_id: None,
            },
            occurred_at: "2026-01-01T00:00:00Z".into(),
            source: SourceKind::NyxidRelay {
                platform: Platform::Telegram,
                route_id: "route".into(),
            },
        }
    }

    #[test]
    fn initial_request_extracts_name_and_instructions_without_copying_login_code() {
        let value = draft(&activity(
            "帮我创建一个叫 Paper assistant 的 Bot。Agent instructions：Explain papers.",
        ));
        assert_eq!(value["name"], "Paper assistant");
        assert_eq!(value["instructions"], "Explain papers.");
        assert_eq!(draft(&activity("link_abc")), json!({}));
    }
}
