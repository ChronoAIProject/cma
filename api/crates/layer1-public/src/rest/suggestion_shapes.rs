use layer2_core::sandbox::suggestions::*;
use serde::Serialize;
use utoipa::ToSchema;

#[derive(Serialize, ToSchema)]
pub(crate) struct SuggestionReadingView {
    #[schema(value_type = String, example = "ready")]
    pub status: SuggestionStatus,
    pub set: Option<SuggestionSetView>,
    pub observation: Option<SuggestionObservationView>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SuggestionSetView {
    pub id: String,
    pub viewer_id: String,
    pub agent_id: String,
    pub answer_id: String,
    pub scope: SuggestionScopeView,
    pub source: SuggestionSourceView,
    pub items: Vec<SuggestionItemView>,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct SuggestionScopeView {
    pub turn_sequence: u64,
    pub answer_sequence: u64,
}
#[derive(Serialize, ToSchema)]
pub(crate) struct SuggestionSourceView {
    pub kind: String,
    pub version: String,
}
#[derive(Serialize, ToSchema)]
pub(crate) struct SuggestionItemView {
    pub id: String,
    pub text: String,
    pub rank: u32,
}
#[derive(Serialize, ToSchema)]
pub(crate) struct SuggestionObservationView {
    pub elapsed_ms: u64,
    pub usage: SuggestionUsageView,
    #[schema(value_type = Option<String>)]
    pub failure: Option<SuggestionFailure>,
}
#[derive(Serialize, ToSchema)]
pub(crate) struct SuggestionUsageView {
    pub model: Option<String>,
    pub input_tokens: Option<u64>,
    pub cached_input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cost: Option<SuggestionCostView>,
}
#[derive(Serialize, ToSchema)]
pub(crate) struct SuggestionCostView {
    pub amount: String,
    pub currency: String,
}

impl From<SuggestionReading> for SuggestionReadingView {
    fn from(reading: SuggestionReading) -> Self {
        Self {
            status: reading.status,
            set: reading.set.map(|set| SuggestionSetView {
                id: set.id,
                viewer_id: set.viewer_id,
                agent_id: set.agent_id,
                answer_id: set.answer_id,
                scope: SuggestionScopeView {
                    turn_sequence: set.scope.turn_sequence,
                    answer_sequence: set.scope.answer_sequence,
                },
                source: SuggestionSourceView {
                    kind: set.source.kind,
                    version: set.source.version,
                },
                items: set
                    .items
                    .into_iter()
                    .map(|item| SuggestionItemView {
                        id: item.id,
                        text: item.text,
                        rank: item.rank,
                    })
                    .collect(),
            }),
            observation: reading
                .observation
                .map(|observation| SuggestionObservationView {
                    elapsed_ms: observation.elapsed_ms,
                    failure: observation.failure,
                    usage: SuggestionUsageView {
                        model: observation.usage.model,
                        input_tokens: observation.usage.input_tokens,
                        cached_input_tokens: observation.usage.cached_input_tokens,
                        output_tokens: observation.usage.output_tokens,
                        cost: observation.usage.cost.map(|cost| SuggestionCostView {
                            amount: cost.amount,
                            currency: cost.currency,
                        }),
                    },
                }),
        }
    }
}
