use cmaeg_core::message::{Block, Field, OutboundMessage, MAX_MESSAGE_TEXT_BYTES};
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Debug, Deserialize)]
pub(super) struct SetupView {
    step: String,
    configuration: Value,
    #[serde(default)]
    owner: Option<Value>,
    #[serde(default)]
    services: Vec<SetupService>,
    #[serde(default)]
    external_bot_id: Option<String>,
    #[serde(default)]
    admission: Option<Value>,
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
struct SetupService {
    id: String,
    slug: String,
    name: String,
    connected: bool,
    selected: bool,
    required: bool,
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
    pub(super) fn message(self) -> Result<OutboundMessage, String> {
        let title = match self.step.as_str() {
            "login" => "Connect Bot Father to NyxID",
            "owner" => "Who should own this Bot?",
            "name" => "Name your Bot (reply `/setup name: Your Bot name`)",
            "agent" => "Agent Profile",
            "agent_custom" => "Customize your Agent Profile",
            "workspace" => "Workspace Profile",
            "workspace_custom" => "Customize your Workspace Profile",
            "sandbox" => "Sandbox Profile",
            "sandbox_custom" => "Customize your Sandbox Profile",
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
        if let Some(notice) = &self.notice {
            body.push(notice.clone());
        }
        if self.step == "name" {
            body.push("Reply `/setup name: Your Bot name` to continue. Ordinary questions still go to your Bot Father assistant.".into());
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
        self.configuration_blocks(&mut blocks)?;
        if !self.choices.is_empty() {
            let mut lines = vec![
                "Send the /setup command beside your choice. You can keep chatting with Bot Father between setup steps.".to_owned(),
            ];
            for choice in self.choices {
                let command = choice
                    .id
                    .strip_prefix("create:")
                    .map(|_| "create")
                    .unwrap_or(choice.id.as_str());
                let selected = if choice.selected { " (selected)" } else { "" };
                lines.push(format!(
                    "- {}{} — `/setup {command}`",
                    choice.label, selected
                ));
            }
            if self.step == "services" {
                lines.push(
                    "You can repeat a service command to toggle it, then reply `/setup done`."
                        .into(),
                );
            }
            blocks.push(Block::Text {
                text: lines.join("\n"),
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
            // Existing relays support plain text and links without callback
            // support; keep the URL visible even if capabilities advertise buttons.
            blocks.push(Block::Text {
                text: format!("{label}: {url}"),
            });
        }
        let message = OutboundMessage {
            text: Some(
                "CMA Bot Father will create the resources only after you confirm Create Bot."
                    .into(),
            ),
            blocks,
            reply_to: None,
        };
        message.validate().map_err(|_| {
            "This configuration is too large to show for confirmation. Reply `/setup edit` and shorten the profile contents before creating the Bot.".to_owned()
        })?;
        Ok(message)
    }

    fn configuration_blocks(&self, blocks: &mut Vec<Block>) -> Result<(), String> {
        let edits = match self.step.as_str() {
            "agent_custom" => Some("To change instructions, reply `/setup instructions: Explain research papers clearly.` To change the model, reply `/setup model: MODEL_ID` or `/setup model: default`. You can send several edits, then reply `/setup done`."),
            "workspace_custom" => Some("To clone a repository, reply `/setup repository: https://github.com/owner/repo` (optionally followed by a branch or tag). For an empty directory, reply `/setup directory: work`. Reply `/setup done` after editing."),
            "sandbox_custom" => Some("To change resources, reply `/setup resources: 2000 4096` for 2 CPU cores and 4096 MiB memory, then `/setup done`. You can also choose Start on demand or Keep always live below."),
            _ => None,
        };
        if let Some(edits) = edits {
            push_text(blocks, edits.into());
        }
        let (key, guidance) = match self.step.as_str() {
            "agent_custom" => ("agent", "Edit the JSON below and send `/setup ` followed by the complete object as one message. Agent fields: model (or null for the default), thinking_effort (minimal, low, medium, high, x_high), permission_level (guarded or autonomous), skills (Ornn source/id/version references), and system_prompt (your Bot's instructions)."),
            "workspace_custom" => ("workspace", "Edit the JSON below and send `/setup ` followed by the complete object as one message. Workspace fields: name, location, setup_commands. Use location {\"type\":\"directory\",\"name\":\"work\"} or {\"type\":\"git_repository\",\"url\":\"https://github.com/owner/repo\",\"reference\":\"main\"}. Keep agent_profiles empty; the Agent Profile from this setup is attached automatically."),
            "sandbox_custom" => ("sandbox", "Edit the JSON below and send `/setup ` followed by the complete object as one message. Sandbox fields: cpu_millis, memory_mib, agent_provider, max_workspaces, max_agents_per_workspace, software_install_commands, environment_variables (name/value entries), plus availability.always_keep_alive. Keep model_access as chronoai_managed, workspace_profiles empty, and warmup_enabled false. Choose service access in the next step; nyxid_services does not bypass that selection."),
            "review" => return self.review_blocks(blocks),
            _ => return Ok(()),
        };
        push_text(blocks, format!("Advanced configuration: {guidance}"));
        let mut template = json!({(key): self.configuration.get(key)});
        if key == "sandbox" {
            template["availability"] = self.configuration["availability"].clone();
        }
        push_text(blocks, format!("```json\n{}\n```", pretty(&template)?));
        push_text(blocks, "CMA validates this configuration before continuing. You can also use a shortcut below, or reply `/setup back` to return to the default/custom choice.".into());
        Ok(())
    }

    fn review_blocks(&self, blocks: &mut Vec<Block>) -> Result<(), String> {
        let owner = self
            .owner
            .as_ref()
            .ok_or("Review is missing the selected owner.")?;
        let bot = self
            .external_bot_id
            .as_ref()
            .ok_or("Review is missing the selected Telegram Bot.")?;
        let admission = self
            .admission
            .as_ref()
            .ok_or("Review is missing the admission policy.")?;
        let owner_kind = owner.get("kind").and_then(Value::as_str).unwrap_or("owner");
        let owner_id = owner
            .get("subject")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let services = self
            .services
            .iter()
            .filter(|service| service.selected)
            .map(|service| {
                format!(
                    "- {} ({}, ID: {}) — {}{}",
                    service.name,
                    service.slug,
                    service.id,
                    if service.connected {
                        "connected"
                    } else {
                        "connection required"
                    },
                    if service.required { "; required" } else { "" }
                )
            })
            .collect::<Vec<_>>();
        push_text(blocks, format!(
            "Selected owner: {owner_kind} {owner_id}\nTelegram channel Bot: {bot}\nSelected services:\n{}\nAdmission policy:\n{}",
            if services.is_empty() { "(none)".into() } else { services.join("\n") },
            pretty(admission)?,
        ));
        for (key, label) in [
            ("agent", "Agent Profile"),
            ("workspace", "Workspace Profile"),
            ("sandbox", "Sandbox Profile"),
            ("availability", "Sandbox availability"),
        ] {
            let content = self
                .configuration
                .get(key)
                .ok_or("Review is missing a profile configuration.")?;
            push_text(
                blocks,
                format!("{label} — selected configuration:\n{}", pretty(content)?),
            );
        }
        push_text(blocks, "Check the owner, Bot, service access, admission rules and all profile settings above. Reply `/setup create` to approve this complete plan, or `/setup edit` to change it.".into());
        Ok(())
    }
}

fn pretty(value: &Value) -> Result<String, String> {
    serde_json::to_string_pretty(value)
        .map_err(|_| "Bot Father could not render the configuration.".into())
}

/// Preserve the entire configuration, including prompts near the profile limit.
/// Each block has its own byte bound; split on a valid UTF-8 boundary.
fn push_text(blocks: &mut Vec<Block>, text: String) {
    let mut remaining = text.as_str();
    while !remaining.is_empty() {
        let mut end = remaining.len().min(MAX_MESSAGE_TEXT_BYTES);
        while !remaining.is_char_boundary(end) {
            end -= 1;
        }
        blocks.push(Block::Text {
            text: remaining[..end].to_owned(),
        });
        remaining = &remaining[end..];
    }
}
