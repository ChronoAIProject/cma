//! Agent-selected operations are separate from the user's attested input.
use super::*;
use crate::bots::BotError;
use serde::{Deserialize, Serialize};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetupActionRequest {
    pub request_id: String,
    #[serde(default)]
    pub expected: Option<SetupExpected>,
    pub action: SetupAction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetupExpected {
    pub setup_id: String,
    pub revision: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum SetupAction {
    View,
    Draft {
        draft: SetupDraft,
    },
    Choose {
        choice_id: String,
    },
    Customize {
        customization: Box<SetupCustomization>,
    },
    Confirm {
        plan_digest: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetupActionReceipt {
    pub request_id: String,
    pub digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetupReviewEvent {
    pub event_id: String,
    pub occurred_at: String,
    pub reviewed_at: String,
}

impl SetupActionRequest {
    pub(super) fn validate(&self) -> Result<(), BotError> {
        if self.request_id.is_empty()
            || self.request_id.len() > 128
            || !self
                .request_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
        {
            return Err(BotError::Invalid(
                "Provide a request_id with 1–128 letters, digits, hyphens or underscores.".into(),
            ));
        }
        if !matches!(self.action, SetupAction::View) && self.expected.is_none() {
            return Err(BotError::Invalid(
                "A setup ID and revision are required for this action.".into(),
            ));
        }
        Ok(())
    }

    pub(super) fn digest(&self, event: &SetupEvent) -> Result<String, BotError> {
        crate::profiles::content_digest(&serde_json::json!({
            "event_id": event.event_id, "request": self,
        }))
        .map_err(|_| BotError::Unavailable)
    }

    pub(super) fn replayed(&self, setup: &BotSetup, digest: &str) -> Result<bool, BotError> {
        match setup
            .processed_actions
            .iter()
            .find(|r| r.request_id == self.request_id)
        {
            Some(receipt) if receipt.digest == digest => Ok(true),
            Some(_) => Err(BotError::Conflict),
            None => Ok(false),
        }
    }

    pub(super) fn check_expected(&self, setup: &BotSetup) -> Result<(), BotError> {
        if self.expected.as_ref().is_some_and(|expected| {
            expected.setup_id != setup.id || expected.revision != setup.revision
        }) {
            return Err(BotError::Conflict);
        }
        Ok(())
    }

    pub(super) fn content(&self) -> (SetupDraft, SetupCustomization) {
        match &self.action {
            SetupAction::Draft { draft } => (draft.clone(), SetupCustomization::default()),
            SetupAction::Customize { customization } => {
                (SetupDraft::default(), *customization.clone())
            }
            _ => (SetupDraft::default(), SetupCustomization::default()),
        }
    }

    pub(super) fn choice(&self, setup: &BotSetup) -> Result<Option<String>, BotError> {
        match &self.action {
            SetupAction::Choose { choice_id } => {
                // Provisioning is available only through the consent-checking action.
                if choice_id.is_empty() || choice_id.len() > 512 || choice_id.starts_with("create")
                {
                    return Err(super::helpers::invalid_action());
                }
                Ok(Some(choice_id.clone()))
            }
            SetupAction::Confirm { plan_digest } => {
                if setup.step != SetupStep::Review
                    || setup.confirmation_digest.as_deref() != Some(plan_digest)
                    || setup.plan_digest()? != *plan_digest
                {
                    return Err(BotError::Conflict);
                }
                Ok(Some(format!("create:{plan_digest}")))
            }
            _ => Ok(None),
        }
    }

    pub(super) fn apply_content(
        &self,
        setup: &mut BotSetup,
        draft: &SetupDraft,
        customization: &SetupCustomization,
    ) -> Result<(), BotError> {
        match &self.action {
            SetupAction::View => Ok(()),
            SetupAction::Draft { .. }
                if matches!(
                    setup.step,
                    SetupStep::Login | SetupStep::Owner | SetupStep::Name
                ) =>
            {
                super::helpers::apply_draft(setup, draft)?;
                if setup.step == SetupStep::Name && setup.draft.name.is_some() {
                    setup.step = SetupStep::Agent;
                }
                Ok(())
            }
            SetupAction::Customize { .. } => {
                match setup.step {
                    SetupStep::AgentCustom
                        if customization.workspace.is_none()
                            && customization.sandbox.is_none()
                            && customization.availability.is_none() =>
                    {
                        setup.configuration.agent = customization
                            .agent
                            .clone()
                            .ok_or_else(super::helpers::invalid_action)?;
                        setup.step = SetupStep::Workspace;
                    }
                    SetupStep::WorkspaceCustom
                        if customization.agent.is_none()
                            && customization.sandbox.is_none()
                            && customization.availability.is_none() =>
                    {
                        setup.configuration.workspace = customization
                            .workspace
                            .clone()
                            .ok_or_else(super::helpers::invalid_action)?;
                        setup.step = SetupStep::Sandbox;
                    }
                    SetupStep::SandboxCustom
                        if customization.agent.is_none() && customization.workspace.is_none() =>
                    {
                        setup.configuration.sandbox = customization
                            .sandbox
                            .clone()
                            .ok_or_else(super::helpers::invalid_action)?;
                        if let Some(availability) = customization.availability {
                            setup.configuration.availability = availability;
                        }
                        setup.selected_services.clear();
                        setup.step = SetupStep::Services;
                    }
                    _ => return Err(super::helpers::invalid_action()),
                }
                Ok(())
            }
            _ => Err(super::helpers::invalid_action()),
        }
    }
}

pub(super) fn require_consent(
    setup: &BotSetup,
    event: &SetupEvent,
    digest: &str,
) -> Result<(), BotError> {
    if setup.step != SetupStep::Review
        || setup.confirmation_digest.as_deref() != Some(digest)
        || setup.plan_digest()? != digest
    {
        return Err(BotError::Conflict);
    }
    let review = setup
        .confirmation_event
        .as_ref()
        .ok_or(BotError::Conflict)?;
    let occurred =
        OffsetDateTime::parse(&event.occurred_at, &Rfc3339).map_err(|_| BotError::Forbidden)?;
    let reviewed =
        OffsetDateTime::parse(&review.reviewed_at, &Rfc3339).map_err(|_| BotError::Forbidden)?;
    let source =
        OffsetDateTime::parse(&review.occurred_at, &Rfc3339).map_err(|_| BotError::Forbidden)?;
    if event.event_id == review.event_id || occurred <= reviewed || occurred <= source {
        return Err(BotError::Forbidden);
    }
    match &event.input {
        SetupInput::Interaction {
            block_id,
            action_id,
        } if block_id == &setup.block_id() && action_id == &format!("create:{digest}") => Ok(()),
        SetupInput::Text(text) if explicit_consent(text) => Ok(()),
        _ => Err(BotError::Forbidden),
    }
}

fn explicit_consent(text: &str) -> bool {
    matches!(
        text.trim().to_lowercase().as_str(),
        "create"
            | "create bot"
            | "confirm create"
            | "confirm creation"
            | "创建"
            | "创建bot"
            | "创建 bot"
            | "确认创建"
            | "确认创建bot"
            | "确认创建 bot"
            | "确认创建机器人"
    )
}
