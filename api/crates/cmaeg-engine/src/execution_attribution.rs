use serde::{Deserialize, Serialize};

/// Stored inside the sealed channel/event/consumer row. Only the reference
/// for the currently presented provider response is projected publicly.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExecutionAttribution {
    provider: String,
    response_id: String,
    reference: String,
}

impl ExecutionAttribution {
    pub fn new(provider: String, response_id: String, reference: String) -> Option<Self> {
        if provider.is_empty()
            || provider.len() > 128
            || response_id.is_empty()
            || response_id.len() > 512
            || !valid_reference(&reference)
        {
            return None;
        }
        Some(Self {
            provider,
            response_id,
            reference,
        })
    }

    pub fn retain(&self, retained: &mut Vec<Self>) {
        if retained.len() < 4
            && !retained
                .iter()
                .any(|value| value.matches(&self.provider, &self.response_id))
        {
            retained.push(self.clone());
        }
    }

    pub fn matches(&self, provider: &str, response_id: &str) -> bool {
        self.provider == provider && self.response_id == response_id
    }

    pub fn public_reference(&self) -> Option<String> {
        valid_reference(&self.reference).then(|| self.reference.clone())
    }
}

fn valid_reference(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
}
