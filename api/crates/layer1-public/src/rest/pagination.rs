//! Bounded query parsing retained for resource routes introduced after #802.

use serde::Deserialize;

/// Optional page values accepted at the HTTP boundary.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageParams {
    pub limit: Option<u32>,
    pub cursor: Option<String>,
}

impl PageParams {
    pub fn into_request(self) -> Result<layer2_core::PageRequest, layer2_core::CoreError> {
        layer2_core::PageRequest::new(
            self.limit.unwrap_or(layer2_core::DEFAULT_PAGE_SIZE),
            self.cursor,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_bounds_are_owned_by_the_core() {
        assert_eq!(
            PageParams::default().into_request().unwrap().limit,
            layer2_core::DEFAULT_PAGE_SIZE
        );
        assert!(PageParams {
            limit: Some(0),
            cursor: None,
        }
        .into_request()
        .is_err());
    }
}
