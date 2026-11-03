use std::time::{Duration, Instant, SystemTime};
use super::*;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SuggestionUsage {
    pub model: Option<String>,
    pub input_tokens: Option<u64>,
    pub cached_input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cost: Option<ReportedSuggestionCost>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportedSuggestionCost {
    pub amount: String,
    pub currency: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SuggestionFailure { Unavailable, Timeout, InvalidResponse, Interrupted }

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SuggestionObservation {
    pub elapsed_ms: u64,
    pub usage: SuggestionUsage,
    pub failure: Option<SuggestionFailure>,
}

pub struct SuggestionGeneration {
    pub output: String,
    pub usage: SuggestionUsage,
}

pub struct SuggestionCompletion {
    pub status: SuggestionStatus,
    pub set: Option<SuggestedReplySet>,
    pub observation: SuggestionObservation,
}

#[async_trait]
pub trait SuggestionGenerator: Send + Sync {
    async fn generate(&self, work: &SuggestionWork) -> Result<SuggestionGeneration, SuggestionFailure>;
}

pub fn unix_millis() -> u64 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_millis().min(i64::MAX as u128) as u64
}

pub struct SuggestionGenerationService { store: Arc<dyn SuggestionStore> }

impl SuggestionGenerationService {
    pub fn new(store: Arc<dyn SuggestionStore>) -> Arc<Self> { Arc::new(Self { store }) }

    pub async fn run_next(&self, generator: &dyn SuggestionGenerator) -> Result<bool, AgentReadError> {
        let Some(work) = self.store.claim_suggestions(unix_millis()).await? else { return Ok(false); };
        let started = Instant::now();
        let generated = tokio::time::timeout(Duration::from_secs(GENERATION_TIMEOUT_SECONDS), generator.generate(&work)).await;
        let mut observation = SuggestionObservation { elapsed_ms: started.elapsed().as_millis() as u64,
            usage: SuggestionUsage::default(), failure: None };
        let items = match generated {
            Ok(Ok(response)) => {
                observation.usage = response.usage;
                match super::quality::candidates(&response.output, &work.context) {
                    Some(items) => items,
                    None => { observation.failure = Some(SuggestionFailure::InvalidResponse); Vec::new() }
                }
            }
            Ok(Err(failure)) => { observation.failure = Some(failure); Vec::new() }
            Err(_) => { observation.failure = Some(SuggestionFailure::Timeout); Vec::new() }
        };
        let status = if observation.failure.is_some() { SuggestionStatus::Failed }
            else if items.is_empty() { SuggestionStatus::Empty } else { SuggestionStatus::Ready };
        let set = (status == SuggestionStatus::Ready).then(|| SuggestedReplySet {
            id: work.id.clone(), viewer_id: work.owner.to_string(), agent_id: work.agent_id.to_string(),
            answer_id: format!("event:{}", work.scope.answer_sequence), scope: work.scope,
            source: SuggestionSource { kind: "generated".into(), version: work.strategy_version.clone() },
            items: items.into_iter().enumerate().map(|(rank, text)| SuggestedReply {
                id: format!("{}:{rank}", work.id), text, rank: rank as u32,
            }).collect(),
        });
        self.store.complete_suggestions(&work, SuggestionCompletion { status, set, observation }).await?;
        Ok(true)
    }
}
