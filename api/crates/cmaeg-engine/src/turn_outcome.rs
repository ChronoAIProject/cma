//! Events passed to delivery, retaining context only for the lifetime of a turn.

#[derive(Debug, Clone, PartialEq)]
pub enum TurnOutcome {
    Accepted {
        response_id: String,
        thread_reply: bool,
    },
    TextDelta {
        text: String,
    },
    MessageCommitted {
        text: String,
    },
    ToolStarted {
        name: String,
    },
    ToolFinished {
        name: String,
        ok: bool,
    },
    Completed {
        final_text: Option<String>,
    },
    Failed {
        code: String,
        message: String,
    },
    Incomplete {
        reason: String,
    },
}
impl TurnOutcome {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Accepted { .. } => "accepted",
            Self::TextDelta { .. } => "text_delta",
            Self::MessageCommitted { .. } => "message_committed",
            Self::ToolStarted { .. } => "tool_started",
            Self::ToolFinished { .. } => "tool_finished",
            Self::Completed { .. } => "completed",
            Self::Failed { .. } => "failed",
            Self::Incomplete { .. } => "incomplete",
        }
    }
    pub fn terminal(&self) -> bool {
        matches!(
            self,
            Self::Completed { .. } | Self::Failed { .. } | Self::Incomplete { .. }
        )
    }
}
