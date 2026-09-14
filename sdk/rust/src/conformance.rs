//! Public deployment discovery types.

use serde::Deserialize;

use crate::{Error, Result};

/// Public state exposed while CMA's replacement interfaces are rebuilt.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Conformance {
    /// Service identifier returned by the deployment.
    pub service: String,
    /// Current deployment state.
    pub status: String,
    /// Public interfaces currently exposed by this build.
    pub interfaces: Vec<String>,
}

impl Conformance {
    pub(crate) fn validate(self) -> Result<Self> {
        if self.service.trim().is_empty() || self.status.trim().is_empty() {
            return Err(Error::protocol(
                "conformance requires non-empty service and status values",
            ));
        }
        if self
            .interfaces
            .iter()
            .any(|interface| interface.trim().is_empty())
        {
            return Err(Error::protocol(
                "conformance interfaces must be non-empty strings",
            ));
        }
        Ok(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_public_state() {
        let valid = Conformance {
            service: "cma".into(),
            status: "rebuilding".into(),
            interfaces: Vec::new(),
        };
        assert_eq!(valid.clone().validate(), Ok(valid));

        for invalid in [
            Conformance {
                service: "".into(),
                status: "rebuilding".into(),
                interfaces: Vec::new(),
            },
            Conformance {
                service: "cma".into(),
                status: " ".into(),
                interfaces: Vec::new(),
            },
            Conformance {
                service: "cma".into(),
                status: "rebuilding".into(),
                interfaces: vec!["".into()],
            },
        ] {
            assert!(matches!(invalid.validate(), Err(Error::Protocol { .. })));
        }
    }
}
