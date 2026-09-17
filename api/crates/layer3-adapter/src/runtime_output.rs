//! Private runtime delivery receipts. These are not public API response fields.

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

/// An immutable native output position, scoped again by the durable Agent generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeOutputSource {
    pub stream_id: String,
    pub cursor: u64,
    pub native_conversation_id: String,
    pub payload_sha256: String,
}

/// Only the durable consumer may release a retained runtime frame.
#[derive(Clone)]
pub struct RuntimeOutputReceipt {
    pub source: RuntimeOutputSource,
    acknowledgements: mpsc::Sender<RuntimeOutputSource>,
}

impl RuntimeOutputReceipt {
    pub fn new(
        source: RuntimeOutputSource,
        acknowledgements: mpsc::Sender<RuntimeOutputSource>,
    ) -> Self {
        Self {
            source,
            acknowledgements,
        }
    }

    pub async fn acknowledge(&self) -> crate::Result<()> {
        self.acknowledgements
            .send(self.source.clone())
            .await
            .map_err(|_| {
                crate::AdapterError::Unavailable("runtime acknowledgement link ended".into())
            })
    }
}

impl std::fmt::Debug for RuntimeOutputReceipt {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("RuntimeOutputReceipt")
            .field(&self.source)
            .finish()
    }
}
