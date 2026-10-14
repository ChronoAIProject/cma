//! Model discovery through the owned Agent's admitted model service.

use super::Agents;
use crate::Result;
use serde::Deserialize;

/// A model offered by the Agent's model service.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AgentModel {
    /// Exact model identifier used when updating Agent settings.
    pub id: String,
    /// Human-readable model name.
    pub label: String,
    /// Whether this Agent's current runtime supports selecting the model.
    pub available: bool,
}

/// Current model catalog and per-model runtime availability.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AgentModels {
    /// Models returned by the Agent's admitted model service.
    pub models: Vec<AgentModel>,
}

impl Agents<'_> {
    /// Discover models for an owned Agent, including unavailable runtime choices.
    pub async fn models(&self, id: &str) -> Result<AgentModels> {
        self.client.get(&["agents", id, "models"], &[]).await
    }
}
