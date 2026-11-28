#[derive(Clone, Copy)]
pub(crate) struct PoolFailure {
    pub(crate) phase: &'static str,
    pub(crate) reason: &'static str,
    pub(crate) message: &'static str,
}

impl PoolFailure {
    pub(crate) const fn retirement(reason: &'static str, message: &'static str) -> Self {
        Self {
            phase: "retirement",
            reason,
            message,
        }
    }

    pub(crate) const fn apply(reason: &'static str, message: &'static str) -> Self {
        Self {
            phase: "reconciliation",
            reason,
            message,
        }
    }
}
