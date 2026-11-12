use super::AgentUsage;

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct UsageFold {
    pub(super) observed: Option<AgentUsage>,
    closed_input: u64,
    closed_output: u64,
    generation: Option<u64>,
    input_peak: u64,
    output_peak: u64,
    context_used: u64,
    context_window: Option<u64>,
}

impl UsageFold {
    pub(super) fn open_generation(&mut self, generation: u64) -> Result<(), String> {
        if self.generation.is_some_and(|current| generation <= current) {
            return Err("agent usage generation is not increasing".into());
        }
        if self.generation.is_some() {
            self.closed_input = self.closed_input.saturating_add(self.input_peak);
            self.closed_output = self.closed_output.saturating_add(self.output_peak);
        }
        self.generation = Some(generation);
        self.input_peak = 0;
        self.output_peak = 0;
        self.context_used = 0;
        self.context_window = None;
        Ok(())
    }

    pub(super) fn observe(
        &mut self,
        input_tokens: u64,
        output_tokens: u64,
        context_used: u64,
        context_window: Option<u64>,
    ) -> Result<(), String> {
        if self.generation.is_none() {
            return Err("agent usage precedes its conversation".into());
        }
        self.input_peak = self.input_peak.max(input_tokens);
        self.output_peak = self.output_peak.max(output_tokens);
        if context_used > 0 {
            self.context_used = context_used;
        }
        if let Some(context_window) = context_window {
            self.context_window = Some(context_window);
        }
        Ok(())
    }

    pub(super) fn reading(&self) -> AgentUsage {
        if let Some(observed) = self.observed {
            return observed;
        }
        AgentUsage {
            current_turn: None,
            input_tokens: self.closed_input.saturating_add(self.input_peak),
            output_tokens: self.closed_output.saturating_add(self.output_peak),
            context_used: self.context_used,
            context_window: self.context_window,
        }
    }
}
