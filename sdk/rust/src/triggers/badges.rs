//! Anonymous conditional retrieval of server-rendered Trigger badges.

use super::{ButtonLabel, Theme};
use crate::{
    Client, Error, Result,
    http::{Request, receive},
};
use reqwest::{StatusCode, header};

/// Presentation, attribution, and cache validation for a public badge.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BadgeOptions {
    /// Optional approved label override.
    pub label: Option<ButtonLabel>,
    /// Optional color mode override.
    pub theme: Option<Theme>,
    /// Optional registered placement identity.
    pub placement: Option<String>,
    /// Previously received entity tag for conditional retrieval.
    pub if_none_match: Option<String>,
}

/// Server-rendered badge bytes or an explicit cache hit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BadgeResponse {
    /// Fresh SVG asset returned by the server.
    Modified {
        /// Exact server-rendered markup, returned as inert text without XML sanitization.
        svg: String,
        /// Required nonempty entity tag for the current asset.
        etag: String,
    },
    /// The supplied entity tag still identifies the current asset.
    NotModified {
        /// Required nonempty entity tag for the current asset.
        etag: String,
    },
}

impl Client {
    /// Retrieve a public badge anonymously, preserving conditional 304 semantics.
    pub async fn read_trigger_badge(
        &self,
        public_id: &str,
        options: &BadgeOptions,
    ) -> Result<BadgeResponse> {
        let segments = ["triggers", "public", public_id, "badge.svg"];
        let mut request = Request::get(&segments);
        request.anonymous = true;
        request.statuses.push(StatusCode::NOT_MODIFIED);
        let mut query = Vec::new();
        if let Some(label) = options.label {
            query.push(("label", label.as_str().into()));
        }
        if let Some(theme) = options.theme {
            query.push(("theme", theme.as_str().into()));
        }
        if let Some(placement) = &options.placement {
            query.push(("placement", placement.clone()));
        }
        request.query = &query;
        request.headers.insert(
            header::ACCEPT,
            header::HeaderValue::from_static("image/svg+xml"),
        );
        if let Some(etag) = &options.if_none_match {
            if etag.trim().is_empty() {
                return Err(Error::validation("badge entity tag must not be empty"));
            }
            let value = header::HeaderValue::from_str(etag)
                .map_err(|_| Error::validation("badge entity tag must be a valid HTTP header"))?;
            request.headers.insert(header::IF_NONE_MATCH, value);
        }
        let response = receive(&self.inner, request).await?;
        let etag = response
            .headers
            .get(header::ETAG)
            .and_then(|value| value.to_str().ok())
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| Error::protocol("badge response did not carry an entity tag"))?
            .to_owned();
        if response.status == StatusCode::NOT_MODIFIED {
            if options.if_none_match.is_none() {
                return Err(Error::protocol("unsolicited 304 badge response"));
            }
            if !response.body.is_empty() {
                return Err(Error::protocol(
                    "304 badge response unexpectedly carried a body",
                ));
            }
            return Ok(BadgeResponse::NotModified { etag });
        }
        if !response
            .headers
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| {
                value
                    .split(';')
                    .next()
                    .is_some_and(|kind| kind.trim().eq_ignore_ascii_case("image/svg+xml"))
            })
        {
            return Err(Error::protocol("badge response did not carry SVG content"));
        }
        let svg = String::from_utf8(response.body)
            .map_err(|_| Error::protocol("badge SVG was not UTF-8"))?;
        if !svg.trim_start().starts_with("<svg") {
            return Err(Error::protocol(
                "badge response did not carry the expected SVG prefix",
            ));
        }
        Ok(BadgeResponse::Modified { svg, etag })
    }
}
