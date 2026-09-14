//! Generic count limits used by later resource services.

use serde::{Deserialize, Serialize};

/// Optional maximum number of live resources in one scope.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CountLimit(pub Option<u32>);

impl CountLimit {
    pub fn new(maximum: Option<u32>) -> Result<Self, crate::CoreError> {
        if maximum == Some(0) {
            return Err(crate::CoreError::InvalidRequest(
                "a configured count limit must be greater than zero".into(),
            ));
        }
        Ok(Self(maximum))
    }

    pub fn maximum(self) -> Option<u32> {
        self.0
    }

    pub fn admits(self, current: u32, additional: u32) -> bool {
        self.0.is_none_or(|maximum| {
            current
                .checked_add(additional)
                .is_some_and(|total| total <= maximum)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_limits_are_unbounded_and_configured_limits_are_inclusive() {
        assert!(CountLimit::default().admits(u32::MAX, 1));
        let limit = CountLimit::new(Some(3)).unwrap();
        assert!(limit.admits(2, 1));
        assert!(!limit.admits(3, 1));
        assert_eq!(limit.maximum(), Some(3));
        assert!(CountLimit::new(Some(0)).is_err());
    }
}
