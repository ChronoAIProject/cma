//! Transient runtime admission result; never a persisted command failure enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RuntimeCapacityError {
    #[error("runtime_stream_capacity_exhausted")]
    Stream,
    #[error("runtime_request_capacity_exhausted")]
    Request,
}
impl RuntimeCapacityError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::Stream => "runtime_stream_capacity_exhausted",
            Self::Request => "runtime_request_capacity_exhausted",
        }
    }
    pub fn from_adapter(error: &layer3_adapter::AdapterError) -> Option<Self> {
        match error {
            layer3_adapter::AdapterError::RuntimeCapacity(capacity) => Some(match capacity.kind {
                layer3_adapter::RuntimeCapacityKind::Stream => Self::Stream,
                layer3_adapter::RuntimeCapacityKind::Request => Self::Request,
            }),
            _ => None,
        }
    }
}
