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

/// The last durable cumulative observation is authoritative even when older
/// generation counters or later raw Usage events remain in the log. Callers
/// must first prove the complete prefix and select its last UsageUpdated row.
impl super::AgentUsageReading {
    pub fn from_summary(
        agent: &super::Agent,
        entry: &super::AgentLogEntry,
    ) -> Result<Self, String> {
        if entry.agent_id != agent.id
            || entry.seq == 0
            || entry.seq > agent.conversation.last_seq
            || crate::seconds_from_rfc3339(&entry.appended_at).is_none()
        {
            return Err("usage summary source identity is invalid".into());
        }
        let super::AgentLogKind::UsageUpdated {
            input_tokens,
            output_tokens,
            context_used,
            context_window,
            turn_sequence,
            turn_input_tokens,
            turn_output_tokens,
        } = entry.kind
        else {
            return Err("usage summary has no cumulative observation".into());
        };
        Ok(Self {
            agent_id: agent.id.clone(),
            through_seq: agent.conversation.last_seq,
            usage: AgentUsage {
                input_tokens,
                output_tokens,
                context_used,
                context_window,
                current_turn: turn_sequence.map(|turn_sequence| super::AgentTurnUsage {
                    turn_sequence,
                    input_tokens: turn_input_tokens,
                    output_tokens: turn_output_tokens,
                }),
            },
        })
    }
}
