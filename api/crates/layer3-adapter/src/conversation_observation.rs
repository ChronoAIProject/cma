//! Provider-neutral observations delivered through the ordered conversation journal.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationDescriptionSource {
    ProviderTitle,
    PromptPreview,
}

/// Exact input evidence, consumed by the domain without persisting another transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConversationDescriptionInput {
    Current(String),
    First(String),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConversationMetadata {
    pub working_directory: Option<String>,
    pub description: Option<String>,
    pub description_source: Option<ConversationDescriptionSource>,
    pub description_input: Option<ConversationDescriptionInput>,
}

/// Actual provider counters, never estimates based on content or context size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationUsage {
    pub turn_id: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub last_input_tokens: u64,
    pub last_output_tokens: u64,
    pub context_used: u64,
    pub context_window: Option<u64>,
    /// An explicit context-full sentinel reset the provider's consumed counters.
    pub counters_reset: bool,
}
