use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserOperation {
    Status,
    Enable,
    Disable,
    Observe,
    Release,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrowserRequest {
    pub sandbox_id: String,
    pub runtime_generation: u64,
    pub boot_id: String,
    pub agent_id: crate::AgentSlot,
    pub operation: BrowserOperation,
    #[serde(default)]
    pub observer: Option<String>,
    #[serde(default)]
    pub context_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserState {
    NotEnabled,
    Unsupported,
    Loading,
    Ready,
    Live,
    Paused,
    Disconnected,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowserFrame {
    pub sequence: u64,
    pub captured_at: String,
    pub media_type: String,
    pub width: u32,
    pub height: u32,
    pub data: String,
}

impl std::fmt::Debug for BrowserFrame {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BrowserFrame")
            .field("sequence", &self.sequence)
            .field("encoded_bytes", &self.data.len())
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowserObservation {
    pub state: BrowserState,
    pub context_id: Option<String>,
    pub location: String,
    pub frame: Option<BrowserFrame>,
    pub observers: u32,
    pub captures: u64,
    pub bytes: u64,
    pub min_interval_ms: u32,
    pub fresh_for_ms: u32,
    pub max_frame_bytes: u32,
    pub max_observers: u32,
}

impl BrowserObservation {
    pub fn idle(state: BrowserState) -> Self {
        Self {
            state,
            context_id: None,
            location: "Blank page".into(),
            frame: None,
            observers: 0,
            captures: 0,
            bytes: 0,
            min_interval_ms: 1000,
            fresh_for_ms: 3000,
            max_frame_bytes: 262144,
            max_observers: 16,
        }
    }

    pub fn valid(&self) -> bool {
        use base64::Engine as _;
        self.observers <= 16
            && self.min_interval_ms == 1000
            && self.fresh_for_ms == 3000
            && self.max_frame_bytes == 262144
            && self.max_observers == 16
            && matches!(
                self.location.as_str(),
                "Blank page" | "Local page" | "Web page"
            )
            && self
                .context_id
                .as_ref()
                .is_none_or(|value| uuid::Uuid::parse_str(value).is_ok())
            && (self.state != BrowserState::Live
                || (self.frame.is_some() && self.context_id.is_some()))
            && self.frame.as_ref().is_none_or(|frame| {
                frame.sequence > 0
                    && frame.width == 1280
                    && frame.height == 720
                    && frame.media_type == "image/jpeg"
                    && frame.captured_at.len() <= 32
                    && frame.data.len() <= 349528
                    && base64::engine::general_purpose::STANDARD
                        .decode(&frame.data)
                        .is_ok_and(|bytes| {
                            bytes.len() <= 262144 && bytes.starts_with(&[255, 216, 255])
                        })
            })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowserReceipt {
    pub agent_id: crate::AgentSlot,
    pub boot_id: String,
    pub observation: BrowserObservation,
}

#[async_trait::async_trait]
pub trait BrowserAdapter: Send + Sync {
    async fn browser_control(
        &self,
        _handle: &crate::RuntimeHandle,
        _request: &crate::BrowserControlRequest,
    ) -> crate::Result<crate::BrowserControlReceipt> {
        Err(crate::AdapterError::Unsupported(
            "browser control is unavailable".into(),
        ))
    }

    async fn browser(
        &self,
        handle: &crate::RuntimeHandle,
        request: &BrowserRequest,
    ) -> crate::Result<BrowserReceipt>;
}
