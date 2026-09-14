//! Wire shapes used by the temporary OpenAPI document.

use serde::Serialize;
use utoipa::ToSchema;

/// Public state exposed while CMA's replacement interfaces are rebuilt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct ConformanceDocument {
    pub service: &'static str,
    pub status: &'static str,
    pub interfaces: Vec<&'static str>,
}

impl Default for ConformanceDocument {
    fn default() -> Self {
        Self {
            service: "cma",
            status: "rebuilding",
            interfaces: Vec::new(),
        }
    }
}
