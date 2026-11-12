//! Immutable ownership of a retained frame's decoded domain effect.

use std::{fmt, ops::Deref, sync::Arc};

use crate::ConversationUpdate;

/// The transport selects shared ownership; disabled transports retain legacy cloning.
/// The payload cannot be mutated or extracted independently of its owning envelope.
#[derive(Clone)]
pub struct RuntimeOutputUpdate(Storage);

#[derive(Clone)]
enum Storage {
    Owned(Box<ConversationUpdate>),
    Shared(Arc<ConversationUpdate>),
}

impl RuntimeOutputUpdate {
    pub fn new(update: ConversationUpdate, shared: bool) -> Self {
        Self(if shared {
            Storage::Shared(Arc::new(update))
        } else {
            Storage::Owned(Box::new(update))
        })
    }

    pub fn is_shared(&self) -> bool {
        matches!(self.0, Storage::Shared(_))
    }
}

impl From<Box<ConversationUpdate>> for RuntimeOutputUpdate {
    fn from(update: Box<ConversationUpdate>) -> Self {
        Self(Storage::Owned(update))
    }
}

impl Deref for RuntimeOutputUpdate {
    type Target = ConversationUpdate;

    fn deref(&self) -> &Self::Target {
        match &self.0 {
            Storage::Owned(update) => update,
            Storage::Shared(update) => update,
        }
    }
}

impl fmt::Debug for RuntimeOutputUpdate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.deref().fmt(formatter)
    }
}

#[cfg(test)]
#[path = "runtime_output_update_tests.rs"]
mod tests;
