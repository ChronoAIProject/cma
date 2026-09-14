#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SandboxModelError {
    #[error("invalid {resource} transition from {from} to {to}")]
    InvalidTransition {
        resource: &'static str,
        from: &'static str,
        to: &'static str,
    },
    #[error("invalid {field}")]
    InvalidValue { field: &'static str },
    #[error("{resource} prerequisite is not satisfied: {detail}")]
    Prerequisite {
        resource: &'static str,
        detail: &'static str,
    },
}

impl SandboxModelError {
    pub const fn invalid_value(field: &'static str) -> Self {
        Self::InvalidValue { field }
    }
}
