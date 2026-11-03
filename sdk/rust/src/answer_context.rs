#![doc = "Conditional input bound to a durable successfully completed answer."]

use serde::{Deserialize, Serialize};

#[doc = "The final answer that must still be eligible when new input is admitted."]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnswerContext {
    #[doc = "Durable sequence of the answer's turn start."]
    pub turn_sequence: u64,
    #[doc = "Durable sequence of the final assistant message."]
    pub answer_sequence: u64,
}

impl AnswerContext {
    pub(crate) fn validate(&self) -> crate::Result<()> {
        if self.turn_sequence == 0
            || self.answer_sequence <= self.turn_sequence
            || self.answer_sequence > i64::MAX as u64
        {
            return Err(crate::Error::validation(
                "answer context requires ordered durable sequences",
            ));
        }
        Ok(())
    }
}
