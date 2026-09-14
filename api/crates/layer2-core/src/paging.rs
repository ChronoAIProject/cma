//! Bounded cursor paging shared by retained store ports.

use serde::{Deserialize, Serialize};

pub const DEFAULT_PAGE_SIZE: u32 = 50;
pub const MAX_PAGE_SIZE: u32 = 200;

/// One bounded store request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageRequest {
    pub limit: u32,
    pub cursor: Option<String>,
}

impl PageRequest {
    pub fn new(limit: u32, cursor: Option<String>) -> Result<Self, crate::CoreError> {
        if !(1..=MAX_PAGE_SIZE).contains(&limit) {
            return Err(crate::CoreError::InvalidRequest(format!(
                "page limit must be between 1 and {MAX_PAGE_SIZE}"
            )));
        }
        if cursor.as_ref().is_some_and(|value| value.is_empty()) {
            return Err(crate::CoreError::InvalidRequest(
                "page cursor must not be empty".into(),
            ));
        }
        Ok(Self { limit, cursor })
    }
}

impl Default for PageRequest {
    fn default() -> Self {
        Self {
            limit: DEFAULT_PAGE_SIZE,
            cursor: None,
        }
    }
}

/// One page and the opaque cursor for the following page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
}

impl<T> Page<T> {
    pub fn new(items: Vec<T>, next_cursor: Option<String>) -> Self {
        Self { items, next_cursor }
    }
}
