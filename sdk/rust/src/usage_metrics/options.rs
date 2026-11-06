//! Bounded administrator filter discovery, independent of report readiness.
use super::UsageMetrics;
use crate::{Error, Result, http::Request};
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};

/// The retained directory to search.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OptionKind {
    /// Owner display names.
    Owner,
    /// Sandbox names, optionally narrowed by owner.
    Sandbox,
    /// Runtime versions.
    Runtime,
}

/// One bounded search page or resolution of a previously selected value.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OptionQuery {
    /// Directory to search.
    pub kind: OptionKind,
    /// Literal display-name search, at most 128 bytes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub search: Option<String>,
    /// Owner subject, supported only for Sandbox choices.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// Resolve this opaque value; mutually exclusive with search and cursor.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected: Option<String>,
    /// Authenticated continuation returned for the same query and principal.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// Page size from 1 through 50; defaults to 50.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u16>,
}

/// Plain label and opaque selection value. Only actual duplicate labels carry context.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FilterOption {
    /// Internal filter value; do not present this as the label.
    pub value: String,
    /// Human-readable display name or safe unnamed fallback.
    pub label: String,
    /// Secondary context for duplicate options; omit from selected labels.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disambiguator: Option<String>,
}

/// Bounded options and an optional continuation cursor.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FilterOptions {
    /// At most 50 options.
    pub items: Vec<FilterOption>,
    /// No further page when absent.
    pub next: Option<String>,
}

impl UsageMetrics<'_> {
    /// Search choices without creating a report. Requires administrator authority
    /// on every request. Search text stays in the JSON body, outside access-log URLs.
    pub async fn options(&self, query: &OptionQuery) -> Result<FilterOptions> {
        if !self.admin {
            return Err(Error::validation(
                "filter options require administrator reporting",
            ));
        }
        if query.limit.is_some_and(|limit| !(1..=50).contains(&limit)) {
            return Err(Error::validation("option limit must be from 1 through 50"));
        }
        let request = Request::operation(
            Method::POST,
            &["admin", "usage", "metrics", "options"],
            Some(query),
            StatusCode::OK,
        )?;
        self.client.request(request).await
    }
}
