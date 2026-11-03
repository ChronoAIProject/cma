#![doc = "Saved contextual follow-ups on a canonical final answer."]
use super::Agents;
use crate::{Error, Result, answer_context::AnswerContext};
use serde::Deserialize;

#[doc = "An asynchronous saved generation state; reads never generate or send input."]
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SuggestionReading {
    #[doc = "pending, running, ready, empty, failed, expired or unavailable."]
    pub status: String,
    #[doc = "Present only for a current, ready answer."]
    pub set: Option<SuggestedReplySet>,
    #[doc = "Bounded generation observations, with unknown cost represented as null."]
    pub observation: Option<SuggestionObservation>,
}
#[doc = "Stable suggestion identity shared with normal conditional input."]
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SuggestedReplySet {
    #[doc = "Saved set identifier."]
    pub id: String,
    #[doc = "Authenticated owner of this set."]
    pub viewer_id: String,
    #[doc = "Agent owning the answer."]
    pub agent_id: String,
    #[doc = "Canonical event:<sequence> message identity."]
    pub answer_id: String,
    #[doc = "Condition to preserve on normal sends."]
    pub scope: AnswerContext,
    #[doc = "Generation origin and immutable strategy version."]
    pub source: SuggestionSource,
    #[doc = "One to three ordered choices."]
    pub items: Vec<SuggestedReply>,
}
#[doc = "Suggestion origin."]
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SuggestionSource {
    #[doc = "generated or template."]
    pub kind: String,
    #[doc = "Immutable generation strategy version."]
    pub version: String,
}
#[doc = "Exact text eligible for ordinary conditional sending or editing."]
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SuggestedReply {
    #[doc = "Stable item identifier."]
    pub id: String,
    #[doc = "Exact message text, without trimming."]
    pub text: String,
    #[doc = "Ascending rank within the saved set."]
    pub rank: u32,
}
#[doc = "Recorded model work, independent of a user Agent turn."]
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SuggestionObservation {
    #[doc = "Elapsed wall-clock milliseconds."]
    pub elapsed_ms: u64,
    #[doc = "Provider-reported usage; missing values are unknown."]
    pub usage: SuggestionUsage,
    #[doc = "Sanitized terminal failure when generation did not succeed."]
    pub failure: Option<String>,
}
#[doc = "Model usage without inferred billing prices."]
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SuggestionUsage {
    #[doc = "Reported model identity."]
    pub model: Option<String>,
    #[doc = "Reported input tokens."]
    pub input_tokens: Option<u64>,
    #[doc = "Reported cached input tokens."]
    pub cached_input_tokens: Option<u64>,
    #[doc = "Reported output tokens."]
    pub output_tokens: Option<u64>,
    #[doc = "Reported cost, or unknown when absent."]
    pub cost: Option<SuggestionCost>,
}
#[doc = "An amount explicitly reported by the model service."]
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SuggestionCost {
    #[doc = "Decimal amount without floating-point conversion."]
    pub amount: String,
    #[doc = "Currency of the reported amount."]
    pub currency: String,
}
impl Agents<'_> {
    #[doc = "Read a saved result; never admits input or starts model generation."]
    pub async fn suggestions(&self, id: &str, scope: AnswerContext) -> Result<SuggestionReading> {
        scope.validate()?;
        let reading: SuggestionReading = self
            .client
            .get(
                &["agents", id, "suggestions"],
                &[
                    ("turn_sequence", scope.turn_sequence.to_string()),
                    ("answer_sequence", scope.answer_sequence.to_string()),
                ],
            )
            .await?;
        if ![
            "pending",
            "running",
            "ready",
            "empty",
            "failed",
            "expired",
            "unavailable",
        ]
        .contains(&reading.status.as_str())
            || (reading.status == "ready") != reading.set.is_some()
        {
            return Err(Error::validation("invalid suggestion state"));
        }
        if let Some(set) = &reading.set {
            if set.agent_id != id
                || set.scope != scope
                || set.answer_id != format!("event:{}", scope.answer_sequence)
                || set.id.trim().is_empty()
                || set.viewer_id.trim().is_empty()
                || set.source.version.trim().is_empty()
                || !["generated", "template"].contains(&set.source.kind.as_str())
                || set.items.is_empty()
                || set.items.len() > 3
                || set.items.iter().any(|item| {
                    item.id.trim().is_empty()
                        || item.text.trim().is_empty()
                        || item.text.len() > 256 * 1024
                })
                || set
                    .items
                    .windows(2)
                    .any(|pair| pair[0].rank >= pair[1].rank)
                || set.items.iter().enumerate().any(|(index, item)| {
                    set.items[..index].iter().any(|previous| {
                        previous.id == item.id
                            || previous.text.trim().to_lowercase()
                                == item.text.trim().to_lowercase()
                    })
                })
            {
                return Err(Error::validation("invalid or mismatched suggestion scope"));
            }
        }
        Ok(reading)
    }
}
