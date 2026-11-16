use super::*;
use serde_json::{json, Value};

#[derive(Debug)]
pub(crate) enum Failure {
    Store(Error),
    Refusal { code: &'static str, observed: Value },
}
impl From<Error> for Failure {
    fn from(error: Error) -> Self {
        Self::Store(error)
    }
}
impl Failure {
    pub fn refuse(code: &'static str, observed: Value) -> Self {
        Self::Refusal { code, observed }
    }
    pub fn code(&self) -> &'static str {
        match self {
            Self::Refusal { code, .. } => code,
            Self::Store(error) => match error {
                Error::Busy => "concurrent_store_write",
                Error::Lost => "evidence_unavailable",
                Error::Expired => "evidence_expired",
                Error::Capacity => "inspection_capacity",
                Error::Corrupt => "invalid_operator_evidence",
                Error::Unavailable => "store_unavailable",
            },
        }
    }
    pub fn document(&self, mutation: bool) -> Value {
        let code = if mutation && matches!(self, Self::Store(Error::Unavailable)) {
            "operator_outcome_uncertain"
        } else {
            self.code()
        };
        let observed = match self {
            Self::Refusal { observed, .. } => observed.clone(),
            _ => Value::Null,
        };
        json!({"code":code,"observed":observed})
    }
    pub(super) fn transient_head(&self) -> bool {
        matches!(
            self,
            Self::Store(Error::Busy | Error::Lost | Error::Expired)
                | Self::Refusal {
                    code: "binding_version_mismatch",
                    ..
                }
        )
    }
    pub(super) fn storage(self) -> Error {
        match self {
            Self::Store(error) => error,
            Self::Refusal { .. } => Error::Corrupt,
        }
    }
}
impl Selected {
    pub(super) fn refusal(&self, code: &'static str) -> Failure {
        let mut observed = json!(self.candidate);
        observed["lease_until_ms"] = json!(self.head.authority.as_ref().map(|a| a.until_ms));
        observed["guard_until"] = json!(self.evidence.live_until);
        Failure::refuse(code, observed)
    }
}
