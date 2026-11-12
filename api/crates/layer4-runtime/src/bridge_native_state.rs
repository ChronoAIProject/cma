//! Correlated ACP observations; requested identity alone is never native proof.
use layer3_adapter::agent_capacity_recovery::ConfirmedAgentState;
use layer3_adapter::{PromptIdentity, PromptResponse, RequestPermissionRequest};
use serde_json::Value;

#[derive(Default)]
pub(in crate::bridge) struct NativeObservation {
    pub conversation_id: Option<String>,
    pub native_state: Option<ConfirmedAgentState>,
    pending_open: Option<(Value, Option<String>)>,
    approval: Option<(Value, Vec<String>)>,
}

impl NativeObservation {
    pub fn invalidate(&mut self) {
        tracing::debug!("native conversation observation is unconfirmed");
        self.conversation_id = None;
        self.native_state = None;
        self.pending_open = None;
        self.approval = None;
    }

    pub fn input(&mut self, frame: &[u8]) {
        let Ok(value) = serde_json::from_slice::<Value>(frame) else {
            self.invalidate();
            return;
        };
        match value.get("method").and_then(Value::as_str) {
            Some("session/load" | "session/new") => {
                let overlapping = self.pending_open.is_some();
                self.invalidate();
                if overlapping {
                    return;
                }
                let Some(id) = value.get("id").filter(|id| valid_call(id)) else {
                    return;
                };
                let session = if value["method"] == "session/load" {
                    let Some(session) = session(&value) else {
                        return;
                    };
                    Some(session.to_owned())
                } else {
                    None
                };
                self.pending_open = Some((id.clone(), session));
            }
            Some("session/prompt") => {
                let prompt = value
                    .get("id")
                    .and_then(Value::as_str)
                    .and_then(PromptIdentity::parse);
                // Overlapping or foreign prompts make the native state uncertain.
                if session(&value) != self.conversation_id.as_deref()
                    || self.conversation_id.is_none()
                    || self.native_state != Some(ConfirmedAgentState::Idle)
                {
                    self.invalidate();
                    return;
                }
                self.native_state = prompt.map(|prompt| ConfirmedAgentState::Working {
                    prompt_id: prompt.as_str().into(),
                });
            }
            Some("session/close" | "session/cancel") => self.invalidate(),
            None => self.answer_permission(&value),
            _ => {}
        }
    }

    pub fn output(&mut self, frame: &[u8]) {
        let Ok(value) = serde_json::from_slice::<Value>(frame) else {
            self.invalidate();
            return;
        };
        if value.get("method").is_some() {
            if value["method"] == "session/request_permission" {
                self.permission(&value);
            }
            return;
        }
        if self
            .pending_open
            .as_ref()
            .is_some_and(|(id, _)| value.get("id") == Some(id))
        {
            let (_, requested) = self.pending_open.take().expect("correlated open");
            let result = value.get("result").filter(|result| result.is_object());
            if value.get("error").is_some()
                || result.is_none()
                || value
                    .pointer("/result/sessionId")
                    .is_some_and(|id| !id.is_string())
            {
                self.invalidate();
                return;
            }
            let returned = value.pointer("/result/sessionId").and_then(Value::as_str);
            let confirmed = match (requested, returned) {
                (Some(requested), None) => Some(requested),
                (Some(requested), Some(returned)) if requested == returned => Some(requested),
                (None, Some(returned)) if !returned.is_empty() => Some(returned.to_owned()),
                _ => None,
            };
            self.native_state = confirmed.as_ref().map(|_| ConfirmedAgentState::Idle);
            self.conversation_id = confirmed;
            return;
        }
        let prompt = match &self.native_state {
            Some(
                ConfirmedAgentState::Working { prompt_id }
                | ConfirmedAgentState::WaitingApproval { prompt_id, .. },
            ) => prompt_id,
            _ => return,
        };
        if value.get("id").and_then(Value::as_str) != Some(prompt.as_str()) {
            return;
        }
        // Errors and malformed terminal results cannot establish native idleness.
        let terminal = value.get("error").is_none()
            && value.get("result").is_some_and(|result| {
                serde_json::from_value::<PromptResponse>(result.clone()).is_ok()
            });
        self.native_state = terminal.then_some(ConfirmedAgentState::Idle);
        self.approval = None;
    }

    fn permission(&mut self, value: &Value) {
        let parsed = value
            .get("params")
            .cloned()
            .and_then(|params| serde_json::from_value::<RequestPermissionRequest>(params).ok());
        let Some(request) = parsed else {
            self.invalidate();
            return;
        };
        let Some(call) = value.get("id").filter(|id| valid_call(id)) else {
            self.invalidate();
            return;
        };
        if self.conversation_id.as_deref() != Some(request.session_id.0.as_ref()) {
            self.invalidate();
            return;
        }
        let Some(approval_id) = layer3_adapter::hosted::observed_permission_id(&request, call)
        else {
            self.invalidate();
            return;
        };
        let prompt_id = match &self.native_state {
            Some(ConfirmedAgentState::Working { prompt_id }) => prompt_id.clone(),
            Some(ConfirmedAgentState::WaitingApproval {
                prompt_id,
                approval_id: current,
            }) if current == &approval_id => prompt_id.clone(),
            _ => {
                self.invalidate();
                return;
            }
        };
        self.approval = Some((
            call.clone(),
            request
                .options
                .iter()
                .map(|option| option.option_id.0.to_string())
                .collect(),
        ));
        self.native_state = Some(ConfirmedAgentState::WaitingApproval {
            prompt_id,
            approval_id,
        });
    }

    fn answer_permission(&mut self, value: &Value) {
        let Some((call, options)) = &self.approval else {
            return;
        };
        if value.get("id") != Some(call) {
            return;
        }
        let valid = value.get("error").is_none()
            && match value
                .pointer("/result/outcome/outcome")
                .and_then(Value::as_str)
            {
                Some("cancelled") => true,
                Some("selected") => value
                    .pointer("/result/outcome/optionId")
                    .and_then(Value::as_str)
                    .is_some_and(|id| options.iter().any(|option| option == id)),
                _ => false,
            };
        self.native_state = match self.native_state.take() {
            Some(ConfirmedAgentState::WaitingApproval { prompt_id, .. }) if valid => {
                Some(ConfirmedAgentState::Working { prompt_id })
            }
            _ => None,
        };
        self.approval = None;
    }
}

fn valid_call(value: &Value) -> bool {
    matches!(value, Value::String(_) | Value::Number(_))
}

fn session(value: &Value) -> Option<&str> {
    value
        .pointer("/params/sessionId")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
}

#[cfg(test)]
#[path = "bridge_native_state_tests.rs"]
mod tests;
